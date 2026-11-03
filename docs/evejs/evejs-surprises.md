<!-- Provenance: the `evejs-surprises` lane, 2026-10-22. Read-only outside this
     file. The EvEJS archive was **not extracted and nothing was read out of
     it**: every EvEJS-side statement here rests on this project's own records
     of it (`docs/archive/evejs-261018-reference.md`, `docs/evejs/evejs-subsystem-gap.md`,
     `docs/re/evejs-crosscheck.md`, `docs/re/evejs-data.md`,
     `docs/re/destiny-ball-crosscheck.md`, `docs/lanes/evejs-probe.md`,
     `docs/port-tracker.md`) and is marked where it does. The *client* side of
     every load-bearing claim was re-verified in this lane against build
     3619879 -- see the "re-verified here" notes and §7.3. Python line numbers
     come from `tools/dis.py` (`co_lnotab`), never from a decompiler; DLL
     offsets are file offsets in `/client/bin64/_destiny.dll` where stated.
     Marks: **[V]** read off this build; **[V2]** a third party's decode of a
     different build; **[I]** inferred; **[A]** a shipped assumption. -->

# Porting from EvEJS: the surprises, and the one thing they have in common

**Audience: the next agent about to port an EvEJS subsystem.** You may have
read nothing else. This page is what will bite you, how you will recognise it,
and what to do instead. The individual surprises are the evidence; **the
pattern in §5 is the deliverable.**

EvEJS is a third-party EVE server emulator this project reads for facts only.
(From 2026-10-20 to 2026-11-03 the tree was AGPL-3.0-only so it could take
source; it is now under `LICENSE.md` and holds no EvEJS code.) It
has been worth weeks and it has been wrong in ways that cost real time. Both
halves are here, because the question you actually have is not *"is this source
good?"* — it is **"which of its claims can I take on trust?"**

Contents: 0 the rule · 1 the build delta, and why it is not where you think ·
2 where it was wrong · 3 where it was right · 4 where **we** were wrong ·
5 the classification · 6 the checklist · 7 what rests on what.

---

## 0. The rule, before anything else

> **Read EvEJS for structure. Never ship a value you got from it.**
>
> *Structure* — what the parts are, what order operations go in, which fields a
> record has and in what sequence, that a problem exists at all — transferred
> across a 223,669-build gap at a rate this project did not expect. *Values* —
> an enum number, a cap, a tax, a service name, a dict key, a semantic name for
> a reused slot — did not, and the failures are silent.
>
> The discriminator is not "wire versus data". It is **derived versus
> authored**: did they read it off an artefact that still exists, or did they
> have to invent it because nothing answered the question? Their tree cannot
> tell you which, because it has no tests and no `TODO` anywhere (§5.2). You
> have to ask it yourself, and the question is always *"which file must they
> have read, and can I read it?"*

Running score on claims checked against our client: **eighteen disagreements
with EvEJS, eighteen decided our way** (`docs/port-tracker.md` §15.3, §18.5).
Read §4 before you conclude anything flattering from that number.

---

## 1. The build delta, and why it is not where you think

**EvEJS targets client build 3396210. Ours is 3619879.** Same version line
(V24.01), **223,669 builds apart**. Every build named anywhere in their archive
— 3294658, 3396210, 3436472, 3475087, 3487903, 3503375 — is *older* than ours,
so relative to us the risk is stale content, never post-24.01 content
(`docs/re/evejs-crosscheck.md` §6, `docs/re/evejs-data.md` §6).

That one fact is why this project's evidence convention has a separate mark for
EvEJS at all:

| mark | means |
|---|---|
| **[V]** | verified against **our** client: a named `.pyc` with real `co_lnotab` lines, or a `bin64/*.dll` address |
| **[V2]** | a third party's decode of a **different** build |
| **[I]** | inferred, saying from what |
| **[A]** | an assumption we chose to ship, saying what would falsify it |

`[V2]`'s definition was written deliberately narrow when it was coined
(`0764174`, the `dungeon-evidence` lane): *another decoder, working from other
bytes, produced the same answer.* It is **only ever applied to names — never to
an offset, size, type, unit or meaning.** Hold that line. The same lane found
why: of ten `guardCommand` field names corroborated at build 3475087, the
eleventh — `jitterRadius` — appears **zero times in 4.7 MB** of their data
while our record has it. **The field set is not constant across builds**, so
even a perfect name match says nothing about the *set*.

### 1.1 The surprise inside the surprise: the wire did not move

The expected shape of the risk was "build-sensitive wire layouts". **It is
not.** On 2026-10-22 the `evejs-probe` lane logged a client of our own writing
into a containerised EvEJS and recorded both directions
(`docs/lanes/evejs-probe.md` §6):

> framing — identical. The whole `0x01`–`0x2F` marshal opcode table —
> identical. The H1–H10 handshake sequence — identical. `boot_build` 3396210 is
> *accepted* by a 3619879 client, because the client refuses only a **newer**
> server.

Zero unanswered requests across 31 character-select calls and 45 in-game calls.
**The format did not move in 223k builds.** That makes their agreement on
framing and marshal evidence of *stability*, which is a more useful property
than same-build agreement would have been.

What the delta moved is **everything looked up by name**, and **everything they
had to author themselves**. Those are §2's two clusters, and they are the whole
of the delta's practical footprint.

