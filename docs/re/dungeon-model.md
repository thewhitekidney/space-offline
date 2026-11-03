# The dungeon model: `dungeons.fsdbinary`, read end to end

What this is: the complete field layout of the client's dungeon table — the
dungeon row, its rooms, their objects, the NPC spawn entities on those
objects, and the guard parameters that drive them — decoded, cross-checked
against four independently authored files, and reduced to the list of things
a server has to produce in order to put one real EVE encounter in space.
Client build V24.01 (3619879).

Read [`docs/re/static-corpus.md`](static-corpus.md) §6.1 for the container and
what was known before, [`docs/re/native-loaders.md`](native-loaders.md) for
where the field names come from, and [`docs/re/dogma.md`](dogma.md) §9.2 for
the first derivation of this table's structs.

Claims are **[V]** when they were measured against something outside the
loader — a second file, the localisation corpus, or a 100% foreign-key
closure — and **[I]** when they are inference from the layout. Where neither
holds this document says `unknown` and means it.

A third marker, **[V2]**, is used for exactly one thing: a claim that *we*
have not measured in this client, but which an independent third-party decode
of a **different** client build agrees with. **[V2]** is not **[V]** — it does
not mean "we read it", it means "another decoder, working from other bytes,
produced the same answer". In this document it is only ever applied to **field
names**, it comes from exactly one source, and §5.1a states what that source
is and what it does and does not buy.

---

## 1. Headline

**The dungeon table is fully open.** 5,516 dungeons, **8,483 rooms**,
**260,242 objects** and **30,610 NPC spawn entities** — every object with a
type and a position, most with an orientation and a radius, 77,401 with a
localised name, and 39,956 with a spawn rule and a respawn delay.
Every field in the record has an offset and a type, and the four open
questions this lane was given are answered — two a clean yes, one a
qualified yes, one a settled no.

| question | answer |
|---|---|
| Is the object's `position` real coordinates? | **Yes.** `vec3<f32>` at struct `+116`, metres, room-local, deadspace scale. **[V]** |
| List or dict? | **Dict**, decisively. `pyd_schema.py` is right and `dogma.py` §9.2 is wrong. **[V]** |
| Spawn groups and triggers? | **Partly present.** The spawn *rules* and parameters are in the record; the trigger *instances* are not in the corpus. **[V]** |
| Loot? | **Not in the corpus.** Settled: both halves swept by field name, and the eight loot-table references that exist have no table to point at. **[V]** |

The single most consequential result is the second one, because it was also
making the first one unreadable: **`rooms` and `objects` are hash maps, and
reading them as lists returned one room per bucket and a wrong object set.**
Decoding them as dicts makes the per-room object counts reproduce
`asteroidcountbytypeidroomiddungeonid.static` **exactly, 3,054 of 3,054
cells**, where the previous reader disagreed with it on the very first row
anyone looked at (12 Veldspar against 51).

---

## 2. What a server has to emit to place one real dungeon in space

This is the section the rest of the document exists for. Dungeon 47,
*Crimson Hand Supply Depot*, is the worked example; the numbers below are
read out of the client, not invented.

**What the client needs, per `docs/archive/destiny-and-the-ballpark.md` §4.4:** a ball
carries `itemID` (int64), `mode`, `radius` (f32) and `position` (3 × f64); a
`foo.SlimItem` carries `itemID`, `typeID` and `ownerID`, and every SlimItem's
`itemID` must already be a ball. So one object in space is, at minimum,
`(itemID, typeID, ownerID, position, radius)`.

**What the dungeon table supplies, directly, for every one of those fields
except two:**

| destiny needs | dungeon table gives | status |
|---|---|---|
| `typeID` | `object.typeID` @ struct `+140` | **[V]** 6,249/6,249 distinct values are `types` keys |
| `position` | `object.position` @ struct `+116`, `vec3<f32>` | **[V]** metres, room-local |
| `radius` | `object.radius` @ struct `+128` | **[V]** present on 78,000 objects; for asteroids (category 25, group 711) it is the QUANTITY, not a ball radius |
| orientation | `object.yaw` `+144`, `pitch` `+112`, `roll` `+132` | **[V]** three `f32`, degrees **[I]** |
| a per-object id | `object.objectID` @ `+100` (= the dict key) | **[V]** unique within a dungeon; 511 collisions corpus-wide, so it is an authoring id, not an item id |
| `ownerID` | `dungeon.factionID` @ record `+92` | **[I]** the faction is there; the NPC corp per object is not |
| the system anchor | — | `unknown`, and it is *not* in this table |

So the shape of the work is: **take the dungeon's rooms, give each room an
anchor point in a solar system, and emit every object of that room as a ball
plus a SlimItem with the position added to the anchor.** Nothing has to be
authored to do that.

**Positions are room-local.** **[V]** 6,091 of 8,483 rooms (71.8%) contain at
least one object at *exactly* `(0, 0, 0)`, and the types of those objects are
the room's own anchor markers: Beacon (1,442), Invisible Cloud (876),
Generic Agent Boombox (711), Acceleration Gate (704), Deadspace Signature
(392), Celestial Beacon (380), Cosmic Anomaly (362). A per-room bounding box
puts 5,650 rooms in the 10–100 km range and 995 in 100 km–1,000 km, i.e.
**one room is one deadspace pocket**, and the origin is its warp-in point.
**[I]** the server therefore picks one position per room — for the entry room
a celestial or a signature in the system, for the rest an arbitrary deadspace
offset — and the authored layout is relative to it.

