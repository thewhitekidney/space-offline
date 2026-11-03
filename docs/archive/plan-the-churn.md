# The churn: a comprehensive reverse-engineering pass

> **Demoted 2026-10-28** — a finished plan: its research waves ran (the notes are in `docs/re/` and `docs/lanes/`). Current truth: `docs/archive/plan-subsystems.md`. Kept for history; do not brief a lane from this file.

Written 2026-10-19, at the owner's request: *"a comprehensive RE of the client
to assist us in making implementations in the server much easier, this is
where we'll be able to scale parallelism to the max"* — together with the
question this document turned out to be mostly about: *"what about the
content? there is a huge gap between the client and what is orchestrated by
the server... this is truly not something we can recreate, right?"*

Companion documents: [`docs/archive/roadmap.md`](roadmap.md) is what we build and in what
order; [`docs/archive/plan-docked-subsystems.md`](plan-docked-subsystems.md) is the current
execution round; [`lanes/README.md`](../lanes/README.md) is how parallel work is
divided and every rule below defers to it. This document is the *research*
programme, which is a different thing from either: it produces files, not
features.

---

## 1. Why the loop feels like brute force

Today's loop is: the owner launches the client, reports a symptom, we read the
bytecode around that symptom, write one arm, and ask them to relaunch. It
works, and it has closed a lot of ground. But its cost is dominated by one
thing, and it is not the diagnosing:

**We do not know the shape of the answer until we go and look, one call at a
time, and a wrong shape costs a human's evening.**

The census (`findings/client-calls.tsv`, 1,816 call sites) was the first
attack on that, and it was the right one: it tells us *where* every remote
call is and *what the client does with our answer* — subscripts it, unpacks
it, iterates it, caches it behind a guard, hands it to native code. That is
what found four silent live locks before they cost anyone an evening.

What the census does not carry is the **shape**: which container family, which
columns, in which order, with which types, and which of them are load-bearing.
That is still re-derived by hand, per call, at the moment someone needs it.

So the churn's thesis is narrow and testable: **shape knowledge is the
bottleneck, it is bulk-extractable, and extracting it parallelises perfectly
because it is read-only.** Nothing in the churn writes Rust. Its output is
`findings/*.tsv` and `docs/re/*.md`, and its value is that the next twenty
features are lookups instead of investigations.

## 2. The thing we had not noticed: there are two RE projects here

Conflating them is most of why progress feels like brute force.

**Project A — the call surface.** What the client asks us, and what each
answer must look like. This is everything we have done so far. Its ground
truth is the `.pyc` tree (`out/pyc/`, 12,774 files). It parallelises cleanly
by service domain, because service modules are disjoint.

**Project B — the static corpus.** *What exists in EVE.* 203 files under
`res:/staticdata/`, reachable right now, that we have touched exactly once —
`bin64/staticdata/mapObjects.db`, for one station's four ids.

We have been running Project A almost exclusively, and Project B is the
force multiplier: most of Project A's expensive questions ("what `groupID`
does this type have", "what are the accounting entry types", "what does a
market group tree look like") stop being investigations and become lookups the
moment the corpus is readable.

**One lane unlocks all of Project B**, and it is therefore the highest-leverage
piece of work currently identified anywhere in this repository.

## 3. The corpus, and the answer to the content question

`docs/archive/recon-v24.01.md` already recorded that static data is client-side, and the
`.fsdbinary` reader was listed there as an open question. Taking the inventory
properly changes the picture considerably. Every file below is present on
disk and its size is measured, not estimated:

| file | size | what it would mean |
|---|---|---|
| `dungeons.fsdbinary` | **52 MB** | not a name table |
| `solarsystemcontent.static` | **92 MB** | what is *in* each solar system |
| `types.fsdbinary` | 9.2 MB | every item in the game |
| `npccharacters.fsdbinary` | 1.9 MB | NPC agents/characters |
| `missions.fsdbinary` | 1.3 MB | missions, at least as the journal sees them |
| `accountingentrytypes.fsdbinary` | 27 KB | the wallet's ref types, which a lane is deriving by hand today |