### 1.2 A `[V2]` nobody promoted is a liability with a citation on it

A citation makes a number *auditable*, not *true*. These are the `[V2]`s still
load-bearing in shipped code today, and each is a thing to settle rather than
to inherit:

| what | where | status |
|---|---|---|
| `STANDING_TAX_ZERO_POINT = 6.67` | `reprocess.rs:365` | **[I]** + **[V2]**; the client ships a per-station base take and **no standing curve** at all. Needs a capture of a real `GetQuotes` at known standing |
| `NPC_STATION_TAX = 0.0` (industry) | `industry.rs:507` | **[I]** + **[V2]**; no client table. Visible in the job tooltip before it breaks |
| `GOTO_DISTANCE = 1e16` | `space_state.rs:71` | **[V2]** magnitude, but argued independently ("far enough the ship never arrives"), so low-risk by construction rather than by evidence |
| `MISSILE`, `FORMATION`, `MUSHROOM` mode tails | `destiny.rs:126` | **[V2]** only, and **deliberately not encoded**: a wrong first field corrupts every record after it. The right refusal |
| `INDUSTRY_SECURITY_TAX_RATE = 0.05` | `industry.rs:484` | ~~**[V2]**~~ — **this is wrong and §7.1 corrects it.** The constant is in our client and the value is right |

`destiny.rs:126` is the model answer: a `[V2]` layout that would fail silently
is not encoded at all, and the comment says what would settle it.

---

## 2. Where EvEJS was wrong, and what each one cost

Ordered by how much the shape of the mistake will help you.

### 2.1 A required key their server omits — and it is why this is not a source port

Their `CryptoHandshakeAck` omits `access_token` and `computer_hash`. Our client
reaches character selection and dies with `KeyError: 'access_token'` at
`connectionService.py(175) LoginSso`.

**Cost: nothing to us, because we were right first** —
`docs/reference/login-path.md` §H10 had both keys documented as required and indexed
directly before this project had heard of EvEJS. What it cost was *their
server*, inside ten minutes of first contact (`7c8a478`), and that failure is
the entire origin of the programme's governing decision: **capture the
contract, not the code** (`docs/port-tracker.md` §4). A ported subsystem
carries its build's required-key assumptions invisibly.

**The shape to remember:** a server omitting a key is indistinguishable from a
key being optional, right up until a different build indexes it directly.

### 2.2 `DstBallMode` 9 and 10 are swapped, and the error parses cleanly

Their constants table has `MINIBALL 9, FIELD 10`. Ours has **`FIELD 9,
MINIBALL 10`**.

**Re-verified in this lane**, independently of `docs/re/destiny-ball-crosscheck.md`
§2.3: the thirteen `DSTBALL_*` name/doc pairs in `_destiny.dll` run in table
order from file offset `0xa2cc8`, and `DSTBALL_FIELD` is at `0xa2ea0` with
`DSTBALL_MINIBALL` at `0xa2ec8` — FIELD ninth, MINIBALL tenth. **[V]**

Note what agreed and what did not: both ends of their table are right
(`RIGID 11`, `FORMATION 12`; the mode bound is `cmp eax, 0xc`), and only the
middle differs. **Two agreeing endpoints are not evidence about the interior.**

**Cost: avoided, and the tracker sharpened why it mattered** (§10.3): `FIELD`
is a supported free mode in their encoder with **no tail case**, so a
force-field ball writes byte 10 where our client wants 9 — it **parses cleanly
and renders wrong**, and their own `[0,12]` range clamp cannot catch it.

### 2.3 A call that does not exist in our build — a whole protocol generation moved

Their `machoNet.GetInitVals` returns a ~145-entry `serviceInfo` dict, and one
of our own inventory lanes relayed that it *"gates every other lane"*.

**Re-verified in this lane: `GetInitVals` has zero hits across all 12,774
`.pyc` files**, with `GetServiceInfo` as the positive control (it hits
`carbon/common/script/net/machoNet.pyc` and three UI controllers). **[V]** The
real call is `machoNet.GetServiceInfo`, and I read its call site:
`machoNet.pyc` line **2028** is
`self.serviceInfo = self.RemoteServiceCall(session, self.myProxyNodeID,
'machoNet', 'GetServiceInfo')`, with the only `SETUP_EXCEPT` in the enclosing
`ConnectToServer` at bytecode offset 385 (line 1882) — far before it. **So the
call is mandatory (an exception there aborts login) and its content is free**;
`{}` is both sufficient and better than the `util.CachedObject` a real server
returns. **[V]**

**Cost: one false cross-lane dependency, retired by one grep**
(`docs/port-tracker.md` §10.1). The lesson the cross-check drew is the single
cheapest check in this document: **before taking any call-level claim, grep
this build for the call name. One command, and it is the only check that
catches a whole protocol generation having moved.**

### 2.4 The near-miss: six of seven fields right, and the seventh kills the feature

`docs/re/destiny-ball-crosscheck.md` §4.1 took the WARP mode tail from their
encoder: sizes right, order right, **last field named `warpOwnerID`**. The
`warp-destiny` lane read the tail out of our own `_destiny.dll` and it is
`warpFactor` — an int64 in milli-AU/s, proved by `cvtsi2sd` on `ball+0x98` and
by the engine's own log format string naming `warpFactor` (`docs/re/warp.md`
§4.2, §10 item 2). **[V]**