**The room graph.** A dungeon's rooms are a chain, and the client's own
authoring shows it: dungeon 47's six rooms are named `R0` *Deadspace Warp
Point* (2 objects: the `entryObjectID` Celestial Beacon and a gate), then
`R1` *Trade Sector* (35), `R2` *Industrial Sector* (68), `R3` *Central
Processing* (26), `R4` *Supply Sector* (84), `R5` *Command Sector* (80).
`dungeon.entryObjectID` @ `+84` is **[V]** an `objectID` of the same dungeon
for 5,499 of the 5,504 dungeons that have one, and `dungeon.connections` is a
list of gates: each element's `fromObjectID` is **[V]** an `objectID` of the
same dungeon (3,340 of 3,396) and carries the ship restriction for that gate
— `allowedShipsList` closes **162/162 into `typelist`** **[V]**. For 5,393 of
5,516 dungeons `len(connections) == rooms − 1`, which is a chain's edge
count. **The gate's destination room is `unknown`**: the connection struct has
no room field, and nothing else in the record names one. Room ids are
consecutive within a dungeon for 5,363 of 5,511 dungeons and the short names
run `R0, R1, …`, so **[I]** the destination is the next room in id order, but
that is an inference and a server can equally keep its own edge list.

**The prose comes free.** Per room, `messageHeaderID` and `messageTextID`
resolve to the pop-up the client shows on entering the pocket — dungeon 47
room 12782: *"CRIMSON HAND INTRANET"* / *"Command Sector: Logging on to
network …"*. **[V]** 2,485/2,485 headers and 2,369/2,369 texts resolve.
Per object, `objectNameID` resolves for **77,401 of 77,401** — that is every
named structure, sentry and asteroid in the game, in CCP's English.

**What a server must still author or source elsewhere, stated plainly:**

1. **The room → solar system placement.** Not in this table. `missions`
   joins a dungeon to a mission and carries `solarSystemID`; placing a
   free-standing anomaly is a separate decision.
2. **NPC fleet composition.** The record says *where* a spawn is and *how* it
   behaves, never *what* ships it contains: that is `entity.npcGroupingID`,
   which points outside the corpus (§5).
3. **Loot.** Nothing (§6).
4. **Trigger instances.** The condition vocabulary ships; which object fires
   which event does not (§5).

---

## 3. Question 1 — the object's geometry, confirmed

`findings/pyd-schemas.tsv` says `position` is a `vec3<f32>` at object struct
`+116`, recovered from a `PyStructSequence_New` over a 3-slot loop with
members `x`, `y`, `z`. **It is exactly that, and the data says so:**

- **Scale.** Over all 260,242 objects, `max(|x|,|y|,|z|)` lands in the
  10–100 km decade for **177,924** of them (68%), 1–10 km for 47,870 (18%)
  and 100–1,000 km for 17,971 (7%). 12,317 objects (4.7%) are at exactly the
  origin, which §2 explains. 83 objects are beyond 10,000 km and are CCP's
  own outliers. **[V] metres on a deadspace-pocket scale.**
- **They differ per object within a room, and they are a layout.** Dungeon 47
  room 12779 holds a Beacon at `(-36121, 0, 0)`, an Ancient Acceleration Gate
  at `(15881, -3415, -1317)`, a Blood Cruise Missile Battery at
  `(1075, 629, 3004)`, a Tower Sentry ~4 km from it, and 51 Veldspar
  asteroids spread over ±30 km, each with its own radius (1,515 m to
  49,092 m). See `findings/corpus-dungeons.tsv` for the rows.
- **Orientation is adjacent and real.** `yaw` @ `+144`, `pitch` @ `+112`,
  `roll` @ `+132`, each an `f32` with its own presence bit. **[V]** 99.2% of
  present values are in `[-360, 360]`, which is degrees and not radians and
  not a quaternion; the remainder are CCP's typos (one yaw of −29,460). A
  conventional value of exactly `360.0` is common on gates and beacons.
  **[I]** the rotation order is `unknown` — three Euler angles in degrees do
  not say which order they compose in, and nothing in the data settles it.
  The names `yaw`, `pitch` and `roll` are **[V2]** (§5.1a): the 3475087 decode
  uses the same three, 493 times, in its verbatim client-field copies, and its
  values fall in `yaw` 0..360, `pitch` -89.95..89.69, `roll` -179.99..179.99 —
  per-axis bounds consistent with degrees in the conventional ranges. That is
  a second decoder's distribution over 493 values, not a unit declaration, so
  the `[I]` on the order and the `[V]` on degrees both stand as they were.
- **`radius` @ `+128` is not geometry you choose.** It is present on ~78,000
  objects, almost all asteroids and large collidables, and ranges from
  sub-metre to 1e7; 20 rows are negative. **For an asteroid it is the
  quantity [V]**: `dungeonHelper.py:85-104` `SetObjectQuantity` stores
  `slimItem.dunRadius = quantity`, `:130-145` `GetObjectQuantity` returns
  `dunRadius` for `categoryAsteroid`, and `:149-165` `GetObjectRadius`
  converts it with `ComputeRadiusFromQuantity` (`dungeoneditor.py:715-747`).
  An earlier **[I]** here read it as the destiny ball radius; that was wrong
  for category 25 and group 711. Other objects keep it as a size.