And the rest of the PvE scaffolding is there by name: `dungeonobjectives`,
`dungeonenvironmenttemplates`, `objectives`, `objectivechains`,
`objectivetypes`, `objectivetasktypes`, `epicarcs`, `agentsinspace`,
`agenttypes`, `conversations`, `conversationagents`, `dialogs`,
`voidspaceencounters`, `operations`/`operationsites`/`operationtasks`,
`triggertypes`, `goals`, `wrecks`, `surveys`, plus
`oretypesbydungeonid`, `icetypesbydungeonid`, `gastypesbydungeonid` and
`asteroidcountbytypeidroomiddungeonid`.

That last filename is worth reading twice. **It is keyed by room id and
dungeon id** — so in this data model a dungeon has *rooms*, and the client
ships a table keyed by them. Belt and ice composition, which is content in
every ordinary sense, is client-side.

### So: can we recreate the content?

**The honest answer is "much more of it than I would have guessed, and the
file list above is evidence rather than hope — but what is in those files is
unproven until someone reads them, and that is Wave 0's job, not a thing to
assume."**

The distinction that matters is between the **nouns** and the **verbs** of
PvE:

- **Nouns — almost certainly all recoverable.** Every type, group, category,
  dogma attribute and effect, every blueprint and material, the whole map
  including stargates and celestials, NPC corporations and factions, agents,
  station services and operations, market groups, skill requirements, ore and
  ice distribution. These are shipped because the client cannot render or
  compute without them. **This includes dogma, which is the one that matters
  most**: because the attribute and effect definitions are client-side and
  complete, an NPC we build from a real `typeID` fights with real EVE numbers
  without us authoring a single balance decision. We get combat fidelity for
  free and only have to author encounter *design*.
- **Verbs — this is the open question, and it is narrower than it looks.**
  Whether `dungeons.fsdbinary` holds room layouts and spawn groups, or only
  the environment/graphics envelope for them. Whether `missions.fsdbinary`
  holds objectives and rewards, or only the titles and types the journal
  draws. Whether loot tables, rat spawn tables and NPC aggression behaviour
  appear anywhere. **52 MB and 92 MB are the reason to go and look rather
  than assume**, and the reason to look *before* designing anything.

Two refinements the owner's framing deserves:

**The SDE complements this less than one would think.** The SDE is largely the
same static data in friendlier form — it is a convenience, not a second
source. It does carry a few things in more usable shape (the fully denormalised
map with celestial positions, agents, blueprints, planetary schematics), and
`mapDenormalize` in particular may be cheaper than the client's own map
tables. But **the SDE has never contained missions, dungeons, spawn tables or
loot tables either.** If those are not in the client, the SDE will not rescue
them.

*Corroborated 2026-10-20, and it is the strongest kind — behavioural.* EvEJS
downloads the official SDE at first start
(`https://developers.eveonline.com/static-data/tranquility/eve-online-static-data-3396210-jsonl.zip`,
`docker/entrypoint.sh:11-13`) and **never reads a client at all** — zero grep
hits for `ResFiles|SharedCache|index_tranquility|CLIENT_PATH` in `server/src`.
And then it bundles **300+ MB of content anyway**:

```
dungeonAuthority/data.json        67.7 MB   "compiled from EVE client 3475087"
missionAuthority/data.json        14.0 MB
agentAuthority/data.json          10.8 MB
npcBehaviorProfiles/data.json      6.5 MB
content-packs/*.json             ~300 MB   DED rated sites, combat anomalies,
                                           pirate exploration, ore/gas/wormhole
                                           sites, homefront, drone anomalies
abyssalLootTables.generated.data     —     *generated*
abyssalRewardTables.generated.json 6.6 MB   *generated*
```

Nobody downloads the SDE and *then* compiles 67 MB of dungeon geometry out of a
client if the SDE carries it. So the SDE is the **"what things are"** layer —
types, groups, dogma, map, stations — and the **"what happens where"** layer
is client-only, exactly as claimed above. The two `generated` filenames also
independently confirm our loot negative (`docs/re/dungeon-model.md`): a second
project with every incentive to find a loot table invented one instead.

The architectural consequence is ours, not theirs. We read that layer from the
player's own client **at runtime** and ship none of it; they baked it into a
586 MB archive — which is what the `LEGAL.md`/`NOTICE.md` they deleted in this
same release used to forbid (§0.2 of the reference doc).