**This is the most expensive shape in the document.** `destiny::warp_factor`
clamps to at least 1000 because the native clamps `<= 0` to `1` — 0.001 AU/s,
four hours to cross a system, *indistinguishable from a server that has stopped
answering* (`docs/port-tracker.md` §30.1). An owner id written into that slot is
an enormous integer that still parses.

**Cost: two rounds in which warp was correctly refused**, which was the right
call — and the refusal was lifted only when the field became `[V]`. Six of
seven fields right is not 86% right; it is one silent catastrophic field.

### 2.5 A misreading whose conclusion is right by accident

Their handshake sends the verification flag as `false`, and the handoff
document recorded that as *"the verification flag, hardcoded `false` …
verification off"*.

**Re-verified in this lane.** `evecrypto/placebo.pyc`'s `verify` body (line 22)
is exactly `(signedData[0], signedData[1] == 0)` — `BINARY_SUBSCR`,
`BINARY_SUBSCR`, `LOAD_CONST 0`, `COMPARE_OP`, `BUILD_TUPLE`. And in
`carbon/common/script/net/GPS.pyc`'s `__Execute` (def line 1009), the
`RAISE_VARARGS` for `RuntimeError('Failed to verify initial function blobs')`
at line 1012 sits at **bytecode offset 36**, while the first `SETUP_EXCEPT` in
the function is at **offset 105**. **[V]**

So there is no "off". The element is a **required `0`-equal value**. `false`
works only because `False == 0` in Python 2; `True`, `1`, `None` or `''` all
kill the handshake with an unexplainable `GPSTransportClosed`.

**Cost: zero, because `docs/reference/login-path.md` §4 H8 already said to send `0`.**
The entry is here for its shape: **a conclusion that happens to hold is the
most dangerous kind of wrong**, because the thing that would have caught it —
the implementation working — is exactly what suppresses the check. The obvious
next move from "verification is off" is to send `True`.

(The same pass also found the cheapest possible answer: `marshal.loads` is the
**stdlib** marshal, not `blue.marshal`, so `signedFunc = ("N", 0)` — one byte —
satisfies the client and emits no executable code to it at all.)

### 2.6 An invented constant wearing a plausible round number

Their frame cap is `payloadLength > 0 && <= 10_000_000`.

**Re-verified in this lane**: `carbon/common/script/net/SocketGPS.pyc` line 133
installs `MaxPacketSize = 10485760`. **[V]** The client's four real numbers are
1,000,000 (carbonio default), 1,048,576 (legacy default), **10,485,760** (what
`SocketGPS` actually installs, on both `Connect` and `Accept`) and 5,000,000
(the `machoNet.dropPacketThreshold` pref default). **10,000,000 is none of
them.**

It is a *safe* invention — above anything the client sends, below what it
refuses — and that is precisely the problem: **an invented constant that works
looks exactly like a decoded one.** A suspiciously round number in a
third-party server is a tell.

**Cost: zero, and one real gain** — chasing it established that our
`MAX_FRAME = 10_485_760` is the client's own number and that carbonio compares
`plen + 4` while legacy compares `plen`, which is why a payload ceiling of
`MAX_FRAME - 4` is exactly right rather than conservative.

### 2.7 A field that does not exist in our build at all

They send `isLootLogging` on the fleet init state. **Re-verified in this lane:
zero hits across the corpus.** **[V]**

**Carry the grep trap with this entry**, because the finding depends on it:
`grep` on this machine is a ugrep wrapper that always passes `-I`, so a
recursive grep **silently skips `.pyc` files**. Use `/usr/bin/grep`, always
with a positive control — *"returned nothing"* and *"cannot see this file
type"* are the same output, and a false negative here would have looked
identical to the finding (`docs/port-tracker.md` §15.3).

### 2.8 Their bugs are not build deltas, and their code is not a test oracle

Their server is not merely aimed at another build; it has ordinary defects, and
a port inherits them silently. From the inventory (`docs/port-tracker.md`
§10.7, §15.3):

- **`GetPlanetsForChar` as a `CRowset`** raises `AttributeError` on **every
  player's first colony** — `planetSvc._UpdateColonyPresence` calls `.append`
  on it and `dbutil.CRowset` has none.
- **`refreshAffiliationSessions` silently drops every role change**, so after a
  corp is created the server holds the new CEO's director mask and the client is
  never told.
- **Market fills are price-then-station, not price-then-time.**
- **Mining mints ore outside its own custody boundary** and then decrements the
  asteroid — a real duplication window.
- **`setCorporationAlliance` returns early on first failure after partial
  writes.**

Each is cheap to get right while porting and expensive to discover later. Read
their decomposition; do not assume their *behaviour* is the specification.

### 2.9 Their data contradicts its own header, so count before you quote

Their 4.7 MB dungeon-geometry file reports 26 templates / 103 rooms / 4,097
objects in its own `counts` block. The records actually present are **26
templates, 29 rooms, 1,169 objects**, and the per-template audits *inside the
same file* agree with the lower numbers (`0764174`).

**Cost: a wrong sample size reported to the owner, corrected in the tracker.**
**A file that disagrees with its own header is a reason to count anything you
intend to quote from that tree.**

### 2.10 Service counts: three populations, none of them the gap

This one is a measurement trap rather than a build delta, and it has misled in
both directions.

