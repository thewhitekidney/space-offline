# Porting EvEJS subsystems, bit by bit

> **Demoted 2026-10-28** — a finished plan: §1's licence decision was taken (the owner chose AGPL-3.0 on 2026-10-20, which the text itself records), and the source-port rounds ran as lanes. Current truth: `docs/port-tracker.md`, and `docs/lanes/README.md` for how lanes work. Kept for history; do not brief a lane from this file. **Superseded 2026-11-03:** the tree is under the No Money Share-Alike Licence (`LICENSE.md`) and holds no EvEJS code; the AGPL decision below is history.

Written 2026-10-20, after a working EvEJS was stood up in a container and
driven with a real V24.01 client (`docs/evejs/evejs-run-safely.md`). The owner's
assessment: *"they have numerous subsystems implemented, a market server,
character skills and attributes, implants, fitting, docking, warping, jumping,
sov mechanics"* — and the ask is a subagent-optimised plan to bring those over
incrementally, assuming theirs are correct and iterating later.

This plan takes that ask literally on the **what** and changes the **how**,
for one reason that is evidence rather than caution. See §1.

Contents: 1 the one decision, 2 why a source port imports bugs, 3 the unit of
work, 4 Round 0 (shared, serial), 5 Round 1 (docked, wide), 6 Round 2
(undocked, gated), 7 the lane brief template, 8 what this does not solve.

---

## 1. The one decision, stated once

**EvEJS is AGPL-3.0-only. `server/Cargo.toml` declares
`LicenseRef-Proprietary`. `README.md:5` promises "Our own code only."** Their
marshal codec is itself ported from EVEmu, so the chain runs back further than
the archive does.

Copying their source — or transcribing it closely enough to be a derivative
work — relicenses this tree. Three ways forward, and this is the owner's call,
not a technical one:

| | what it costs |
|---|---|
| **(a) Relicense to AGPL-3.0** | source porting becomes legitimate. §13 adds network copyleft: anyone permitted to connect may demand source. For an invite-only server that is survivable. It also pulls in the EVEmu lineage, and `README.md`'s promise has to be rewritten rather than quietly broken |
| **(b) Stay clean-room, port behaviour** | more work per subsystem, no licence chain, the promise stands, **and the result is correct for our build rather than theirs** (§2) |
| **(c) Two trees** | an AGPL fork beside a clean one. Twice the maintenance, and the clean one can never take a fix from the other |

**Recommendation was (b).** **The owner chose (a) on 2026-10-20 and the tree
is relicensed**: `LICENSE` is the AGPL-3.0 text, `server/Cargo.toml` declares
`AGPL-3.0-only`, and `README.md` says so. Source porting from EvEJS is now
legitimate.

§§3–7 stand almost unchanged — the rounds, the file ownership and the lane
briefs are the same. What changes is each lane's *source of truth*: "their
module **and** the capture", rather than the capture alone. Both still matter,
and §2 is why: their code is correct for build 3396210, and every ported
subsystem needs its required-key and constant assumptions checked against
**our** client before it is trusted. The capture is how that check is made
cheap. A port that skips it imports their build silently.

### 1a. Three obligations that now attach, and one that does not

**Attribution, per file.** Any file carrying ported EvEJS code names it in its
header — what was taken, from which module, and that it is AGPL-3.0-only.
"Rewrote it from their logic" counts. This is both the licence's requirement
and, more usefully, the marker a future reader needs to know which assumptions
came from build 3396210.

**Network copyleft (§13).** Anyone permitted to interact with the server over
a network may demand corresponding source. For an invite-only sandbox among
friends that is survivable and arguably irrelevant, but it is now a real
obligation rather than a hypothetical, and it travels with every copy.

**No sublicensing back out.** The tree cannot return to proprietary. A
contributor who wants their work under other terms has to say so before it
lands.

**What the relicence does *not* permit: CCP's content.** EvEJS ships ~505 MB
of CCP-derived static data — dungeon geometry compiled from a client, 2,878
missions with verbatim prose, 10,941 agent records, harvested ESI. **The AGPL
has no bearing on any of it**, and taking it would break the owner's standing
rule directly. The clean-room position on CCP data is unchanged and absolute:
we read the player's own client at runtime, or point them at CCP's published
SDE, and we ship neither. A licence that lets us take another server's code
does not let either of us ship CCP's.

---

## 2. Why a source port imports bugs — measured, not argued

"Assume they are correctly implemented" is the assumption that broke within
ten minutes of first contact, twice, in both directions:

**Their server could not log our client in.** Their `CryptoHandshakeAck` omits
`access_token` and `computer_hash`. Our client reached character selection and
died on `KeyError: 'access_token'` at `connectionService.py(175) LoginSso`.
Our `docs/reference/login-path.md` §H10 has had both keys documented as *required on the
SSO path and indexed directly* since before we had ever heard of EvEJS, and
`handshake.rs` emits both with a test. **We were right and they were wrong,
for our client.**

**Their Destiny constants are wrong for our build.** Their `DstBallMode` table
has `MINIBALL 9, FIELD 10`; ours, cross-checked three ways against this
client, has `FIELD 9, MINIBALL 10`
(`docs/re/destiny-ball-crosscheck.md` §2.3).

Both are the same root cause: **their target is client 3396210, ours is
3619879.** Not a different game — a different build, three revisions apart, and
the differences are *silent*. A ported subsystem carries its build's
assumptions invisibly, and we would discover them one at a time as
wrong-shaped payloads months later, in code we did not write and do not
understand.

The same comparison also went their way twice: nine of eleven `[I]` ball
fields got names from their encoder, and their `/addisk` + `/setisk` pair is a
better interface than the single setter we would have written. **So they are
worth learning from everywhere and copying from nowhere** — which is not a
compromise position, it is what the evidence supports.

---

## 3. The unit of work: a captured contract, not a file of theirs

We now have something we did not have this morning: **a working reference
server we can drive, with a client we control, and a tap between them.**

`tools/jail-forward.py` already sits in the middle of every packet. Round 0
turns it into a framing-aware recorder writing our own `.evecap` format
(`capture.rs` §File format), which means **`evecap` decodes EvEJS traffic with
our own marshal, today.** No pcap, no second decoder, no root.

So the deliverable per subsystem is a **wire contract**:

```
calls        service.method(args...) -> return shape
notifications what gets pushed, with what payload, in what order
errors       which UserError keys, which empty-shape variants
fixtures     the .evecap itself, committed as a regression test
```

Three sources, in strict priority:

1. **The capture** — what their server sent and *our client accepted*. This is
   stronger evidence than their source code, because it is validated against
   the build we care about.
2. **The client's own bytecode** — `out/pyc/`, 12,774 files, ground truth.
   Settles every "is this key required" question, which is exactly where their
   server was wrong.
3. **Their decomposition** — module boundaries, operation ordering, interface
   shape. Facts about the problem, not expression. Read freely, cite the
   client.

A lane that produces a contract and an implementation that satisfies it has
ported the subsystem in every sense the owner asked for, and the result is
ours.

**Driving their server is cheap, which is what makes this work.** Their GM
console is ungated and enormous: `/giveskill`, `/allskills`, `/ship`, `/fit`,
`/unfit`, `/create`, `/giveitem`, `/hangar`, `/corpcreate`, `/setalliance`,
`/sov`, `/upwell`, `/keepstar`, `/tr`, `/solar`, `/tele`, `/addisk`, `/npc`.
One line in a chat window exercises a subsystem end to end. That is a test
harness we did not have to build.

---

## 4. Round 0 — coordinator only, serial, nothing parallel

Rule 2 of `docs/lanes/README.md`: *shared files are removed, not scheduled*,
and the removal goes first, by the coordinator, alone. Everything here is a
shared decision that would otherwise be made four times inconsistently.

**0.1 The capture tap.** `jail-forward.py` splits on EVE framing (`u32` LE,
plen in bits 0..27, bit 28 OOB, bit 29 ZLIB — `docs/reference/framing.md`) and writes
`.evecap`. Every later lane's input comes from this, so it cannot be a lane.
*Signal:* `evecap` prints a decoded EvEJS login.

**0.2 The ball blob.** With 0.1 in hand, undock on their server and capture
the `SetState`. That is the byte-exact reference blob
`destiny-ball-crosscheck.md` §6 has been blocked on, and it gates the entire
undocked tier. Confirms or refutes nineteen fields at once.
*Signal:* a `DoDestinyUpdate` decoded, field-by-field against §1 of that doc.

**0.3 The identity split** (roadmap Step B, already announced twice). Three
hardcoded `user_id: 1`, ~15 `characters.first()`. Touches the hot files
`session.rs`, `dispatch.rs`, `conn.rs`. **Corp and alliance cannot be done
without it** — we learned from `/corpcreate` that membership lives in the
session and changing it means re-pushing sessions for a *set* of characters.
*Signal:* two clients, two identities, `station.GetGuests` lists both.

**0.4 One `Notify` commit.** Every variant Round 1 needs, added at once:
`OnGodmaItemChange`, `OnOwnOrdersChanged`, `OnIndustryJob`,
`OnCorporationChanged`, `OnSkillTrainingStarted`. Otherwise five lanes edit
`push.rs`.