**Worth doing, though:** the SDE is public, versioned per build, official, and
JSONL. As a *cross-check* on our hand-decoded cFSD it is the cheapest second
source we will ever get — if our `types.fsdbinary` reading agrees with CCP's
own published table for the same build, the 71-column FK sweep stops being
our arithmetic and starts being two independent readings. It replaces nothing;
it would **verify** a lot. Same rule as the client: the player downloads it,
we ship a reader.

**And the part that genuinely is not shipped, we were always going to author.**
NPC aggression and target selection, warp-in behaviour, respawn timing,
anomaly generation, loot rolls, mission scripting glue. That is not a gap in
our data; it is the game design, and per
[`eve-offline-end-goal`](roadmap.md) an MMO-RTS meant to run for years with no
victory condition needs its own answers to all of it anyway. **It also
collapses into one piece of work with something already on the roadmap**: the
AI that flies a rat and the AI that flies a player-like bot are the same
engine with different objectives. Authoring that is the project, not a tax on
it.

One more thread worth pulling, listed as a Wave 0 question because it is
cheap and the upside is large: `localization_fsd_en-us.pickle` is **40 MB**,
and CCP localises mission briefing text. If mission prose is in that pickle,
the most labour-intensive content in EVE is sitting in a plain Python 2 pickle.

### Update, same day: the prose half is settled, and the answer is yes

The `localization-corpus` lane read it, and `tools/loctable.py` is the reader.
The corpus is a plain Python 2 protocol-0 pickle -- no container, no
compression, no header, and (verified by disassembling the opcode stream) not
a single opcode that can import or construct anything. **308,657 English
messages, 29.5 million characters, of which 276,826 are reachable only by
numeric id -- and that is the content**, as against the 31,831 that are
label-addressable UI chrome.

What is demonstrably in there: mission briefings, offer, completion, failure
and decline lines, objectives text, mission titles, item and type
descriptions, NPC corporation and faction and bloodline descriptions, dungeon
object names in runs of thousands, and every `UserError` body. Verified
directly rather than taken on report -- message 145165 is 1,314 characters of
Serpentis reconnaissance briefing, and its stored token record names the two
slots the *server* fills:

```
{[character]player.name}            variableType 0
{[location]dungeonLocationID.name}  variableType 3
```

**That is the shape of the whole arrangement: the server sends a message id
and a dict of arguments, and the client renders CCP's prose.** Plurals and
gendered forms resolve client-side from `conditionalValues`, so we send a
number or a character id and the client picks the arm. The join to the rest of
the corpus is a plain global integer `messageID` with no namespace, which
means `types.fsdbinary`'s name and description columns hold ids rather than
strings -- and `tools/loctable.py id <N>` is therefore a cheap oracle for
*verifying a decoded FSD column*: if a column's integers resolve to briefing
prose, it is the briefing column.

**So §3's open question has narrowed by half.** The prose exists and is
readable. What is still unproven is the *structure* -- which mission, which
objectives, which spawns, in which room -- and that is exactly what
`dungeons.fsdbinary` and `missions.fsdbinary` would hold. The decisive test is
now specific rather than open-ended: **do those tables carry columns of
integers that resolve through `loctable.py` to this prose?** If they do, the
join is proven and the content question is answered completely.

### Update, same day: the structure half is settled too, and the answer is yes

`tools/fsd.py` and `tools/fsd_dump.py` are the reader. The `fsd-reader` lane
decoded **54 of the 203 files completely, with CCP's own field names and
types**, and recovered keys and record strides for 121 of the 149 native ones.
`dungeons.fsdbinary` is **not** an environment envelope -- the graphics
envelope is a separate 114-row table, `dungeonenvironmenttemplates`. It is
5,516 dungeons, each with a description, a faction, an objective-text id and a
**list of rooms, each room carrying a list of objects with type ids**. That is
where the 52 MB lives, about 9.5 KB per dungeon.

Verified directly rather than taken on report. Dungeon 47:

```
dungeonNameID  113227 'Crimson Hand Supply Depot'
descriptionID   62481 '<P>According to reliable sources within the Imperial
                       navy this deadspace complex is currently occupi...'
factionID      500012
room 12778  objects=9   Beacon, Ancient Acceleration Gate, Landing Pad,
                        Habitation Module - Casino, Habitation Module -
                        Roadhouse, Cargo Rig, Starbase Storage Facility,
                        Starbase Hangar, Stationary Mammoth
room 12779  objects=17  Veldspar x12, Beacon, Ancient Acceleration Gate,
                        Blood Cruise Missile Battery,
                        Tower Sentry Bloodraider III, Barren Asteroid
```