- **206** service entry points from the archive's file listing. **I reproduced
  this exactly in this lane** from the zip's central directory (`unzip -l`, no
  extraction): 206 distinct `server/src/services/**/*Service.js` basenames.
- **143** — what `machoNet.GetServiceInfo` *actually answers on the live wire*,
  every value `None` (`docs/lanes/evejs-probe.md` §5.1).
- **175** — names our client statically mentions at a `RemoteSvc`-family call
  site; **300** — client-side service classes. Three different populations, no
  two comparable (`docs/re/evejs-crosscheck.md` §8).

And the "135 services we lack" figure is **not a work estimate in either
direction** (`docs/port-tracker.md` §10.5): their **16 `structure*` services
are one feature** (13 of 16 require one shared state module, one table, one
~40-field record), while **4 corp service names hide 132 handlers and 19,000
lines**. Service names overstate *and* understate by about 2× each way.

**The live count is the only one that means anything.** Our 87 against their
143 is the real surface gap.

---

## 3. Where EvEJS was right — including where we doubted it

A document that only catalogued their errors would be unfair and, worse, less
useful. These are the entries that answer *"when do I trust it?"*

### 3.1 The asteroid table: data that cannot be obtained any other way

Their own database builder emits an **empty** `asteroidTypesBySolarSystemID`
table with a comment saying it was *"Generated without client private asteroid
map"*. The real table cannot be built from CCP's published SDE at all.

We have it, because we read the player's client at runtime. **Re-verified in
this lane**: `res:/staticdata/asteroidtypesbysolarsystemid.pickle` is in our
client's `resfileindex.txt`, 96,712 bytes — 3,974 systems, 24,071 assignments.
**[V]**

**This is the vindication of the whole architecture** (`docs/port-tracker.md`
§7): *read-the-client is not only the legal position, it is the one that yields
more data.* Their tooling is the clearest possible evidence for it, because
they documented the gap themselves.

The same pattern recurs across their static corpus (`docs/re/evejs-data.md`
§7): their `skillTrainingAlphaCaps` was scraped from a wiki while the client
ships `res:/staticdata/clonegrades.static`; their mission export nulls out
`missionRewards` on 1,883 of 2,878 rows while our `missions.fsdbinary` has
every one of those as a named, typed, offset-resolved column. **We can read
more of their own mission table than their shipped export contains.**

### 3.2 Eleven field names for a C++ serialiser, and one correction of ours

`docs/archive/destiny-and-the-ballpark.md` §4.4 read the Destiny ball record out of
`_destiny.dll` and marked **eleven fields `[I]`** — sizes and order from the
disassembly, semantics a best guess. EvEJS has to feed the same serialiser, so
it contains an independent reading of the same wall.

**Nine of eleven got a name, and every default value matched an init value we
had read independently out of the DLL** — `-1, -1, -1, 1.0, 0`
(`docs/re/destiny-ball-crosscheck.md` §1). Two readings of one C++ serialiser
agreeing on five arbitrary sentinels is not coincidence.

**And one of the two disagreements went their way.** We had `ball+0x08` as
`followId`; theirs says `harmonic`. They are right — **and our own disassembly
already contained the proof**: the `followId` fixup pass is gated on mode
bitmask `0x1032` = {FOLLOW, ORBIT, MISSILE, FORMATION}, and a field in the
*common header* would need no mode gate. **We had the evidence and drew the
wrong conclusion from it.**

What this cost them nothing and bought us everything: *"what we needed was
never the code; it was the names of eleven fields"*, and a name learned from a
second reading is a fact about CCP's serialiser, not an expression we took.

### 3.3 Five of nine mode tails, later confirmed byte-exact

Round 0.2 dumped reference blobs from our own client's writer and measured the
mode tails directly. **Five of the nine `[V2]` rows became `[V]`, and all five
agree with EvEJS exactly** — field order, widths, and the semantics that
`followID` is the *target's* id and not the subject's
(`docs/reference/destiny-reference-blob.md` §9.7). `STOP` writes nothing; `GOTO` is three
`f64`; `FOLLOW` and `ORBIT` are `i64 + f32`; `TROLL` is one `u32` effect stamp.

That is the strongest single reason to take their remaining four rows
seriously — **and still not a reason to call them `[V]`** (§1.2).

### 3.4 Reading their codec exposed a latent bug in ours

Their payload header is `0x7E`, and chasing *why* closed two of our
long-standing gaps and found a defect. `0x7D` is not a second magic: it is the
marshal **version**, and a non-zero version means **there is no mapcount word**
(`docs/re/evejs-crosscheck.md` §3, from `blue.marshal.Save`'s own signature and
both the reader and the writer). Our `eve-marshal` read the mapcount
*unconditionally* — for a `7D 01` stream it would swallow four body bytes and
mis-decode everything after.

Latent, not live — this client never writes version 1, and the probe confirmed
EvEJS never does either — but it is **the one place in the archive that
corroborated a correction of ours** (the shared-id table at the tail) rather
than a claim of ours.

### 3.5 Decompositions, and three that changed our plan

This is the category to actually read them for, and it is the one with no
licence question attached: module boundaries and operation ordering are facts
about the problem.

- **Fleets.** Not on the original Round 1 list and it should have been: 5,278
  lines, **zero persistence**, two call sites into the space runtime, nothing
  needed from corps, and it serves the co-op end goal directly. Moved into
  Round 1 on that basis; it shipped as 33 arms and 74 tests and was proven
  across two real clients (`docs/port-tracker.md` §10.6, §15).