**What a server emits to place one:** `position + room_anchor` as three
`f64`, `radius` as the ball radius, `(yaw, pitch, roll)` as the orientation
if it sends one at all (§4.4's block 8 is gated on a client setting), and
`typeID` in the SlimItem. No transformation other than the anchor offset is
needed: the numbers are already metres in a right-handed metre frame of the
same kind `solarsystemcontent.static` uses for planets.

---

## 4. Question 2 — the verdict: **dict, not list**

**`pyd_schema.py` is right. `tools/dogma.py` §9.2's "list" is wrong, and
`fsd.CFSD_ROOM_COLUMNS` is wrong with it.** The test the brief proposed was
run exactly as specified, and then three more were run on top of it.

### 4.1 The cheap decisive test

> does `u32_at(record+48)` equal the room count for all 5,516 dungeons?

**Yes. 5,516 of 5,516, zero mismatches.** `+48` is the slot 8 bytes after the
`rooms` pointer at `+40`, and it holds the dict's **total entry count** —
precisely the convention `tools/dogma.py`'s own module docstring records for
`requiredskillsfortypes` ("carries the dict's total entry count in the record
slot after the pointer"). The same test at the next level down also passes:
`u32` at room struct `+40`, after the `objects` pointer at `+32`, equals the
decoded object count for **all 8,483 rooms**, zero mismatches.

### 4.2 The layout, and why the old reading looked right

A cFSD dict field is, per `tools/dogma.py`'s docstring:

```
slot        -> uint64 p
p - 8       =  uint64 bucketCount
p + 8*b     =  uint64 bucketPtr                 (b < bucketCount)
bucketPtr-8 =  uint64 entryCount
bucketPtr + i*stride = entry                    (i < entryCount)
```

and an entry whose value is a struct is **`uint32 key`, 4 bytes of padding,
then the value struct inline**. That gives a room entry of `8 + 96 = 104`
bytes and an object entry of `8 + 152 = 160` — **the same 104 and ~160
`static-corpus.md` §6.1 measured by hand**, which is the first clue that the
two readings were describing the same bytes.

It also explains, exactly, both of the old reader's symptoms:

- **Why the room ids were right.** `fsd.cfsd_list_at(table, r+40)` reads
  `n = u64(p-8)` and then `n` elements from `p` — i.e. it reads the
  **bucket-pointer array** and treats each bucket pointer as a room record.
  A bucket's first entry begins with `uint32 key` + 4 zero bytes, so
  `u64(bucket)` *is* the roomID of that bucket's first entry. The ids were
  real; there were just only `bucketCount` of them.
- **Why the room list "was sometimes a subset".** **[V]** Dungeon 47's rooms
  dict has two buckets, `[12778, 12780, 12782]` and `[12779, 12781, 12783]`,
  and `fsd.dungeon_rooms` reported `12778, 12779` — the first entry of each
  bucket. Dungeon 49's buckets are `[8650, 8652, 8654]` and
  `[8651, 8653, 8655]`, and that lane reported `8650, 8651` while the
  asteroid table named 8652–8654. The caveat recorded on `fsd.dungeon_rooms`
  — "the rooms are a contiguous arena array this list only indexes into" —
  is closed: **the subset is the first entry of each hash bucket.**
- **Why the object counts were wrong, exactly.** This one is worth spelling
  out, because it is the arena-adjacency coincidence of `dogma.md` §10(c)
  caught in the act. `fsd.dungeon_rooms` reads `cfsd_list(table, room + 8)`,
  where `room` is a bucket pointer; `room + 8` is the room struct's `+0`
  `assetManipulations` pointer. For dungeon 47 room 12778 that pointer is
  661,360 — and the `objects` dict pointer (struct `+32`) holds
  **661,368 = 661,360 + 8**, because the generator wrote the two vectors into
  the arena back to back and `assetManipulations` is empty. So `cfsd_list`
  reads its count from 661,360, which is where the *objects dict's bucket
  count* lives, and its elements from 661,368, which is the
  **bucket-pointer array**. **[V]** It therefore
  returns one element per *bucket*: 9 for room 12778 and 17 for room 12779,
  against the 35 and 68 objects those rooms really hold, and the typeIDs it
  reports are the first entry of each bucket — `[16729, 17397, 16735, 17401,
  10124, 12273, 17787, 17627, 17781]`, which is exactly the first-entry list
  this reader produces for those nine buckets. Room 12779's 17 buckets begin
  with twelve Veldspar, which is the famous "12 Veldspar where the asteroid
  table says 51": the count was never an object count, it was a histogram of
  bucket heads.

  **And a smaller correction while we are here.** That discrepancy has been
  recorded as "`dungeon_rooms` reports 12 of type *1031140* while the
  asteroid table says 51 of type *1230*, and both render as Veldspar". There
  is no type disagreement: `fsd_dump.py dungeon` prints each type as
  `_text(msgs, typeNameID)`, so the number in its output is the **message
  id**, and 1,031,140 is typeID 1230's `typeNameID`. `dungeon_rooms` was
  reporting type 1230 all along. Only the count was wrong.

  The silver lining: a bucket pointer **is** an entry start, so
  `CFSD_OBJECT_COLUMNS`' `typeID @+148` was reading the right byte of the
  wrong object. That is why it validated at 97%.

### 4.3 The independent confirmations

Decoded as dicts, the object counts are checkable against three files nobody
involved in this table wrote:

| check | result |
|---|---|
| `asteroidcountbytypeidroomiddungeonid.static` — `{dungeonID: {roomID: {typeID: count}}}` | **3,054 / 3,054 (dungeon, room, typeID) → count cells exact**, over 1,221 dungeons and 1,451 rooms, **0 rooms missing**, 0 mismatches **[V]** |
| `oretypesbydungeonid.static` | 2,660 / 2,660 (dungeon, typeID) pairs present as objects **[V]** |
| `icetypesbydungeonid.static` | 67 / 67 **[V]** |
| `gastypesbydungeonid.static` | 92 / 92 **[V]** |

Plus, inside the table: `object.roomID` @ `+136` equals the containing room's
dict key for **260,242 of 260,242** objects, `object.objectID` @ `+100`
equals its own dict key, `entity.objectID` equals its object's key, and
`object.typeID` closes **6,249/6,249 distinct values into `types`** (the
previous reading managed 97% of *instances*).

**So the room-record layout is settled and the object counts are no longer
suspect.** `findings/corpus-dungeons.tsv` carries the whole shape.

One nuance worth recording so the next reader does not over-read the TSV:
**`pyd_schema.py`'s `dict<str,...>` does not mean the key is a string.** The
extractor cannot see the key type, and in this table every dict key is a
`uint32` id (`roomID`, `objectID`, `entityID`). In `objectivechains` and
`objectivetypes` the same notation covers dicts whose keys *are* string
pointers. The key type is per-field and has to be established from the data.

---

## 5. Question 3 — spawn groups and triggers

**The spawn model is in the record. The trigger instances are not in the
corpus.** Both halves of that are measured.

### 5.1 What is there

Each object may carry an `entities` dict (**[V]** 15,629 objects do;
30,610 entities in all) and an inline `guardCommand` struct (**[V]** on
39,956 objects). Together they are the spawn rule:

```
entity        behaviorName(str)  groupBehaviorName(str)  npcGroupingID *
              entityNameID(msgid)  triggerSpawn(bool) *  additionalLootId
guardCommand  aggressionRange(f32 m) *  orbitRange(f32 m) *  jitterRadius
              entityMinRespawnDelay * / entityMaxRespawnDelay * (s)
              objectMinRespawnDelay * / objectMaxRespawnDelay * (s)
              entityDelaySpawn *  entitySpawnOnce *  objectDelaySpawn *
              objectTriggerSpawn *
```

`*` marks a name that is **[V2]** per §5.1a — a second, independent decode of
client build 3475087 gives that field the same name. **The asterisk is on the
name and nothing else.** It is not on the offset, the size, the type, the
`(f32 m)` or the `(s)` written beside it, nor on the field's meaning; that
source carries decoded values in JSON and says nothing whatever about where in
the struct they sat or what unit they are in. The five unmarked names —
`jitterRadius`, `behaviorName`, `groupBehaviorName`, `entityNameID`,
`additionalLootId` — have **no** second witness, and `jitterRadius`'s absence
is specifically interesting (§5.1a).

Measured over the whole table, these are authored values and not placeholders
**[V]**: `aggressionRange` is 250,000 m on 14,522 objects and 30,000 m on
6,824; `orbitRange` is 1,500 m on 24,195 and 5,000 m on 9,748; respawn delays
are 3,600 s on ~26,000 and 86,400 s on ~11,000; `jitterRadius` takes exactly
three values (500, 1,500, 2,500 m). `objectTriggerSpawn` is true on 8,069
objects and `entity.triggerSpawn` on 9,355 of 30,610 entities, so **roughly a
third of all spawns in EVE's PvE content are trigger-gated rather than
present on room entry** — that is the answer to "where is a spawn group
expressed": it is a flag on the spawn, not a separate table.

`entityNameID` resolves **4,578 / 4,578 [V]**, and the names are what you
would hope: *Eroma Eralen*, *Alpha Squadron*, *November Squadron*,
*Quebec Squadron*. `groupBehaviorName` has 23 values including
*Incursion Drifter Group*, *Incursion Amarr Group* and
*Shadow Of The Serpent Group*; the other 20 and 109 of the 111
`behaviorName` values are **decimal-digit strings** (`"2492"`, `"341"`), i.e.
ids of server-side behaviour trees — and the client ships those trees as
bytecode under `behaviors/trees/`, which is where the next person should
look.

### 5.1a The `guardCommand` names, corroborated at a second client build

The ten `guardCommand` names above were recovered from this client's own
loader, which means they were a *best fit to a getattro chain* — the sizes and
the order were read off the structure, but nothing outside the loader had ever
agreed to the names. Something now does.

**The source.** The third-party EvEJS server ships
`tools/DatabaseCreator/staticTables/dungeonClientContent/data.json`
(4,721,615 bytes), tagged `"provider": "ElysianSDE"` and
`"clientBuild": 3475087`, built by a `ClientDungeonCompiler` from
`manifests/resource-dungeons-3475087.json`. Every template in it carries
`provenance: {kind: client_static, clientBuild: 3475087, geometry: exact,
sourceFields: lossless}`, and that `sourceFields: lossless` tag is the thing
that makes the file usable as a name witness: beside each of its own derived
records it keeps a verbatim copy of the client fields it was built from, under
their client names.

**What was checked, and what came back.** The file holds 26 dungeon templates.
Reading every `guardCommand` in it: **501 occurrences with a struct value**
(objects are stored three times over — once per room, once in a flattened
list, and again inside `sourceFields`), reducing to **89 distinct
(template, room, object) guard commands**. All 89 carry **exactly one key set,
with zero variation**, and it is:

```
aggressionRange  orbitRange
entityDelaySpawn  entitySpawnOnce
entityMinRespawnDelay  entityMaxRespawnDelay
objectMinRespawnDelay  objectMaxRespawnDelay
objectTriggerSpawn  objectDelaySpawn
```

— **ten for ten against our list, spelling for spelling, capitalisation
included.** Each of the ten also appears 384 times inside a `sourceFields`
block, i.e. as a client field name rather than as something EvEJS coined. Two
separately written decoders of two different builds of CCP's own structure
agree on all ten names. **[V2]**

The values agree in kind as well, which is worth one line because it means
both decoders are reading the same authored numbers and not merely the same
dictionary keys: in their sample `aggressionRange` is 250,000 on 128 records,
1,500 on 32 and 30,000 on 8; `orbitRange` is 5,000 on 128 and 1,500 on 46; and
every respawn delay is 3,600. Those are our own §5.1 modes. This is agreement
on the *value distribution*, not a measurement of units — neither file carries
a unit.

**`jitterRadius` is the one name with no witness.** Our `guardCommand` has
eleven fields; theirs has ten, and the missing one is `jitterRadius`. The
string does not occur anywhere in their 4.7 MB. Three readings are open and
this lane cannot distinguish them: the field may have been **added between
3475087 and 3619879**; their compiler may simply **not emit it**; or our name
for it may be **wrong and the field something else**. Our own measurement of
it stands on its own — it takes exactly three values (500, 1,500, 2,500 m) in
our build — but it has no second source, and it is the field a future reader
should be most sceptical of.

**The build caveat, stated plainly.** Their decode is of build **3475087**;
ours is of **3619879**; and the server that ships the file targets a third
build, **3396210**. A field name agreeing across two builds is strong evidence
**for the name** — it is very unlikely that two decoders independently invent
the same ten strings — and it is evidence for **nothing else**. It does not
corroborate our offsets, our sizes, our types, our units, or our reading of
what any value *does*, and it says nothing about whether the field's position
in the struct or its semantics are stable between the two builds. CCP
re-authors these structures between releases; `jitterRadius` is the proof that
the field *set* is not even constant. Every offset in §5 remains a claim about
3619879 alone, resting on our own measurements alone.

**Why we did not just use their file.** Because it is a narrow but deep
sample, and the depth is the only part we needed. Theirs: 26 templates, and the
records actually present in it are **29 rooms, 1,169 objects, 3 spawn entities
and 89 guard commands** (its header `counts` block advertises 103 rooms and
4,097 objects, but those are a wider aggregate its own per-template audits do
not back — the audits sum to 29 and 1,169). Ours: **5,516 dungeons, 8,483
rooms, 260,242 positioned objects and 30,610 entities.** So their file covers
roughly 0.5% of the dungeons and 0.4% of the objects we read, and no
corroboration from it can speak to our coverage, our counts, or any closure
test in §4.3 and §8 — those are ours, and they stay [V] on our own evidence.
What it can do, it does: it confirms the *vocabulary*, which is the part that
had no outside witness at all.

**Other names the same source happens to witness**, recorded so nobody has to
re-derive this: inside its `sourceFields` blocks it uses `roomID` (2,409),
`objectID` (2,398), `typeID` (2,380), `position` (2,380), `radius` (1,874),
`order`, `groupTag`, `yaw` / `pitch` / `roll` (493 each), `objectNameID` (61),
`entities` (25), `npcGroupingID` / `entityID` / `triggerSpawn` (18 each),
`factionID` (7), `allowedShipsList` and `fromObjectID` (6 each), and at dungeon
level `entryObjectID`, `archetypeID`, `difficulty`, `dungeonNameID`,
`entryTypeID`, `useDefaultSpaceMusic` (26 each), plus `objectivesID`,
`descriptionID`, `gameplayDescriptionID`, `contentTags` and `connections`
(3 each). **Names it does not contain at all:** `jitterRadius`,
`behaviorName`, `groupBehaviorName`, `entityNameID`, `additionalLootId`,
`messageHeaderID`, `messageTextID`, `emptyResetDelay`, `triggerTypeID`. So
§7.1's `messageTextID` offset correction, §5.1's behaviour-tree names and
§5.5's entity names get no help here and keep the evidence they already had.

One trap to flag, because the inventory lane that found this file reported
`dunObjectID` as a corroborated name: **it is not a client name.**
`dunObjectID` occurs 1,135 times, but only in EvEJS's *own* derived structures
(`populationHints.miningRocks[]`, `populationHints.environmentProps[]`,
`siteSceneProfile.gateProfiles[]`) and never inside a `sourceFields` block.
The client-side name their verbatim copies use is `objectID` — which is the
name we use, so the claim lands, but via a different field than reported.

### 5.2 The trigger vocabulary, confirmed against the client's own bytecode

`triggertypes.static` is the trigger *condition* enum, and this is now
checkable rather than inferred. **[V]** Its 51 keys are
`1–21, 23, 26–38, 41, 44, 46–48, 50–52, 55–62` and its `triggerTypeName`
strings are, one for one and in id order, the `dunTrigger*` constants in
`out/pyc/evedungeons/common/triggers.pyc`: `Attacked` = 1,
`Ship Entered Proximity` = 2, `Exploding` = 3, `Room Entered` = 8,
`Mined` = 7, `Hacking Success` = 11, `Room Wiped Out` = 31,
`Asteroid Depleted` = 48, … Two independent artefacts of the same build, same
ids, same names.

The *actions* are the `dunTriggerEvent*` half of that same module, and they
are the shopping list for a server that wants to run authored content:
`SpawnGuards`, `SpawnGuardObject`, `ActivateGate`, `DropLoot`,
`SpawnItemInCargo`, `MissionCompletion`, `DungeonCompletion`,
`EffectBeaconActivate`, `ObjectDespawn`, `EntityExplode`,
`GrantGroupReward`, `WarpShipAwayAndComeBack`, `SupressAllRespawn`,
`SpawnEntityByTypeID`, `TimerStart`, `PlayMusic`, `StartConversation`, …
74 constants in all, two of them range markers. **[V]** as a list of names
and ids read out of the client.

### 5.3 What is missing, and the shape of the gap

**No trigger instance exists anywhere in the 203 static files.** The dungeon
record has no `triggerTypeID`, no event list and no target field; the only
trigger-shaped data on an object is the two booleans above. The corpus-wide
field-name sweep (§7) finds `trigger` only in `triggertypes.static`,
`musictriggers`, `environmenttemplates.audioTriggers`,
`conversations.LineOfDialogue.musicTrigger`, `operations.musicTrigger`,
`objectivechains.contextParameterTrigger`, `objectivetypes…triggers`, and
the per-type trigger *configuration* in `spacecomponentsbytype`
(`proximityTrap.triggerRange`, `roomCaptureProximityTrigger`,
`hostileBaiter`, `reactiveSpawnComponent`). **[I]** the per-dungeon trigger
table lives in CCP's server database beside the dungeon editor, exactly as a
loot table does; the client is shipped the enum because its UI and its
authoring tools need the names, and nothing more.

The 3475087 decode independently walked into the same wall, which is worth
recording because a negative result is the hardest kind to corroborate. Its
file has a `triggers` collection and a `triggerCount`, but the only trigger
record it actually emits for any of its 26 templates is
`{kind: "entity_trigger_spawn", objectID, entityID, sourceFields: {...,
triggerSpawn: 1, ...}}` — i.e. **it manufactured a trigger record out of the
`triggerSpawn` boolean**, exactly because there was no trigger instance in the
client data to read. A second decoder had the same two booleans and nothing
else. **[V2]** for the shape of the gap; the inference about *where* the real
table lives is still **[I]**.

**So: triggers exist in the data model, the vocabulary is ours, the spawn
gating flags are ours, and the per-object trigger wiring is not.** A server
replaying authored content gets "this spawn waits for a trigger" for free and
has to decide for itself what the trigger is.

### 5.4 `objectivesID` is settled: it points at `dungeonobjectives`

`static-corpus.md` recorded this as 88% into `objectivetypes`, 38% into
`objectivechains` and 5% into `objectives`. With the record decoded
properly, `dungeon.objectivesID` @ `+104` is present on 330 dungeons with 82
distinct values in 2..106, and:

- **82 / 82 close into `dungeonobjectives`** (85 rows) **[V]** — i.e. 96% of
  that table's rows are referenced, against 13% of `crates`' 636 rows and 0%
  of `objectives`' eight.
- The chain continues and closes at every step: `dungeonobjectives` →
  `clientObjectives.objectiveChainID` **61/61 into `objectivechains`** and
  `clientObjectives.nodeGraphID` **53/53 into `nodegraphs`** **[V]**.
- And it is semantically right: `objectivechains` rows are objective
  templates with `contextParameters` (`dungeon_id`, `solar_system_id`,
  `destroy_target`, `timer`), `objectives[]` with `objectiveType` →
  `objectivetypes`, `groups: ["in_dungeon"]` and label paths like
  `UI/Objectives/Titles/capacitorCharge`; `objectivetypes` rows carry
  `tasks[]` with `taskType` → `objectivetasktypes` and per-task `triggers`.
  `objectives.fsdbinary` is unrelated — eight rows named `[AIR NPE] 4.2`,
  the new-player-experience tracker.

**The small-id trap, named:** 82 values in 2..106 also close 100% into
`crates`, `character_resources` and `character_colornames`, because those
tables have dense keys from 1. A closure percentage over a dense low-id space
is not evidence; the discriminators are *what fraction of the target's rows
get used* and *whether the rows mean anything*. That is the same shape of
mistake as the `volume`/`capacity` error in `types`, and it is why the next
item is stated as a negative.

### 5.5 `npcGroupingID` is **not** a typeID — and this is the trap worth recording

`entity.npcGroupingID` closes **77.9% of 30,610 instances** (76.9% of 6,374
distinct values) into `types`. A lane in a hurry writes "FK → types, 78%".
It is wrong, and resolving the names shows it instantly **[V]**:

```
dungeon 4140 room 22150, object 'Fire Cloud', entity 'November Squadron'
  npcGroupingID 31559 -> types: 'Medium Hybrid Discharge Elutriation II Blueprint'
dungeon 4140 room 22150, object 'Fire Cloud', entity 'Alpha Squadron'
  npcGroupingID 28814 -> types: 'Mid-grade Edge Alpha' (Cyberimplant)
dungeon 4137 room 22141, object 'Acidic Cloud'
  npcGroupingID 29966 -> types: 'OLD Legion Defensive - Augmented Plating'
```

The group-name histogram of the "hits" is Rig Blueprint, Cyberimplant,
Deprecated Subsystem — the 20,000–33,000 typeID band is dense enough that a
dense unrelated id space scores 78% against it by accident. **The target of
`npcGroupingID` is `unknown` and it is not in the corpus**: no table in the
203 is keyed over 1..33,290 in a way that fits, and the field name says
"grouping", not "type". **[I]** it names a server-side NPC fleet template —
the thing that turns one spawn point into *Alpha Squadron*.

---

## 6. Question 4 — loot: it is not in the client, and here is why that is now solid

**No loot table exists in the static corpus.** The previous search was
explicitly not exhaustive; this one is, to the limit of what "exhaustive"
can mean here, and it is a negative result the owner can build a decision on.

**The sweep.** Both halves of the corpus were searched **by field name**, not
by file name:

- the 139 cFSD tables with a recovered layout (1,030 record fields + 771
  nested fields in `findings/pyd-schemas.tsv`), grepped for
  `loot|crate|reward|drop|salvage|trigger|spawn`;
- the classic/sqlite `.static` tables' column lists in
  `findings/corpus-inventory.tsv`, same pattern;
- the 10 cFSD tables with no layout are bare scalars or bare lists
  (`native-loaders.md` §1), so they cannot be a loot table.

**What that turns up, and what each one actually is:**

| candidate | what it is | loot table? |
|---|---|---|
| `crates.fsdbinary`, 636 rows | the hacking/crate container *presentation*: `lootPresentationType` (`fixed` 319 / `hacking` 317), `lootTableType` (`byType` 590 / `byCharacterRace` 46), splash images, `nameID`, `typeID`, and **`lootTableByRace {amarr, caldari, gallente, minmatar}`** | **No** — it *references* loot tables |
| `crates.fixedLootItems` | non-empty on **5 of 636 rows**, 9 typeIDs, all SKINs and facial augmentations (*Omen Headhunter SKIN*, *Men's 'Amaranthine Divide' Facial Augmentation*) | the only literal item list in the corpus, and it is promo-crate contents |
| `crates.lootTableByRace.*` | 156 distinct ids in 10,254..13,480 that **close into nothing** (48% into `types` is the dense-id coincidence again) | **No** — these *are* the loot table ids, and the tables are elsewhere |
| `grouprewards.fsdbinary`, 161 rows | group payout curves: `name` (*"Nexus - Caldari - Easy"*, *"[HF] Push Your Luck - Amarr - Hard"*), `tables[].entries[] {playerCount, quantity}`, `rewardType`, `securityCriteria`, `delayed`, `minimumContribution`, `damageContributionMultiplier`, `rewardingNpcCorporationID` | **No** — ISK/LP by player count for Homefront-style sites, not item drops |
| `entity.additionalLootId` | **5 occurrences in all 260,242 objects**, values 10126..10130 | a reference, and five samples is not a closure test; target `unknown` |
| `spacecomponentsbytype` | per-typeID component config incl. `entosisLootTarget.lootTableId`, `hostileBaiter.rewardLootTableID`, `cargoBay.lootDropPercentage`, `additionalLoot`, `autoLooter` | **No** — four more *references* to loot table ids |
| `wrecks.static` | `{shipTypeID: {raceID: wreckTypeID}}` | **No** (already known) |

**The pattern is the finding.** The corpus holds **eight fields across five
tables that reference a loot table by id** — `crates.lootTableByRace`'s four
race slots, `dungeons`' `entity.additionalLootId`,
`spacecomponentsbytype`'s `entosisLootTarget.lootTableId` and
`hostileBaiter.rewardLootTableID`, and
`pvpfilamentmatchtypes.provingGroundLootTableID` — and **no table of loot
tables for them to point at**. The two this lane could read cleanly were
tested against the key sets of all 144 openable tables:
`crates.lootTableByRace`'s 156 distinct ids close into **nothing**, and
`additionalLootId`'s five values close only into `types`, which five samples
cannot distinguish from chance. The other three were not value-tested — they
live in tables this lane did not validate a schema for — and they do not need
to be: there is no candidate target in the 203 either way.

The client's own bytecode says the same thing from the other side:
`dunTriggerEventDropLoot` is a *server-side* dungeon event,
`out/pyc/behaviors/const/loot.pyc` and
`out/pyc/eveProto/generated/eve/npc/loot/` are a client-side **protocol** for
loot the server has already decided, and `crates`' `lootTableType` enum
(`byType` / `byCharacterRace`) is the client being told *how to present* a
drop it is handed.

**So: we must author loot ourselves.** That is a design decision, and the
evidence for it is a by-name sweep of every field in the corpus plus seven
dangling references, not a failure to find a file. The server has what it
needs to *place* the drop (an NPC with a real `typeID`, a `wreckTypeID` from
`wrecks.static`, a `cargoBay` component) and nothing that says what goes in
it.

---

## 7. Corrections this lane owes other files

This lane owns no tool, so the changes are diffs here.

### 7.1 `findings/pyd-schemas.tsv` — one offset is wrong

`dungeons`' room struct: the TSV puts `messageTextID` at `+84` and notes a
"4-byte tag at `+72` of unknown meaning". It is the other way round.
**[V]** gated on the field's own presence bit `0x10`, the `u32` at struct
`+72` resolves as an English message id for **2,369 of 2,369** rooms and the
text is the room's entry prose (*"Industrial sector: Registered visitors
enter…"*), while `+84` is 0 on 1,968 of them and holds small negative
integers (−2 … −6) where it is set.

```diff
-dungeons	struct:unnamed_2084107264003660358/messageTextID	84	i32	V	getattro chain, PyInt_FromLong; 4-byte tag at +72 of unknown meaning (struct-relative); optional(bit=0x10,word=+88)
+dungeons	struct:unnamed_2084107264003660358/messageTextID	72	i32	V	getattro chain, PyInt_FromLong (struct-relative); optional(bit=0x10,word=+88); offset corrected from +84 by dungeon-model: 2,369/2,369 resolve in the en-us corpus at +72, and +84 holds small negatives
```

The fields at `+76`, `+80` and `+84` are then unnamed, and the room struct
has three more attributes than the extractor recovered names for — the same
failure mode as `position`'s `x`/`y`/`z`. Worth one look by whoever owns the
extractor, since it implies the name/offset pairing can slip at the tail of
a getattro chain.

### 7.2 `tools/fsd.py` — `dungeon_rooms` should read dicts

The minimum change: a dict reader, and `dungeon_rooms` built on it. This
supersedes the `CFSD_ROOM_COLUMNS` / `CFSD_OBJECT_COLUMNS` offsets, which
are a list reading of a hash map.

```diff
+def cfsd_dict(table, slot, entry_stride):
+    """``[(key, value-struct offset)]`` for a cFSD dict field. **[V]**
+
+    ``slot`` holds a payload-relative pointer ``p``; ``u64(p-8)`` is the
+    bucket count and ``p + 8*b`` the b'th bucket pointer; a bucket's own
+    ``u64(bp-8)`` is its entry count and each entry is ``uint32 key``, four
+    bytes of padding, then the value struct inline -- so ``entry_stride`` is
+    ``8 + sizeof(struct)``. Validated: the entry count this yields equals the
+    count the record carries in the slot 8 bytes after ``slot``, for all
+    5,516 dungeons and all 8,483 rooms, and the per-room object counts
+    reproduce ``asteroidcountbytypeidroomiddungeonid.static`` exactly.
+    """
+    p = _U64.unpack_from(table.pl, slot)[0]
+    if not 8 <= p < len(table.pl):
+        return []
+    nb = _U64.unpack_from(table.pl, p - 8)[0]
+    if nb > 1 << 22 or p + 8 * nb > len(table.pl):
+        return []
+    out = []
+    for b in range(nb):
+        bp = _U64.unpack_from(table.pl, p + 8 * b)[0]
+        if not 8 <= bp < len(table.pl):
+            continue
+        n = _U64.unpack_from(table.pl, bp - 8)[0]
+        if n > 1 << 22 or bp + n * entry_stride > len(table.pl):
+            continue
+        for i in range(n):
+            e = bp + i * entry_stride
+            out.append((_U32.unpack_from(table.pl, e)[0], e + 8))
+    return out
+
+#: Room struct (96 bytes) and object struct (152 bytes); dict entries are
+#: ``8 + sizeof(struct)``. See `docs/re/dungeon-model.md` and
+#: `findings/corpus-dungeons.tsv`.
+CFSD_ROOM_STRIDE, CFSD_OBJECT_STRIDE = 8 + 96, 8 + 152
+
 def dungeon_rooms(table, dungeon_id):
-    """``[(roomID, [objectTypeID, ...])]`` for one dungeon. **[V]** partly.
+    """``[(roomID, [objectTypeID, ...])]`` for one dungeon. **[V]**
...
     for key, r in table.records():
         if key != dungeon_id:
             continue
-        out = []
-        for room in cfsd_list_at(table, r + 40):
-            ...
+        out = []
+        for rid, room in cfsd_dict(table, r + 40, CFSD_ROOM_STRIDE):
+            types = [_U32.unpack_from(table.pl, obj + 140)[0]
+                     for _oid, obj in cfsd_dict(table, room + 32,
+                                                CFSD_OBJECT_STRIDE)]
+            out.append((rid, types))
         return out
```

Note the `+140`: with the struct frame handled by `cfsd_dict` returning the
*struct* offset, `typeID` is at struct `+140`, which is the same byte as
`CFSD_OBJECT_COLUMNS`' record `+148` counted from the entry. The two
readings of that one column were always the same byte, which is why it
validated.

`CFSD_COLUMNS['dungeons']` also still has `rooms listc @+32` / `listd @+40`;
`+32` is `contentTags` and `+40` is the rooms *dict*. `dogma.md` §10(c)
already asked for that fix and it is still open.

---

## 8. How this was validated

**Calibration set** — looked at while building the reader: dungeon 47 (the
one `static-corpus.md` quotes), dungeon 49 and dungeon 12454 (the two the
previous caveats named), and `findings/pyd-schemas.tsv`'s `dungeons` block,
used as given.

**Test set** — run once, after the reader was frozen, nothing adjusted
afterwards:

| test | result |
|---|---|
| `u32@+48` == decoded rooms entry count, every dungeon | 5,516 / 5,516 |
| `u32@room+40` == decoded objects entry count, every room | 8,483 / 8,483 |
| `asteroidcountbytypeidroomiddungeonid.static` cells | 3,054 / 3,054, 0 rooms missing |
| `oretypesbydungeonid` / `icetypesbydungeonid` / `gastypesbydungeonid` | 2,660 / 2,660, 67 / 67, 92 / 92 |
| `object.typeID` → `types` (distinct) | 6,249 / 6,249 |
| `object.roomID` == containing room's key | 260,242 / 260,242 |
| `object.objectNameID` in the en-us corpus | 77,401 / 77,401 |
| `entity.entityNameID` | 4,578 / 4,578 |
| `room.messageHeaderID` / `room.messageTextID@+72` | 2,485 / 2,485 and 2,369 / 2,369 |
| `dungeon.dungeonNameID` | 5,511 / 5,516 (5 absent from en-us) |
| `dungeon.descriptionID` / `gameplayDescriptionID` / `missionBriefingID` | 493/493, 64/64, 1,460/1,460 |
| `dungeon.objectivesID` → `dungeonobjectives` | 82 / 82 |
| `connection.allowedShipsList` → `typelist` | 162 / 162 |
| `dungeon.archetypeID` → `archetypes` | 58 / 58 (dense low ids; see §5.5's caveat) |
| `triggertypes.static` ids and names vs `evedungeons/common/triggers.pyc` | 51 / 51 |
| `guardCommand` field names vs the EvEJS/ElysianSDE decode of build 3475087 | **10 / 10** names identical over 89 guard commands, one key set, zero variation — names only, and `jitterRadius` absent from their set (§5.1a) |

**Things deliberately *not* claimed**, having been looked at: the gate →
destination-room edge; the Euler rotation order; `npcGroupingID`'s target;
`additionalLootId`'s target; `difficulty`'s and `archetypeID`'s scales;
`room.emptyResetDelay`'s units; the three unnamed room fields at `+76`,
`+80`, `+84`; whether any offset, size or unit in §5 holds at build 3475087,
which §5.1a's name agreement does not touch; and anything from `spacecomponentsbytype`, whose record was
read once with an unvalidated schema and is another lane's table.

**The two traps in the brief, and what happened to them.** The
double-applied `delta` was avoided by taking `pyd-schemas.tsv`'s record
offsets as literal and nested-struct offsets as struct-relative, which the
`u32@+48` test then confirmed on the first run. The `volume`/`capacity`
shape — a field that validates on its own sample — showed up twice here, as
`objectivesID`'s 100% closure into three unrelated tables and as
`npcGroupingID`'s 78% into `types`; both were caught by asking what fraction
of the *target's* rows get used and by reading the resolved names out loud.

---

## 9. What is still `unknown`

1. **The gate → room edge.** `connections[].fromObjectID` names the gate; no
   field names the destination. Room ids are consecutive in 97% of dungeons
   and the short names are `R0, R1, …`, so the ordering is probably it, but
   it is an inference.
2. **`entity.npcGroupingID`'s target** — the NPC fleet template. Not in the
   corpus. This is the single biggest remaining gap for running authored
   content, because it is *what* spawns.
3. **`entity.behaviorName` / `groupBehaviorName`** resolve to digit strings;
   `out/pyc/behaviors/trees/` is the place to look and nobody has.
4. **Loot contents** (§6) and **trigger instances** (§5.3) — absent by
   design, not by oversight.
5. **Room → solar system placement**, and whether a room's anchor has a
   canonical offset at all.
6. Scales and units of `difficulty`, `archetypeID`, `emptyResetDelay`,
   `order`, and the Euler composition order.
7. **Whether `jitterRadius` is really a `guardCommand` field, and really that
   name.** It is the one field in §5.1 the 3475087 decode does not have, and
   §5.1a lists the three explanations this lane cannot choose between.

---

## 10. Scope note

Nothing from the client is committed. `findings/corpus-dungeons.tsv` carries
the table's *shape* — field, offset, kind, presence bit, and what each claim
was validated against — plus one dungeon's row, its six room headers and ten
of its 295 objects, whose only job is to show the reader produces the right
values. The sample values quoted in this document are there for the same
reason. 52 MB of CCP's dungeon layouts stays on the machine that has a
client, as the owner's standing rule requires.
