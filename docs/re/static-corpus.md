# The static corpus: `res:/staticdata/`, client build V24.01 (3619879)

> **Demoted 2026-10-24** — the room/object offsets in §6.1 are superseded. Current truth for §6.1: [dungeon-model.md](dungeon-model.md). Kept for history; do not brief a lane from this file.

What this is: the 203 files the client ships under `res:/staticdata/`, how to
reach them, what format each one is in, which of them we can read today, and
— the question this lane was created to answer — **what is actually inside
`dungeons.fsdbinary`, `missions.fsdbinary` and `solarsystemcontent.static`.**

Tooling: [`tools/fsd.py`](../../tools/fsd.py) is the reader (library, stdlib
only, read-only), [`tools/fsd_dump.py`](../../tools/fsd_dump.py) is the CLI,
and [`findings/corpus-inventory.tsv`](../../findings/corpus-inventory.tsv) is
the per-file inventory. Message ids are resolved with `tools/loctable.py`,
which belongs to the `localization-corpus` lane.

Claims are marked **[V]** (read out of the client's own code, or reproduced by
decoding a real file and checking it against something outside that file) or
**[I]** (inferred). Where neither applies this document says `unknown`, and it
means it.

---

## 1. Headline

**The corpus is readable.** 54 of the 203 files decode completely, with CCP's
own field names and types. The other 149 are in a second container whose
*structure* is now decoded — row count, primary keys, record size, and the
variable-length arena — but whose *per-field layout* is generated C++ and is
described nowhere in the bytecode. For the tables that mattered, the columns
we needed out of those 149 have been found and each one confirmed against an
independent source.

**The content question is answered, and the answer is yes.** `dungeons.fsdbinary`
is not an environment envelope: it is 5,516 dungeons, each with a name, a
designer's description, a faction, and **a list of rooms, each room holding a
list of objects carrying real `typeID`s** — Veldspar, Beacon, Acceleration
Gate, Habitation Module, Tower Sentry Bloodraider III. `missions.fsdbinary`
is 2,892 missions, each with a name, a **`dungeonID` pointing into that
table**, reward `typeID`s and ISK amounts, a content-template key, and **a
list of `(key, messageID)` pairs whose message ids resolve to the full
briefing prose**. `solarsystemcontent.static` is the entire contents of all
8,490 solar systems, with a self-describing schema: planets, moons, asteroid
belts, NPC stations, stargates with destinations and positions, stars with
spectral class, security, security class.

Details, and the parts that are still `unknown`, below.

---

## 2. Reaching the files

**[V]** `$EVE_CLIENT/resfileindex.txt` is a 5-column CSV, one line per
resource:

```
res:/staticdata/types.fsdbinary,3c/3cc5bf8ff5e9099a_c0ee9c29...,<md5>,9169272,1140000
```

Column 2 is a path relative to **`$EVE_CLIENT/../ResFiles/`** — the cache is a
*sibling* of the `tq` install directory, not a child of it. **[V]** All 203
`res:/staticdata/*` entries resolve, all 203 exist, and all 203 are stored
uncompressed (on-disk size equals column 4 for every one), so nothing has to
be inflated.

That sibling relationship is a trap for containers: mounting `$EVE_CLIENT`
alone gives you the manifest with none of the data. `tools/analysis.sh` now
mounts the parent at `/client-root`, and `fsd.Client` looks for `ResFiles` at
`$EVE_RESFILES`, then beside the install directory, then at
`/client-root/ResFiles`.

---

## 3. Four formats, not three

By extension there are three, but `.static` splits in two, so there are four
real formats.

| format | files | readable | what it is |
|---|---:|---|---|
| classic FSD | 31 `.static` | **fully** | `uint32` schema size, a pickled *optimized* schema, then the data |
| fsdlite SQLite | 14 `.static` | **fully** | a SQLite 3 database whose `value` column is plain JSON |
| pickle | 3 `.pickle` | **fully** | a Python 2 pickle, nothing else |
| schema sidecar | 6 `.schema` | **fully** | the optimized schema in YAML, for six `.static` files that embed none |
| cFSD | 149 `.fsdbinary` | **container only** | an undocumented container read by generated C++ |