- **`space/destiny/**` is a payload library, not the behaviour.** 42k lines of
  pure functions; the orchestration lives in the scene runtime — *their own
  README says so*. **Scoping a lane as "port destiny/" buys payload builders
  and zero behaviour**, and our Round 2 plan had walked straight into it
  (§10.2).
- **The item location model.** A location is the triple
  `(ownerID, locationID, flagID)` and nothing else; fourteen declared "custody
  ref kinds" all collapse to it, and fitted modules are not a table but items
  whose flag falls in a fitting range. Adopted. **Their compensating machinery
  was explicitly not adopted** — a durable receipt ledger inside the items
  table, a whole-table sync fence, and per-domain sagas, all of which exist
  because they have no transactions. A staged all-or-nothing edit plan in Rust
  gives that for free (§5, Round 0.5).

Also in this category: **a 39-line file of theirs is a written post-mortem of
exactly the bug we had** — `sessionIdentity.js` names a default-id fallback,
explains that it conflates account id with character id, and states the rule
(*return 0, never a default*). It was read before Round 0.3's identity split
was written.

### 3.6 Negative findings, which are the cheapest kind

`space/npc/npcLoot.js` exists, and the client ships **no loot tables** — eight
fields across five tables reference a loot table with no table of loot tables
to point at (`docs/re/dungeon-model.md`). Their module can only be invented or
ESI-derived.

**That strengthens our negative rather than weakening it**, and it converts
loot from an RE task into a **design** task. It is the one subsystem on the
scope map where "go look at the client" is not the answer, and knowing that
early is worth a round.

### 3.7 A running server beats its own source

Their GM console is **176 commands** behind **one bit** of gating — 172 of them
ungated — and it exercises a subsystem end to end from one line in a chat
window (`docs/port-tracker.md` §10.9). That inverted an ordering: **the GM
console's breadth is a prerequisite *for* testing**, so it comes early. (Two
interface ideas were taken straight: their `/addisk` + `/setisk` pair — relative
and absolute mutators, separately — is better than the single setter we would
have written.)

And driving it produced the one thing reading could not: a **surface we had
never seen**. `OnServerBrainUpdated` is **197,801 bytes** of dogma
`BrainEffect` state, authored by the server, in no shipped file, arriving
unsolicited at login. Measured in the same run and not derivable from the
client alone: the session role mask is OR-ed between H10 and the pushed initial
state (`role |= ROLE_LOGIN | ROLE_PLAYER`), and notification payloads are
pickles nested two deep.

**What a server sends is validated against the build you care about. Its source
is not.**

---

## 4. Where *we* were wrong — the honest half of the same lesson

If the lesson were only "verify the third party", the 18–0 score would mean we
are better readers. It does not. In the same period our own documents were
wrong at least this often, and three were corrected this week alone.

**4.1 A claim that shipped a player-visible defect.** `docs/re/warp.md` §5.1
said a warping ship *"accelerates to 75% of max speed on its own with no
`SetSpeedFraction` from us"*. The align integrator is **throttle-scaled**: a
ball with `speedFraction == 0` is commanded to zero, never passes either align
gate, and warps only when the client's own 180-tick watchdog force-activates
it. A capsule with a published align time of **0.089 s** aligned for *minutes*
in front of the owner (`docs/port-tracker.md` §34). **The fix is one extra
action in the batch**, and the reason no test caught it is worth more than the
fix: **every warp test asserted the `WarpTo` arguments, and the arguments were
correct. The defect lived in what the batch did not also contain** — the test
helper had to be generalised from "find the warp" to "find any action by name"
before the property could even be stated.

**4.2 Three rows of our own disassembly table, wrong by inference.**
`docs/archive/destiny-and-the-ballpark.md` §4.4 had `int32, f32, int32` where the truth
is `f32 maxVelocity, f32 inertia = 1, f32 speedFraction = 0`. The sizes were
never wrong; the types and one name were — and the reasoning was backwards in a
way worth naming: *"init `0x3f800000` = 1.0, a fraction, probably
`speedFraction`"*. **A speed fraction defaults to 0; inertia defaults to 1.**

**4.3 A call that does not exist, in our own plan.** `docs/archive/plan-client-ci.md`
told scenarios to call `blue.os.Quit()`. **Re-verified in this lane: there is
no `blue.os.Quit` in this build.** Every `Quit` string in the corpus belongs to
something else — `DoQuitGame`, `CmdQuitGame`, `AskQuitGame`,
`agentMissionStateQuit`, `QuitAlliance`, `site.pyc`'s stdlib `Quitter`,
trinity's `PostQuitMessage` — and `bluepy.Terminate` is present, which is the
route the client's own quit button takes. **[V]** on both halves; the chain on
to `blue.os.Terminate` is `docs/archive/plan-client-ci.md`'s own reading. **We made
exactly the §2.3 mistake, in our own direction.**

**4.4 "There is no native `WarpTo` entry point to probe."** Wrong, and for a
mechanical reason to keep: **the Ballpark method table is built at runtime by
`lea`**, so a search for pointers to the name string finds nothing. A probe
concluded absence from a search that could not have found presence.