**0.5 The item location model.** Fitting, docking, cargo, market escrow and
industry input all move items between owners and locations. `item.rs` has
`Store` but no flag/location semantics. One decision, made once, or four lanes
invent four.

Round 0 is the whole cost of parallelism, and it is not optional. Skipping it
produces a round that merges badly and a second round spent undoing it.

---

## 5. Round 1 — docked subsystems, wide and parallel

Nothing here needs Destiny, so all of it can run at once. One lane per file,
per rule 1.

| lane | owns | drive it with | contract target |
|---|---|---|---|
| `skills` | `service/character.rs`, `skill.rs` | `/giveskill`, `/allskills`, `/removeskill` | training queue, attributes, SP accrual, `skillHandler`/`skillMgr` |
| `implants` | `service/character.rs` (after `skills` parks) or new `implant.rs` | `/create <implant>`, `/fit me` | implant slots, attribute modifiers, `dogmaIM` |
| `fitting` | `service/inventory.rs` | `/ship`, `/fit me`, `/unfit me` | fit/unfit, slot validity, `charFittingMgr`, `dogmaIM` |
| `market` | `service/market.rs` | `/addisk`, then buy and sell in-game | order placement, matching, escrow, `OnOwnOrdersChanged` |
| `industry` | `service/industry.rs` | `/bp`, `/bpauto`, `/minerals` | job install, cost, completion, `OnIndustryJob` |
| `corp` | `service/corp.rs` | `/corpcreate`, `/setalliance`, `/joinalliance` | create-and-join as one op, session refresh for the affected **set** |
| `wallet` | `service/wallet.rs` | `/addisk`, `/setisk`, `/lp`, `/addlp` | relative and absolute mutators, journal entry naming the actor, clamping |

Six or seven lanes, each with one file, one capture, and no shared edits left.

## 6. Round 2 — undocked, gated on 0.2

Hard prerequisite: a ship in space. Do not start these before Destiny draws
one, or every lane blocks on the same missing thing.

| lane | owns | drive it with |
|---|---|---|
| `destiny-core` | new `space/` module — **coordinator or a single lane, it is the foundation** | undock, then `/tr`, `/deadwarp` |
| `docking` | `service/space.rs` | `/dock`, undock |
| `warp` | `space/` after `destiny-core` parks | `/tr`, `/solar` |
| `jump` | `service/space.rs` + map | `/tele`, gates |
| `sov` | new `service/sov.rs` | `/sov`, `/sovauto`, `/keepstar`, `/upwell` |

Round 2 is deliberately thinner. `destiny-core` is one lane, not four,
because a foundation built by a committee is the thing rule 2 exists to
prevent.

## 7. The lane brief template

Each lane gets exactly this, and nothing else:

```
GOAL        one subsystem, one wire contract, one implementation.
OWNS        <file>. Write nothing else. Foreign compile errors are expected
            and are not yours -- ignore every error outside your file.
INPUT       captures/evejs-<subsystem>-NNNN.evecap  (decode: evecap --tags)
            out/pyc/<the client modules that call this service>
            docs/<the RE doc for this area>
METHOD      1. decode the capture; list every call, return and notification
            2. for EVERY key the client indexes, confirm REQUIRED-ness in the
               client's own bytecode. Their server omits required keys -- this
               step is why we are not porting their source
            3. write the contract into docs/re/<subsystem>-contract.md
            4. implement in your file, from the contract
            5. add the capture as a regression fixture
FORBIDDEN   committing, pushing, running tools/gate.sh, git add -A, git stash,
            editing a hot file from docs/lanes/README.md
PARK        docs/lanes/<name>.md: found / changed / deliberately did not /
            next thing. A correction to the coordinator belongs here too.
```

Step 2 is the one that must not be dropped. It is the difference between this
plan and a source port, and today is what proved it necessary.

## 8. What this does not solve

- **Loot.** Their `npcLootTables` is invented or ESI-derived; the client ships
  none (`docs/re/dungeon-model.md`). A capture of their loot is a capture of
  their fiction. This stays a design task.
- **NPC AI.** Their `npcBehaviorLoop` is a reasonable decomposition to learn
  from, but the parameters are CCP's (`AI_*` dogma attributes) and the engine
  is ours to write either way.
- **Their build's content.** Universe, types and stations come from the
  3396210 SDE in their tree. We read the player's own client at runtime and
  ship nothing. That does not change.
- **Correctness.** Every contract is a hypothesis until our client accepts it.
  That is what Round 1's fixtures are for, and why the owner's "test and
  iterate later" is right — as long as "later" means against *our* client.