### 3.1 Classic FSD — the one the bytecode documents

**[V]** The client reads it in pure Python we already have, and
`tools/fsd.py` is a port of exactly those five modules:

```
out/pyc/fsd/schemas/binaryLoader.pyc              the container, LoadFromString
out/pyc/fsd/schemas/loaders/__init__.pyc          int/float/bool/string/enum/union/vector
out/pyc/fsd/schemas/loaders/listLoader.pyc        list
out/pyc/fsd/schemas/loaders/objectLoader.pyc      object
out/pyc/fsd/schemas/loaders/dictLoader.pyc        dict, IndexLoader, MultiIndexLoader
out/pyc/fsd/schemas/predefinedStructTypes.pyc     the struct formats
```

File layout, from `LoadFromString`:

```
uint32  schemaSize
bytes   schemaSize       cPickle of the optimized schema (protocol 0)
bytes   ...              the data, parsed with that schema
```

The full node grammar is in the `tools/fsd.py` docstring and is not repeated
here. Two things are worth pulling out.

**The schema is a foreign-key map, for free.** **[V]**
`binaryLoader.defaultLoaderFactories` maps thirty-odd type names onto eight
loaders: `typeID`, `localizationID`, `dungeonID`, `npcCorporationID`,
`factionID`, `marketGroupID`, `spawnTableID`, `npcFleetTypeID`,
`groupBehaviorTreeID`, `typeListID`, `npcTag`, `certificateTemplateID` and
the rest are **all just `int`**. The name is not used for decoding at all —
it exists to say what the integer references. Reading a schema therefore tells
you the join graph without decoding a byte of data.

**`buildIndex` and `multiIndex` are access strategies, not layouts.** **[V]**
`LoadIndexFromFile` streams a `buildIndex` dict from the file rather than
loading it whole, and `MultiIndexLoader` adds secondary indices. The bytes are
laid out the same either way, because every node's offsets are relative to
that node's own start, so slicing a value out and parsing it at offset 0 is
equivalent to parsing it in place. `tools/fsd.py` reads both with one code
path; **[V]** reproduced on `solarsystemcontent.static`, which is a
`buildIndex,multiIndex` dict nested three levels deep.

### 3.2 The six sidecar schemas

**[V]** `systems`, `regions`, `constellations`, `jumps`,
`factionsowningsolarsystems` and `dialogs` ship no embedded schema — the
`uint32` at offset 0 is already data — and a sibling
`res:/staticdata/<name>.schema` carries the schema in YAML instead. Those
YAML files are the **already optimized** schema: they contain
`constantAttributeOffsets`, `endOfFixedSizeData`, `optionalValueLookups`,
`fixedItemSize` and `size`, all of which only the optimizer produces. So no
port of `schemaOptimizer.py` is needed; parse the YAML and hand it to the same
loader. `tools/fsd.py` has a deliberately small YAML subset parser for this
(block mappings, flow collections, one anchor form) because the six files are
machine-generated and use nothing else.

### 3.3 The SQLite `.static` files are fsdlite caches

**[V]** 14 `.static` files are not FSD at all: `SQLite format 3`, with the
schema `cache(key, value, time)` plus `indexes(key, value)`. **[V]** That is
`fsdlite`: `fsdlite/storage.pyc` `_cache_save` writes
`self.cache[key] = fsdlite.encode(obj, json=True)` and index rows as
`'{indexName}.{indexKey}' -> key`, and `_cache_load` reads them back with
`fsdlite.decode(data, json=True)`. **[V]** confirmed in the data: a
`blueprints.static` `indexes` row reads `productTypeID.31292 -> 31293`, and
the matching `cache` value is JSON with CCP's own field names:

```json
{"activities": {"research_material": {"skills": [{"typeID": 3409, "level": 5}, ...],
  "materials": [{"typeID": 11457, "quantity": 1}, ...], "time": 525...}},
 "blueprintTypeID": 31293, "maxProductionLimit": 1}
```