**4.5 "No tick path changes a ball's mode."** False, and believed for an hour,
because `StopBall` reaches `SetMode` by a **tail `jmp`** rather than a `call` —
an `e8`-only caller enumeration misses it, and a whole section of
`docs/re/warp.md` depended on not missing it.

**4.6 A design problem that dissolved.** `docs/port-tracker.md` §30.4 declared
that a delayed Destiny push needed *"a stamp allocator that can mint a future
stamp without moving the floor"* — a design, not a patch. Both horns rested on
one unexamined premise: **that the stamp is chosen when the batch is built.**
It need not be. `Push::restamp` binds the stamp as the batch *leaves*. **No
allocator, no future arithmetic, one trait method** (§36.2). Three features had
been blocked on it.

**4.7 A conflict between two of our own lanes that never existed.** One lane
read `OnSpecialFX` as nine arguments, another as fourteen. **They were the same
reading**: the method is `argcount 15` with **nine required**. One lane counted
parameters, the other counted requirements. **Cost: one round of invisible
warps.** The check was two commands — `argcount` from the code object and the
default count from the `MAKE_FUNCTION` operand. *It was never expensive; it was
just never done.*

**4.8 A search-space error, and EvEJS corroborated it.** `reprocess.rs`
asserted in two doc comments that Crius ended per-station refining variation.
The reasoning was sound except for one step — *"the strings appear in no `.pyc`,
therefore the client has no table"* — and `mapObjects.db` is **SQLite**, which
no bytecode grep can see inside. 561 of 5,210 stations refine below 0.5 and 119
take 2.5%. **EvEJS hardcodes the same universal**, so this is not a
disagreement; it is the two of us making the same mistake, and their `[V2]`
*corroborated our error*. The marks were kept and downgraded in meaning: they
corroborate the **fallback**, not the claim (`docs/lanes/reprocess.md` §11.3).

**4.9 A scope document that drifted in both directions.** Four of the twelve
rows corrected in the 2026-10-21 refresh of `docs/evejs/evejs-subsystem-gap.md` were
**measurement errors in the original pass**, not changes in the tree: the "our
state" column had been written from memory of what had been built, and it
over-credited the market and under-credited bookmarks and factional warfare.
The fix is in the method — re-derive from `service/*.rs`, arm by arm. Related:
`jail-verify.sh`'s *"reached 10.137.0.1:26000"* cannot tell our server from
anything else bound to that port, and it passed green against a 22-hour-old
leftover forwarder.

### 4.10 What the 18–0 score actually measures

**It measures the procedure, not the two projects.** Every EvEJS claim this
project uses passes through a mandatory check against our client. Our own
claims get checked when a feature fails in front of a human, or when someone
re-reads a document on purpose. §4 is what happens when somebody does.

Take the number as evidence that **"check it against our client" works**, and
for nothing else.

---

## 5. The classification: which kinds of claim are safe

By category, from the evidence above rather than from plausibility.

### 5.1 The table

| kind of claim | transfers? | why | what to do |
|---|---|---|---|
| **Framing, marshal opcode space, tag semantics, dict pair order, header layout** | **yes** | compiled into `blue.dll`; unchanged across 223k builds and confirmed live (§1.1) | read it from our binary anyway; their agreement is evidence of *stability*, which is the property you want |
| **Handshake sequence and shapes** | **yes** | H1–H10 identical on the wire | same |
| **Serialised record field order, sizes, defaults** | **mostly** | 9/11 names, 5/5 tails, 5 arbitrary sentinels matching | take it as the hypothesis that makes the real experiment cheap (§3.2); never encode on it (§1.2) |
| **Decomposition, module boundaries, operation ordering, "this problem exists"** | **best of all** | facts about the problem, not about a build | this is the reason to read them. Read freely, cite the client |
| **Semantic name for a reused slot** | **coin flip** | they fixed our `harmonic`; they broke `warpFactor` | a name is a hypothesis in *both* directions. If a wrong value fails silently, do not ship it |
| **Anything looked up by name at runtime** — service names, call names, dict keys, session keys | **no** | a whole protocol generation moved (§2.3), and fields vanish (§2.7) | `/usr/bin/grep` this build for the literal, with a positive control. One command |
| **Enum values in the interior of a table** | **no** | §2.2; agreeing endpoints say nothing about the middle | read the table out of our own binary |
| **Required-key sets** | **no** | §2.1 | for every key the client indexes, confirm required-ness in our bytecode. This is the step that must never be dropped |
| **Caps, thresholds, timings** | **no** | §2.6; a safe invention is indistinguishable from a decode | find the client's own number; there are usually several and you need the one that is installed |
| **Game data the client ships** | **do not port — decode** | our build is newer than every build in their archive; their exports are lossy (§3.1) | `tools/fsd.py`, `tools/pyd_schema.py`, `resfileindex.txt` |
| **Game data the client does *not* ship** | **[V2] is the best available answer** | loot, standing curves, station tax curves, order books | mark it, state the falsifier, and make any fallback **fatal** rather than silent |
| **Their own behaviour as a specification** | **no** | §2.8 — ordinary bugs, 15 tests in the whole release | their code is a second opinion, never an oracle |
| **Counts in their own data and docs** | **no** | §2.9, §2.10 | count it yourself, and say which population you counted |

### 5.2 The one mechanism underneath all of it

**EvEJS cannot mark its own uncertainty, and that is structural rather than
sloppy.**