**The object names are the proof on their own**: no graphics table contains a
Blood Cruise Missile Battery per room, and two rooms joined by an Ancient
Acceleration Gate is the literal structure of an EVE deadspace complex.

`missions.fsdbinary` is 2,892 missions with a name, a
`contentTemplateKey` (e.g. `agent.missionTemplatizedContent_BasicKillMission`),
an NPC corporation, a faction, reward and courier item foreign keys into
`types`, ISK- and LP-magnitude columns, a 43,200-second bonus window, and
**`dungeonID` -- 1,461 distinct values joining into `dungeons`**. The prose
join was tested adversarially: the lane was handed message id 145165 (the
Serpentis briefing in the section above) *before* it looked, and found it
exactly once in the file, inside a mission's `(key, messageID)` list, reachable
from mission 1210 "Vengeance" whose sibling entries resolve to the matching
offer lines.

So both chains hold:

```
mission -> messageID -> CCP's prose
mission -> dungeonID -> rooms -> objects -> typeIDs
```

`solarsystemcontent.static` needed no reverse engineering at all -- it is the
classic self-describing FSD container, and all 8,490 systems decode with
planets, moons, asteroid belts, NPC stations, positions, radii, stargates with
destinations, star spectral class, security, faction and wormhole class.
`asteroidcountbytypeidbeltid.static` gives belt composition for **every belt in
the game** (40,928 rows).

**That cross-check was reported as only partly holding, and it fully holds --
the discrepancy was ours, twice over.** The note here used to say that the
room ids agreed with `asteroidcountbytypeidroomiddungeonid` but the type ids
and counts did not: `{12779: {1230: 51}}` against an object list reading type
1031140, count 12.

Both halves were bugs on our side. Rooms and objects are **dicts, not lists**,
so the list reader returned one room per hash bucket and, one level down,
read the objects dict's *bucket count* as an object count -- it had landed on
an empty `assetManipulations` pointer whose value is exactly
`objects_ptr - 8`. And **there was never a second type id**: 1031140 is
Veldspar's `typeNameID`, which `fsd_dump.py`'s own label printed in place of
the typeID.

Read correctly, dungeon 47 has **six** rooms and room 12779 holds **51 of type
1230** -- the asteroid table's figure exactly. Across the whole corpus the
reproduction is 3,054 of 3,054 cells with no room missing, and the dict
reading is confirmed against the record's own count field for all 5,516
dungeons and all 8,483 rooms.

The lesson is the one this document's §2 already argues: a disagreement
between two readings is information, and the reading to doubt first is the one
you wrote.

### What is still missing, and it is the part we were always going to author

- **Loot is genuinely absent, and the negative is now solid rather than
  provisional.** Both halves of the corpus were swept by field name.
  `crates` turns out to be container *presentation* and its `lootTableByRace`
  ids close into **nothing**; `grouprewards` is ISK and LP payout curves by
  player count. Eight fields across five tables reference a loot table by id
  with **no table of loot tables to point at**, and the client ships loot only
  as a *protocol* (`dunTriggerEventDropLoot`). **We author loot ourselves.**
- **Triggers: the vocabulary is shipped, the wiring is not.**
  `triggertypes.static`'s 51 rows are *exactly* the `dunTrigger*` enum in the
  client's own `evedungeons/common/triggers.pyc` -- same ids, same names --
  and the 74 `dunTriggerEvent*` actions are enumerated. But **no per-object
  trigger wiring exists anywhere in the 203 files**, so "when X happens, do Y"
  is ours to write against CCP's own vocabulary.
- **Object geometry is located, and it is real coordinates.** `vec3<f32>` at
  the object struct's `+116`, in metres and **room-local**: 68% of 260,242
  objects fall in the 10-100 km decade, and 6,091 of 8,483 rooms have an
  anchor object -- Beacon, Acceleration Gate, Deadspace Signature -- at
  *exactly* (0, 0, 0). Yaw, pitch and roll are `f32` degrees nearby; the Euler
  composition order is `unknown`.