So these 14 are the *easiest* part of the corpus, not the hardest, and that
includes `blueprints` (5,100 rows), `skins` (7,048), `skinlicenses` (11,877),
`dbuffcollections` (277), `fighterabilities`, `clonegrades` and the five
`industry_*` tables. This is also the one place `fsdlite` touches the shipped
corpus at all: **[V]** `fsdlite` is a YAML/JSON-plus-SQLite store for
`.staticdata` source files, and it has nothing to do with `.fsdbinary`. The
brief's hope that `fsdlite/encoder.pyc` documents the `.fsdbinary` format is
**wrong** — `encoder.pyc` encodes to JSON, not to a binary container.

### 3.4 cFSD (`.fsdbinary`) — the native fork, and how far it is open

**[V] The fork in the brief is real.** `pyFSD/__init__.pyc` is two lines:

```python
import blue, sys
sys.modules[__name__] = blue.LoadExtension('_pyfsd')
```

and `fsdBuiltData/common/base.pyc` (`BuiltDataLoader.__LoadBinaryFromResFolder`)
logs `'Trying to load cFSD file at %s'` and calls `cls.__loader__.load(...)`,
falling back to `binaryLoader.LoadFSDDataForCFG` — the pure-Python path — only
when `__loader__` is `NO_BINARY_LOADER`. Each table's wrapper sets
`__loader__` to its own generated C extension; e.g.
`paperdoll/common/npc.pyc` sets `__loader__ = paperdollsLoader` for
`res:/staticdata/paperdolls.fsdbinary`.

**[V]** `$EVE_CLIENT/bin64/` contains **exactly 149 `*Loader.pyd` files** —
one per `.fsdbinary` table, matching one-for-one. **[V]** `bin64/staticdata/`
contains **only** `mapObjects.db`; there is no SQLite `types` table and no
other sidecar database. That is a settled negative, not an open question.

**[V]** But the container is not native-only, and this lane decoded it. The
structure, reproduced on all 149 files:

```
bytes[24]              tag              high-entropy, per-file
uint64                 payloadSize      == filesize - 32, on all 149
-- payload starts here; every offset below is payload-relative --
uint64                 headerSize       24 on 147 of 149
uint64                 rowCount
uint64                 bucketCount
uint64[bucketCount]    bucketDataOffset
...
<bucket 0>  uint64 rowsInBucket, then rowsInBucket fixed-size records,
            then that bucket's variable-length data
<bucket 1>  ...
```

- **[V]** `rowsInBucket` sits at `bucketDataOffset - 8`, so the first bucket's
  offset is always `headerSize + 8*bucketCount + 8`. Checked on all 147
  well-formed files.
- **[V]** The primary key is a **hash map on `key % bucketCount`**: every
  record in bucket *b* has `key % bucketCount == b`, and keys ascend within a
  bucket. The key is a 32-bit integer at the start of the record.
- **[V]** `sum(rowsInBucket) == rowCount` on all 147.
- **[V]** Variable-length data is addressed by a payload-relative `uint64`,
  with a `uint64` length in the eight bytes *before* it. A **string** is the
  bytes at that offset; a **list** is `uint64` elements there. A list field
  occupies **two** record slots 8 bytes apart — a pointer to the count and a
  pointer to the first element — which is how the 52 MB of `dungeons` is
  reached.
- **[V]** The record size (`stride`) is uniform within a table and was
  determined for **121 of 147** tables by intersecting, across every bucket
  holding two or more rows, the set of distances at which a plausible second
  key could sit, then checking each survivor against every record of every
  bucket. The 26 that failed are nearly all tiny tables where no bucket holds
  two rows, so the data cannot constrain the answer; `goals` (2 rows, 2
  buckets) is the clean example. That is a limit of the method, not a defect
  in the file.
- **[I]** The 24-byte tag is a schema or content fingerprint, by analogy with
  the compatibility check the classic loader gets for free from its embedded
  schema. It is **not** the md5 in `resfileindex.txt`. `unknown` beyond that.
- **[V]** `headerSize` is not always 24: `audiometadata.fsdbinary` has 56 and
  a header of alternating counts and offsets (`[56, 10775, 2444072, 22,
  2450368, ...]`), which looks like several indices in one file;
  `seasonselection.fsdbinary` has 40 and is a singleton with no hash map at
  all (its payload runs straight into `res:/UI/Texture/classes/...`). Those
  two are `header-only` in the inventory. `unknown`.