A server must answer every call, so it must hold a value for every field —
including every field nobody could decode. Their release ships **no test
directory and no `*.test.js` anywhere** (the only tests in it are 15 Rust
`#[test]`s in the market daemon), and `TODO`/`FIXME`/`NOT_IMPLEMENTED` over
150k lines of space and 106k of corp/sov returns **zero**
(`docs/port-tracker.md` §10.8). **There is no textual signal separating
finished code from aspirational code, or a decode from a guess.** A number they
read out of a DLL and a number they chose because something had to go there are
indistinguishable in their source, and both work.

So you cannot grade their claims by reading them. You grade them by asking the
only question that discriminates:

> **Which artefact must they have read to know this — and can I read it?**

- If the artefact is our client's bytecode, a DLL, or a `res:/staticdata/`
  file: **read it yourself.** Their answer becomes corroboration, and it is
  worth having, because agreement across a 223k-build gap is evidence the thing
  is *stable*.
- If no artefact answers the question: **they invented it, and so must we.** That
  is the only place `[V2]` belongs, and it needs a falsifier attached.

And the rule from §15.1 of the tracker, which is **rule zero of a port** and
sits ahead of reading their code at all:

> **Check `out/pyc/eve/common/` first.** Our client ships CCP's own shared
> client/server code — 139 modules across `dogma`, `net`, `universe`,
> `pathfinder`, `planet`, `search`, `sys`, `util` and more. **Re-verified in
> this lane**:
> `eve/common/script/planet/baseColony.pyc`'s `GetTypeAttribute` (def line
> 1312) raises `NotImplementedError('GetTypeAttribute must be specifically
> implemented on the client and server')` at line 1325 **[V]** — which is the
> proof it is shared code and not a client-side mirror. So their 11,586-line
> colony runtime is a *reimplementation of something we already had*. Also
> there: `eve/common/script/sys/eveSessions.pyc`, carrying the real server's own
> `SessionMgr.GetSessionValuesFromRowset` (line 482) **[V]**.
>
> **For any subsystem with a presence there, the client contains the
> specification and EvEJS is a third party's reading of it.** Three of our four
> landed subsystems were ported from CCP's own code, not from theirs.

The session-key question is the cleanest worked example of rule zero beating a
third party. EvEJS sends `corprole` and the four `rolesAt*` keys on every
session change — **measured on the live wire**, 23 attributes in its
`SessionChangeNotification` — and an inventory lane recommended we match it.
**Re-verified in this lane:** `carbon/common/script/sys/basesession.pyc`'s
`RecalculateDependantAttributesWithChanges` (line 2183) derives `locationid`
from `stationid or solarsystemid` itself, and recomputes `corprole` from the
tuple `('hqID', 'baseID', 'rolesAtAll', 'rolesAtHQ', 'rolesAtBase',
'rolesAtOther', 'stationid', 'solarsystemid')`. **[V]** Sending them is one of
four quiet failure modes. The client contained the server's own answer.

---

## 6. The checklist, for the next EvEJS subsystem

1. **`out/pyc/eve/common/` and `out/pyc/industry/` first**, before their source.
   If CCP's shared code is there, that is your specification and their module is
   a second opinion.
2. **Grep this build for every call name, service name and dict key** you intend
   to use. `/usr/bin/grep -r`, **with a positive control.** One command; it is
   the only check that catches a protocol generation having moved.
3. **For every key the client indexes, confirm required-ness in our own
   bytecode.** Their server omits required keys. This is the step that separates
   this programme from a source port.
4. **Never encode a `[V2]` layout** if a wrong field would be silent. Refuse the
   feature and write down the experiment that settles it — `destiny.rs:126` is
   the model.
5. **Treat every constant as invented until you find the artefact.** If you find
   it, cite it and promote the mark. If you cannot, mark `[V2]`/`[A]`, state the
   falsifier, and make any fallback **fatal** rather than silent — a quiet
   fallback to a stale id is how a build delta hides.
6. **Suspect agreement after a run of agreements.** Of 31 claims cross-checked,
   19 agreed — and *both* of the two most interesting disagreements were found
   only because someone kept checking into a good streak. A high agreement rate
   is the condition under which the next claim gets waved through.
7. **Count anything you quote from their tree**, including from their own
   headers and `counts` blocks.
8. **Prefer a capture to their source.** A running EvEJS, or our own
   `eve-probe`, tells you what a server sent *and what a client accepted*; that
   outranks any reading. Their 176-command GM console drives a subsystem end to
   end from one line.
9. **Check what your batch does not contain**, not only what it does. §4.1 was
   invisible to every test that stopped at the first matching entry. And cite
   `tools/dis.py` only — decompiler line numbers are its own reconstruction and
   have been off by up to 720 lines.
10. **Write down the negative results and the reverts**, with the premise that
    turned out to be wrong. §4.6 is a design problem that dissolved when one
    unexamined premise was dropped, and the record of *why it stopped* is what
    made that visible.
11. **(History: the AGPL period, ended 2026-11-03; no EvEJS code is in the
    tree now.)** Any
    file carrying ported EvEJS logic named it in its header, said which module,
    and said AGPL-3.0-only; *"rewrote it from their logic"* counts. That header
    is also the marker telling a future reader which assumptions came from build
    3396210 — the more useful reason. And ~505 MB of their archive is
    CCP-derived static data: **the AGPL has no bearing on any of it.** We read
    the player's own client at runtime and ship neither.