- **NPC behaviour is partly shipped, and this entry used to deny it.** It said
  aggression, target selection, warp-in and respawn timing were never
  shipped. **The spawn rules are in the dungeon record**: `entities` on 15,629
  objects and `guardCommand` on 39,956, carrying authored aggression radius
  (250 km), orbit distance (1.5 km) and respawn delays (3,600-86,400 s), with
  `triggerSpawn` set on 9,355 of 30,610 entities. What is *not* shipped is the
  engine that acts on them -- target selection and the AI itself -- which is
  still the game design, and still the same engine as the player-like bots.
  The biggest remaining unknown is `npcGroupingID`, which is **what** spawns:
  it closes 78% into `types` and is *not* a typeID (resolving its names gives
  "Fire Cloud", "November Squadron", and a blueprint), so it is left
  `unknown` loudly.
- **Per-field layout of the native tables -- now 139 of 149 done**, with
  1,030 record fields plus 771 over 225 nested record types and **zero fields
  typed `unknown`**, extracted from the loaders' own generated code rather
  than calibrated. All five dogma tables decode, including 5,178 modifier
  rules. What remains is ten tables with no row factory to read a stride from,
  and a handful of enum *meanings* the binary does not state.

One side effect worth recording because it changes another lane's job: the
`UserError` vocabulary is enumerable *today*, without decoding any FSD at all.
`tools/loctable.py errors` yields **2,974 distinct message keys, 1,226 of them
with named arguments**, each with its title and body text. That was scheduled
as hand transcription in Wave 1 and is now a generated column.

## 4. The one shared decision, settled here and not in the lanes

`lanes/README.md` rule 2: a shared decision is removed before a round, not
negotiated during it. For a research programme the shared thing is the
**output schema**, so it is fixed here. Twenty lanes' output only composes if
it was written to compose.

**`findings/shapes-<area>.tsv`** — one row per `(service, method)`:

```
service  method  args  family  columns  types  consumer  guard  cadence  errors  confidence  citation
```

- `family` — `CRowset` | `Rowset` | `CIndexedRowset` | `CFilterRowset` |
  `KeyVal` | `DbRow` | `dict` | `list` | `tuple` | `scalar` | `object` |
  `none` | `unknown`. The family is not cosmetic: `CRowset` subclasses `list`
  so an empty one is **falsy**, old-style `Rowset` has no `.columns`, and a
  packed `DbRow` refuses `STORE_ATTR`.
- `consumer` — what the client does with it, the census's vocabulary.
- `guard` / `cadence` — whether the answer feeds a guarded cache and *what
  paces the re-ask*. Per `docs/guides/debugging-the-client.md` §4 the cadence, not the
  guard, is what decides whether a falsy answer is a cosmetic re-ask or a
  spin: user-paced is fine, a render handler or `@uthread2.debounce` or
  `BufferedCall` or `AutoTimer` is a loop.
- `errors` — the `UserError` the client expects on failure, if any. We cannot
  currently return a sensible error for anything, and for a lot of UI the
  right error is what makes the window behave.
- `confidence` — `V` (read from the code), `I` (inferred), `unknown`.
  **`unknown` is a first-class value and beats a plausible guess.** A
  confident wrong census is worse than a gap, because nobody re-checks it.
- `citation` — `path.py:LINE`, where `LINE` came from `tools/call_lines.py` or
  `tools/pyc27.py`. **Never from decompiler output**: uncompyle6 emits a
  reconstructed file and its line numbers have been observed 720 lines off,
  and three lanes in one round cited them before noticing.

**`findings/corpus-<table>.tsv`** for a static table: its real columns and
types, plus a sample, plus the row count.

**`docs/re/<area>.md`** for the prose, **`docs/lanes/<name>.md`** for the park
note. Nothing else is written by a research lane.

## 5. The waves

Waves exist because the dependencies are real, not to ration parallelism.
Within a wave, everything runs at once.

### Wave 0 — the unlock (2 lanes)

Everything in Wave 3 and much of Wave 1 is downstream of this.

**`fsd-reader`.** Can we read `.fsdbinary` and `.static` outside the client?
The client ships its own loader as bytecode we already have — `fsdlite`
(including `encoder`, `storage`, `indexer`, `cache`), `pyFSD`, `fsd`,
`fsdBuiltData`, `fsdUtilities/loaders` — and some tables ship a `.schema`
beside them. The deliverable is a *reader in `tools/`*, an inventory of all
203 files with what each actually contains, and the answer to §3's open
question. **This is the single highest-leverage lane currently identified.**