**What is not decoded, and will not be from the bytecode.** The field layout
inside a record. The generated `.pyd` resolves attributes by name, so the
*names* are in the module as plain ASCII — `tools/fsd_dump.py names <table>`
extracts them — but that is an **unordered, untyped set with no offsets**, and
it must not be presented as a column list. Converting it into one means
reading the generated getters out of the `.pyd`: each is a tiny function
`PyInt_FromLong(*(int*)(self->data + K))`, and `capstone` is already in the
analysis image, so extracting the `K` per name is tractable work on ~150 small
DLLs. **This lane stopped here, as its brief instructed.** See §7.

**The attribute names are extractable, and they are worth having even
unordered.** `tools/fsd_dump.py names <table>` filters the `.pyd`'s strings
down to lowerCamel/snake identifiers, and for `types.fsdbinary` that yields
CCP's own list: `basePrice capacity certificateTemplate descriptionID
designerIDs factionID graphicID groupID iconID isDynamicType isisGroupID
marketGroupID mass metaGroupID metaLevel portionSize published quoteAuthorID
quoteID raceID radius soundID techLevel typeID typeNameID
variationParentTypeID volume wreckTypeID`. That is an **unordered, untyped
set with no offsets**, so it is not a column list — but combined with a
profiling pass it names columns that profiling alone can only shape. Three of
the `types` columns below were named exactly that way and then confirmed:
`portionSize` (`+40`, never zero, range 1..5000), `quoteAuthorID` (`+108`,
resolves to *"Lai Dai marketing division"*, *"Thukker pilot motto"*) and
`quoteID` (`+112`, resolves to *"Deny your enemy the satisfaction of
combat."*, *"Move fast, strike true."*).

**What this lane did find in those records** — `fsd.CFSD_COLUMNS` — was found
by profiling every row of a table and then confirming each column against
something outside the file: foreign-key closure into another cFSD table,
resolution through the localisation corpus, or a published EVE value. Every
column below is **[V]** in that sense; nothing was adopted on resemblance.

---

## 4. Priority 1 — `accountingentrytypes.fsdbinary`

27 KB, 218 rows, stride 40, fully decoded. `tools/fsd_dump.py rows
accountingentrytypes`.

```
+0   uint32  entryTypeID              [V] the bucket key
+8   uint64  -> description string    [V]
+16  uint64  -> name string           [V]
+24  uint32  optional id A            [I] order not established, see below
+28  uint32  optional id B            [I]
+32  uint32  entryTypeNameID          [V]
+36  uint32  presence bitfield        [V] bit0 <-> A, bit1 <-> B, no exceptions
```

**[V]** All 218 rows decode and `sum(bucket counts) == rowCount == 218`. The
two pointers resolve to the English name and description (`0 -> "Undefined"`,
`95 -> "Upkeep adjustment fee" / "Upkeep cost of upgrade for remainder of
current billing period"`). **[V]** `entryTypeNameID == 59792 + entryTypeID`
for all 103 ids `<= 103` — which is the run the `localization-corpus` lane
derived independently out of the localisation pickle, by name, without seeing
this file. Two sources, one answer.

**[I]** Fields A and B are `entryJournalMessageID` and
`entryTypeDescriptionID` — the two remaining attribute names in
`bin64/accountingEntryTypesLoader.pyd` — **in an order this lane did not
establish**. The lean is A = description id, because for the newest rows A is
exactly `entryTypeNameID - 1`, i.e. allocated beside the name (id 190:
A=1011654, nameID=1011655). That is a lean, not a finding. One lookup each in
the localisation corpus settles it.

Note for the `wallet-ledger` lane: the id space runs to 13005, not to 218, so
this is not a dense table.

---

## 5. Priority 2 — `types` / `groups` / `categories` / `marketgroups`

`typeID -> groupID -> categoryID` is **recovered, closed and cross-checked.**