---

## 7. What this document rests on, and what it could not settle

### 7.1 One correction found while writing this

**`industry.rs:484-486` and `docs/lanes/industry.md` are wrong about where
`INDUSTRY_SECURITY_TAX_RATE` comes from.** Both say
*"`security.common.const` is **not** in the client bundle, so 0.05 is [V2] from
them"*.

`out/pyc/security/common/const.pyc` **is in the bundle**, and its module body
reads **[V]**:

```text
line 3   MARKET_SECURITY_TAX_RATE        = 0.05
line 5   NPC_BOUNTY_SECURITY_TAX_RATE    = 0.2
line 7   INDUSTRY_SECURITY_TAX_RATE      = 0.05
line 9   AGENT_MISSION_SECURITY_TAX_RATE = 0.2
```

The **value is right**; the **provenance claim is wrong**. `0.05` is promotable
from `[V2]` to `[V]` with the citation `out/pyc/security/common/const.pyc:7`,
and the three sibling rates come free. Not my files — reported rather than
fixed.

It is a small correction and exactly the shape §5.2 predicts: the thing that
went wrong was not the number, it was *the belief that no artefact existed*.
One grep settled it.

### 7.2 A trivial citation drift, recorded so it does not propagate

`docs/re/warp.md` §0 item 5 cites
`eve/common/script/util/eveCommonUtils.py:446` for
`AUPerSecondToDestinyWarpSpeed`. Under `tools/dis.py` the `def` is at line
**443** and the body at **447**. The claim is correct —
`int(auPerSecond / const.warpSpeedToAUPerSecond)` **[V]** — and only the line
number is off by one.

### 7.3 What was re-verified in this lane, and what was not

**Re-verified against build 3619879 here** rather than taken from our own
documents: `GetInitVals`' absence and `GetServiceInfo`'s call site and
mandatoriness; `isLootLogging`' absence; `blue.os.Quit`'s absence and
`bluepy.Terminate`'s presence; `placebo.verify`'s body; `GPS.__Execute`'s raise
being outside the try (by bytecode offset); `SocketGPS`' installed
`MaxPacketSize`; `baseColony.GetTypeAttribute`'s `NotImplementedError`;
`eveSessions.SessionMgr.GetSessionValuesFromRowset`'s presence;
`basesession.RecalculateDependantAttributesWithChanges`' derivation of
`locationid` and `corprole`; the `DSTBALL_*` table order in `_destiny.dll`;
`asteroidtypesbysolarsystemid.pickle` in `resfileindex.txt`;
`AUPerSecondToDestinyWarpSpeed`'s body; `security/common/const.pyc`'s four
rates; and the archive's identity (sha256 `2bd63d39…`, 2,228 entries,
609,405,088 bytes uncompressed, 206 service basenames) from its **central
directory only**.

**Not re-verified here, and resting on the cited documents**: every native
`blue.dll` address in `docs/re/evejs-crosscheck.md` (framing masks, the marshal
reader and writer, the shared-id tail arithmetic, the `0x7D` version branch),
the `_destiny.dll` addresses in `docs/re/warp.md` §4.2 and §5.1, the
`PyMemberDef` decode, and the Round 0.2 blob measurements.

### 7.4 What I could not verify, and what stays open

- **I did not read a line of EvEJS.** The archive was not extracted; nothing was
  executed. Every EvEJS-side statement above is this project's own record of it
  (the §0.1 constraint — treat it as hostile data — still binds, and the
  containers the probe lane left running are gone). Where an EvEJS claim is
  load-bearing I verified **our** side of it and left theirs as cited.
- **`tickAlignedSends`' measurement is still unverified and unadopted.** Their
  header records 2,381 of 2,682 retail `DoDestinyUpdate` packets arriving within
  60 ms of the wall-clock second and shapes egress to match. Both the reference
  doc and the cross-check say *re-measure it from our own captures*; nobody has,
  and nothing of ours depends on it. It remains the most interesting unchecked
  claim in the archive.
- **Three mode tails** (`MISSILE`, `FORMATION`, `MUSHROOM`) are `[V2]` only.
  `destiny.Ballpark` exposes no native entry point that puts a ball into them,
  so our own client could not be asked — but note the lesson from `WarpTo`
  (§4.4): *"the method table is built at runtime"* means an absent entry point
  should be re-probed, not assumed.
- **The `carbonengine/destiny` MIT licence claim** in their `visibility/README.md`
  — asserted by the archive, never corroborated, and CCP does not publish
  carbonengine under MIT. Treat the notice as false and the code as AGPL. *An
  MIT header inside an AGPL archive asserting a licence for someone else's
  proprietary engine is the single most expensive thing in that tree to
  believe.*
- **The 13 extra character-row fields** EvEJS sends (37 keys against our 24) are
  **[I]** as unwanted; nobody has re-checked `charselData` and
  `characterSlots` for a read of any of them.
- **The dogma operation ordering** remains the largest unresolved risk from the
  inventory: *two independent guesses agreeing is not a decode*, and if the
  order is wrong every ship stat is quietly off and nothing errors. Rule zero
  may settle it — `eve/common/script/dogma` exists — and nobody has looked.

---

*The surprises are evidence; §5 is the finding; §6 is what to do. If you are
about to take one of their subsystems, §6 item 2 costs one command and has the
best record of anything on this page.*