**`localization-corpus`.** The `localization_fsd_*.pickle` family, starting
with `en-us` at 40 MB and `main` at 3.3 MB. What is the message id space, how
does the client resolve a string, and — the question with the largest upside
— **is mission and NPC prose in there?**

### Wave 1 — the call surface, by domain (up to ~10 lanes, fully parallel)

One lane per service domain, each producing `findings/shapes-<domain>.tsv` and
`docs/re/<domain>.md`. Domains are chosen to match the service modules, which
makes them disjoint by construction:

`items-inventory` · `dogma-fitting` · `market-contracts` · `industry-science`
· `character-skills-clones` · `corp-alliance-standings` ·
`space-navigation-map` · `agents-missions-pve` · `social-mail-notifications` ·
`ui-settings-petitions-help`

Two cross-cutting lanes belong in this wave and are worth as much as any
domain lane:

**`error-vocabulary`.** Every `UserError` / `ThrowUserError` the client
renders, with its message key and the arguments it formats. Returning the
*right* error is frequently what makes a UI behave sanely instead of hanging,
and today we cannot return one at all.

**`session-and-binding`.** Every `session.changed` key, what the client
derives versus receives, and every bind/resolve path. This is the state
machine we are least able to see and the one whose bugs look like a black
screen.

### Wave 2 — the dynamic layer

`destiny-grammar` (blocked on the reference blob — one `/execfile` in a
running client, `tools/destiny-ref.sh`), `ballpark-entities` (what a ship, an
NPC, a structure is on the wire), `undock-warp-jump` (the state machines).
The undock black screen lives here.

### Wave 3 — the content model

Scheduled *after* Wave 0, because its first question is "what is actually in
`dungeons.fsdbinary`" and that is unanswerable until the reader exists.
`dungeon-model`, `mission-model`, `npc-and-spawning`, and then a design lane
for our own authoring format — which should be CCP's data model wherever
their tables turn out to carry one, and ours only where they do not.

## 6. What bounds this, honestly

- **Reviewing lane output is serial and it is the coordinator's context.** The
  lanes are cheap; reading twenty reports carefully is not. This is the real
  limit on wave width, and it is why rule 5 exists: **the park note is the
  deliverable and a chat report is not.** A lane that files well costs the
  coordinator a paragraph. One that does not costs an hour.
- **Research lanes share one working tree.** Hence rule 4 (foreign compile
  errors are expected and ignored), rule 8 (never `git stash` while a lane is
  writing) and the convention that a lane regenerates a shared artifact once,
  at the end.
- **Trust calibration, which has already cost us once.** An earlier lane
  marked `charMgr.GetHomeStationRow` as answered when it was not, and the cost
  landed on the owner's next relaunch. The gate catches compile and test
  failures; it cannot catch a confident census. Spot-check the claims that
  decide what we build next.
- **Nothing from the client is redistributable.** The owner's standing rule is
  *"we're not shipping anything but our stuff — if anyone needs anything from
  the original client they still have to obtain that in a legal way."* So the
  churn's output is **readers, schemas and documentation**, never extracted
  data committed to this repository. A `findings/corpus-*.tsv` carries the
  *shape* of a table and a handful of sample rows to prove the reader works;
  it is not a copy of the table. That line is not a formality — it is what
  makes the difference between a tool and a redistribution.

## 7. Order of operations

1. Wave 0, now. Two lanes. Everything else waits on the first of them only
   where it genuinely does.
2. Wave 1 in parallel with Wave 0 — it reads the `.pyc` tree, not the corpus,
   so it is not blocked. Start with the domains that the current execution
   round needs (`items-inventory`, `character-skills-clones`), and the two
   cross-cutting lanes.
3. Wave 3 opens the moment `fsd-reader` reports, and its first deliverable is
   a straight answer to §3's open question.
4. Wave 2 opens when the owner runs the `/execfile` that produces the Destiny
   reference blob.

The serial spine in `docs/archive/plan-docked-subsystems.md` round 2 continues underneath
all of it, in the central files no lane may touch, and is unaffected.