```
types.fsdbinary       53,288 rows   stride 152
  +0    typeID                  [V] key
  +8    basePrice     f64       [V]
  +16   capacity      f64       [V]
  +32   mass          f64       [V]
  +40   portionSize             [V]
  +48   radius        f64       [V]
  +56   volume        f64       [V]
  +68   descriptionID msgid     [V]
  +84   groupID                 [V]
  +96   marketGroupID           [V]
  +108  quoteAuthorID msgid     [V]
  +112  quoteID       msgid     [V]
  +116  raceID                  [V]
  +128  typeID (repeated)       [V]
  +132  typeNameID    msgid     [V]

groups.fsdbinary       1,611 rows   stride 32
  +0  groupID  [V]   +4  categoryID  [V]   +8  groupID (repeated)  [V]
  +12 groupNameID msgid [V]

categories.fsdbinary      48 rows   stride 40
  +0  categoryID [V]  +16 categoryID (repeated) [V]  +20 categoryNameID msgid [V]
```

How each is **[V]**:

- **Foreign-key closure.** Every one of the 53,288 `groupID` values is a key
  of `groups`; every one of the 1,611 `categoryID` values is a key of
  `categories`. Zero exceptions either way. A wrong offset does not close.
- **Localisation closure.** All 53,288 `typeNameID` values resolve in the
  English corpus; so do all 1,611 `groupNameID` and all 48 `categoryNameID`.
- **Published values.** Rifter: group 25 Frigate, category 6 Ship, mass
  1,067,000 kg, radius 31 m, volume 27,290 m³, capacity 140 m³, basePrice
  400,000. Avatar: group 30 Titan, mass 2.43e9 kg, radius 6,800 m. Tritanium:
  group 18 Mineral, category 4 Material, volume 0.01 m³. Veldspar: group 462,
  category 25 Asteroid, volume 0.1 m³. Drake: group 419 Combat Battlecruiser,
  capacity 450 m³. These are EVE's own numbers.

This independently reproduces the `item-spawn` lane's calibration — it found
`groupID` at `+84`, `nameID` at `+132`, `volume` at `+56` and `categoryID` at
`+4`, and all four agree exactly. Worth noting what that lane's method could
*not* have caught, and what the profiling did: the double at `+16` is
**`capacity`, not `volume`** (Rifter 140, Drake 450, Dominix 750 — cargo
capacity, not hull volume), and `volume` really is at `+56`. A calibration
against a handful of `(typeID, groupID)` constants would not have distinguished
them.

`marketgroups.fsdbinary` is 2,114 rows at stride 28, keys recovered, columns
`unknown` — nobody has needed them yet. `typedogma` (27,160 rows, stride 24),
`dogmaattributes` (2,871, stride 80) and `dogmaeffects` (3,427, stride 112)
likewise have keys and strides but no confirmed columns. **Dogma is
therefore present and enumerable, and only one profiling pass away from
usable.** That is the next job in this lane, not a research question.

---

## 6. Priority 4 — what is actually in the content files

This is the question the programme was created for: *is EVE's PvE content
recreatable at all?* The answer from the data is **yes for the structure, and
yes for the prose, and they join**.

### 6.1 `dungeons.fsdbinary` — 52 MB, 5,516 dungeons, with rooms and objects

```
+0    dungeonID            [V] key
+32   rooms (list count)   [V]
+40   rooms (list data)    [V]
+68   descriptionID msgid  [V]
+76   dungeonID (repeated) [V]
+80   dungeonNameID msgid  [V]
+88   typeID               [V] FK -> types, 373 distinct
+92   factionID            [V] FK -> factions, 27 distinct
+96   a prose messageID    [V] resolves; role unknown
+100  a prose messageID    [V] resolves to objective text
```

**[V]** All 5,516 `dungeonNameID` values resolve: *"Crimson Hand Supply
Depot"*, *"Sansha War Supply Complex"*, *"Angel Military Operations Complex"*,
*"The Wrath of Angels"*, *"Spy Games"*. `descriptionID` resolves to the
designer's note: *"&lt;P&gt;According to reliable sources within the Imperial
navy this deadspace complex is currently occupied by..."*. The column at `+100`
resolves to **objective text**: *"Kill Yukiro Demense."*, *"Follow Eroma
Eralen. Ambush him and take his Caldari Navy Overlay Transmitter."*

The room list is where the 52 MB lives. Each element points at a **104-byte
room record** holding `roomID` at `+0` and a list of objects at `+8`/`+40`.
Each object record carries a **`typeID` at `+148`** — **[V]** 97% of non-zero
values across 3,794 sampled objects are valid `typeID`s, with 456 distinct
values, and they resolve to exactly what a dungeon is made of.

Two independent confirmations that this is real and not pattern-matching:

1. **The room ids are the room ids.** `asteroidcountbytypeidroomiddungeonid.static`
   — the filename that started this whole programme — is a classic FSD table
   we decode completely: `{dungeonID: {roomID: {typeID: count}}}`, 1,221
   dungeons. **[V]** It says dungeon 47 has room 12779 containing Veldspar
   (count 51). The room list in `dungeons.fsdbinary` for dungeon 47 yields
   rooms **12778 and 12779**, and room 12779's object list contains **12
   Veldspar objects**. The two files were decoded by completely different code
   paths and agree on the id space.
2. **The objects read like dungeon objects.** Dungeon 47, room 12778: *Cargo
   Rig, Habitation Module - Casino, Landing Pad, Habitation Module -
   Roadhouse, Beacon, Ancient Acceleration Gate, Starbase Storage Facility,
   Stationary Mammoth, Starbase Hangar.* Room 12779: *12x Veldspar, Beacon,
   Ancient Acceleration Gate, Blood Cruise Missile Battery, Tower Sentry
   Bloodraider III, Barren Asteroid.* Dungeon 49 room 8650: *8x Sansha
   Elevator, 3x Sansha Bunker, 2x Acceleration Gate, Beacon.* Dungeon 59 room
   8776: *4x Frozen Corpse, Beacon, Ancient Acceleration Gate.*

So: **`dungeons.fsdbinary` carries room layouts and per-room object lists with
real type ids, not merely an environment and graphics envelope.** The envelope
is a *separate* table — `dungeonenvironmenttemplates.fsdbinary`, 114 rows —
which is itself evidence: the environment would not need its own table if
`dungeons` were only that. 52 MB over 5,516 dungeons is ~9.5 KB per dungeon,
which is object-layout scale.

**What is still `unknown` here, stated plainly:**

- The object record's **width, position and rotation**. Positions must be in
  there (the objects spaced at ~160 bytes have many slots that look like
  doubles), but no position column is confirmed, so none is claimed.
- **Spawn groups and triggers.** Not found. `triggertypes.static` decodes and
  is a real table — 51 rows of `{triggerTypeName, allowOnGuards,
  allowOnGuardObject}` — so triggers exist in the data model, and the sentry
  guns above are spawned objects by any ordinary reading. But *which* object
  fires *which* trigger, and where a spawn *group* is expressed, is `unknown`.
  It is very likely in the object record or in one of the two dungeon list
  columns this lane did not follow.
- **The room list is sometimes a subset.** Dungeon 49's room list yields 8650
  and 8651, while the asteroid table names 8652, 8653 and 8654 for the same
  dungeon. So either the dungeon record carries more than one room list, or
  the rooms are a contiguous arena array that this list only indexes into.
  Unresolved, and recorded as a caveat on `fsd.dungeon_rooms`.
- **Loot tables.** Not found anywhere in the 203, and this lane did not
  exhaust the search. `crates.fsdbinary` (636 rows, stride 168) and
  `grouprewards.fsdbinary` (161 rows, stride 56) are the two candidates by
  name and shape; neither was profiled. `wrecks.static` decodes and is
  **not** a loot table — it is `{shipTypeID: {raceID: wreckTypeID}}`, 294
  rows, i.e. which wreck model a hull leaves behind.

### 6.2 `missions.fsdbinary` — 2,892 missions, with rewards and the prose join

```
+0    missionID              [V] key
+48   contentTemplateKey str [V] e.g. "agent.missionTemplatizedContent_BasicKillMission"
+56   messages (count)       [V]
+64   messages (data)        [V]
+92   npcCorporationID       [V] FK -> npccorporations
+120  factionID              [V] FK -> factions
+144  dungeonID              [V] FK -> dungeons, 1,461 distinct
+196  missionNameID msgid    [V] resolves for all 2,892
```

Plus, from profiling and not yet pinned to a name, **[V] as shapes**: six
further `FK -> types` columns (reward and courier items), three columns
ranging to 10.6M / 10.5M / 40M (ISK reward, bonus, collateral — magnitudes,
not names), one to 100,000 (loyalty points, by magnitude), one to 43,200
(seconds — the 12-hour bonus window), small enums at `+80` and `+192` that are
mission type and level by range. Those are shapes, so they are listed as
shapes; none is named here because none was confirmed.

**The prose join is proven.** The `messages` list's elements are 16-byte
records `{char* label, uint64 messageID}` -- **two `uint64`s, the first a
pointer to a name string.** This originally read `{uint32 key, uint32
messageID}`, which was wrong and was corrected by two later lanes
independently: `agents-missions` hit it reading per-mission prose slots, and
`dogma-corpus` quantified it from the loader's own generated code -- the list
is at record **+72**, not +56, and all **17,268 values over 2,892 missions
resolve as message ids (100%)**.

The keys being strings rather than an integer enum is *better* than the
original reading suggested: each one names its prose slot
(`messages.mission.briefing` x2,885, `messages.mission.offered.agentsays`
x1,983, 27 labels in all). A server can therefore serve a real mission's whole
conversation out of two FSD columns and the localisation corpus, with no
authored text.

Those message ids resolve to the full briefing. The test was deliberately adversarial: the `localization-corpus`
lane supplied message id **145165** as "1,314 characters of Serpentis mission
briefing" before this lane looked for it. **[V]** It occurs exactly once in
`missions.fsdbinary`, inside the arena, reachable from mission 1210
*"Vengeance"* — and its sibling entries in the same list resolve to
*"I have terrible news, {[character]player.name}. A major personnel convoy
carrying countless..."*. So:

**mission row -> message id -> briefing text, offer, acceptance and completion
lines**, with the `{[character]player.name}` and
`{[location]dungeonLocationID.name}` token slots intact, and **mission row ->
dungeonID -> rooms -> objects -> typeIDs** on the other side. Both halves of
the PvE model are in the client and they join through integers we can read.

### 6.3 `solarsystemcontent.static` — 92 MB, 8,490 systems, fully decoded

This one needs no reverse engineering at all: it is classic FSD with an
embedded, self-describing schema, so we get CCP's names and types directly.
`tools/fsd_dump.py schema solarsystemcontent`:

```
dict[int] buildIndex,multiIndex -> object
  solarSystemID, solarSystemNameID, descriptionID, factionID, security,
  securityClass, luminosity, radius, center/min/max (vector3),
  border, corridor, fringe, hub, international, regional,
  disallowCyno, disallowScanning, dscanRange,
  disallowedAnchorCategories, disallowedAnchorGroups,
  sunTypeID, sunFlareGraphicID, systemWideCloud, visualEffect,
  warpTunnelOverwrite, wormholeClassID,
  star    -> { id, typeID, radius, npcStations, statistics{age, life, locked,
               luminosity, radius, spectralClass, temperature} }
  secondarySun -> { itemID, typeID, position, effectBeaconTypeID }
  planets -> dict[int] -> { typeID, celestialIndex, position, radius,
               solarSystemID, planetNameID, planetAttributes, statistics,
               moons, asteroidBelts, npcStations }
  stargates -> dict[int] -> { typeID, destination, position, rotation,
               allowedShipsTypeListID, ignoredByCorporationDefenseDjinn }
  disruptedStargates -> dict[int] -> { typeID, position, rotation,
               targetSolarSystemID }
```

**[V]** Decoded end to end. System 30000001 comes out with a 0.858 security,
security class `B`, three stargates with destinations, six planets with
positions and radii, moons, asteroid belts and NPC stations, and a star of
type 45041, radius 63,350,000 m, with a spectral class. That is the entire
map's contents, authored, in a file we can read today.

### 6.4 The rest of the PvE scaffolding

From the inventory, all **[V]** as row counts and keys:

| table | rows | status |
|---|---:|---|
| `dungeonobjectives.fsdbinary` | 85 | keys + stride 56, columns unknown |
| `objectives.fsdbinary` | 8 | keys + stride 64 |
| `objectivetypes.fsdbinary` | 95 | keys + stride 40 |
| `objectivetasktypes.fsdbinary` | 53 | keys + stride 48 |
| `objectivechains.fsdbinary` | 111 | keys + stride 40 |
| `epicarcs.fsdbinary` | 21 | keys + stride 56 |
| `agentsinspace.fsdbinary` | 360 | row count only |
| `agenttypes.fsdbinary` | 13 | keys + stride 16 |
| `conversations.fsdbinary` | 1,275 | keys + stride 24 |
| `conversationagents.fsdbinary` | 44 | keys + stride 72 |
| `dialogs.static` | 4,223 | **fully decoded** (sidecar schema) |
| `voidspaceencounters.fsdbinary` | 8 | keys + stride 48 |
| `operations` / `operationsites` / `operationtasks` | 325 / 21 / 397 | keys + strides |
| `npccharacters.fsdbinary` | 11,424 | keys + stride 136 |
| `npccorporations.fsdbinary` | 283 | keys + stride 224 |
| `triggertypes.static` | 51 | **fully decoded**: `triggerTypeName`, `allowOnGuards`, `allowOnGuardObject` |
| `wrecks.static` | 294 | **fully decoded**: `{raceID: wreckTypeID}` per hull |
| `oretypesbydungeonid.static` | 1,120 | **fully decoded** |
| `icetypesbydungeonid.static` | 21 | **fully decoded** |
| `gastypesbydungeonid.static` | 83 | **fully decoded** |
| `asteroidcountbytypeidbeltid.static` | 40,928 | **fully decoded** — belt composition for every belt in the game |
| `asteroidcountbytypeidroomiddungeonid.static` | 1,221 | **fully decoded** |

`dialogs.static` closes the `error-vocabulary` lane's dependency: it decodes
with the sidecar schema and carries `dialogID`, `dialogType` (an enum over
`hint/info/notify/question/audio/warning/error/fatal/windowhelp`), `bodyID`,
`titleID`, `suppressable`, `closable`, `urlAudio`, `urlIcon`, keyed by message
key string. All five columns that lane asked for are there.

---

## 7. What a full cFSD reader would take

For the record, since this lane stopped at the fork deliberately.

The missing piece is, per table, an ordered list of `(name, type, offset)`.
The names are already extractable from the `.pyd` (`fsd_dump.py names`). The
offsets are immediates in the generated getter functions. The path:

1. Parse the PE's `.rdata` for the `PyGetSetDef` or method table of the
   generated row type — an array of `{const char *name, getter, setter, const
   char *doc, void *closure}`. `tools/pe_functions.py` and
   `tools/resolve_vas.py` already do PE section and VA work, and `pefile` is
   in the analysis image.
2. For each entry, disassemble the getter. They are a few instructions each:
   load `self->data`, add a constant, load, box. `capstone` is in the image.
   The constant is the offset; the boxing call (`PyInt_FromLong`,
   `PyFloat_FromDouble`, `PyString_FromStringAndSize`, ...) gives the type.
3. The table order falls out of the getset array order, which gives the
   schema's attribute order for free.

That is mechanical work on ~150 small DLLs with a single code generator behind
them, so one correct extractor handles all of them. The payoff is the
remaining 143 tables at full fidelity, including `typedogma`,
`dogmaattributes` and `dogmaeffects` — the ones that let an NPC built from a
real `typeID` fight with real EVE numbers.

The cheaper intermediate step, and the one to do first: the profiling pass in
`tools/fsd_dump.py profile` plus FK and localisation closure found every
column this lane needed in minutes per table, with confirmation built in. For
`dogmaattributes` and `typedogma` that is probably enough.

---

## 8. Scope note

Nothing from the client is committed here. `findings/corpus-inventory.tsv`
carries each table's *shape* — resource name, size, format, whether it opens,
row count, key type, column names and types — and no table contents. The
sample values quoted in this document are there to prove the reader works;
anyone who wants the data obtains the client themselves.
