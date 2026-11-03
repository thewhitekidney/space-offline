# Dogma: the client's attribute-and-effect system, client build V24.01 (3619879)

What this is: the column layout of the client's dogma tables, how each column
was established, and what the server can do with them that it could not do
yesterday.

**Headline: the dogma tables are decoded, with CCP's own field names, types,
offsets and optionality, and the layouts were not guessed — they were read out
of the client's own generated code.** `typedogma` yields all 648,726 attribute
values and 54,307 effect entries across 27,160 types; `dogmaattributes` all
2,871 attribute definitions; `dogmaeffects` all 3,427 effects *including the
`modifierInfo` records that say what an effect does to which attribute*.

Tooling: [`tools/dogma.py`](../../tools/dogma.py), built on
[`tools/fsd.py`](../../tools/fsd.py). Shape files:
[`findings/corpus-typedogma.tsv`](../../findings/corpus-typedogma.tsv),
[`corpus-dogmaattributes.tsv`](../../findings/corpus-dogmaattributes.tsv),
[`corpus-dogmaeffects.tsv`](../../findings/corpus-dogmaeffects.tsv).

Read [`docs/re/static-corpus.md`](static-corpus.md) first; it decoded the cFSD
*container* and this document finishes the *records*. Claims are marked **[V]**
(read out of the client's own code, or reproduced against a source outside the
file) or **[I]** (inferred). Where neither applies this says `unknown`.

```
tools/analysis.sh python3 tools/dogma.py validate \
    --client /client --resfiles /client-root/ResFiles
```

runs all 55 checks in §7. They pass, with zero failures, on V24.01.

---

## 1. The method: §7 of `static-corpus.md`, done

`static-corpus.md` §7 proposed reading each table's layout out of
`bin64/<table>Loader.pyd` by parsing a `PyGetSetDef` table and disassembling
~150 tiny getters. **There is no `PyGetSetDef` table.** **[V]** Scanning every
byte offset of every section of `dogmaAttributesLoader.pyd`,
`dogmaEffectsLoader.pyd` and `typeDogmaLoader.pyd` for a `uint64` equal to the
address of any identifier-shaped string finds **no pointer to a single
attribute name**; the only identifier literals that are pointed at are the
*method* names in the `PyMethodDef` tables (`load`, `get`, `keys`, `items`,
`__dir__`, `_items_`). The generator emits one `tp_getattro` instead, and
`strcmp` is in the import table precisely because of it. So the plan's step 1
is a dead end and step 2 is unnecessary: there are no 150 tiny getters, there
is **one** getter per generated type, and it is a flat `strcmp` chain that
spells the whole schema out in order.

**[V]** Each arm of that chain contains, with nothing left to interpret:

| what | where it comes from |
|---|---|
| the field **name** | `lea rdx, [rip + x]` pointing at the string literal |
| the field **offset** | the displacement of the load in that arm |
| the field **type** | the load's form: `mov dword` → int32, `movzx byte` → bool, `movss` → float, `movsd` → double, `mov qword` followed by `mov rdx, [reg-8]` → string (the loader fetching the length that precedes the bytes), `mov qword` boxed with `PyLong_From*` → int64, `lea`/a store of the field address into a fresh wrapper → list, dict or nested object |
| **optionality** | `test <size> ptr [reg + P], BIT` guarding the arm and jumping to the `Py_None` return |
| the field **order** | the order of the arms, which is also the struct order |

The struct pointer is named by the prologue, always
`mov <reg>, qword ptr [rcx + 0x10]` — `self->data`. One such prologue per
generated type, so a table with list fields yields its row struct *and* one
struct per element type, each complete.

`tools/dogma.py derive <table>` does this, and `tools/dogma.py schema <table>`
prints the result that was committed. The two agree for every table here; that
is the point of shipping both.

**This generalises to all 149 cFSD tables.** It is the same code generator
behind every one, and the extractor needed no per-table special-casing for the
thirteen tables tried. §9 opens two non-dogma tables with it in minutes, §9.1
audits the `fsd-reader` lane's three, §9.2 opens the whole dungeon content
model and §9.3 settles an open dispute about `missions`.

One trap, and it is the only one: **`.text` is not all instructions.** Some
loaders carry jump tables or alignment blobs inside it, and capstone's
`disasm` stops dead at the first byte it cannot decode. Unfixed, that made
`missionsLoader.pyd` yield 731 instructions out of ~37,000 and the extractor
report an *empty* schema — a silent wrong answer, which is the worst kind.
`derive_schema` resynchronises by skipping the offending byte and continuing.
Anyone reusing this code elsewhere should keep that loop.

### 1.1 The record model, which `static-corpus.md` could not see

**[V]** A cFSD record is

```
uint32  key                      <- what CfsdTable.stride already finds
        padding to the struct's own alignment
<row struct>                     <- generated C++, laid out by MSVC
```

and the struct's alignment is **8** when it contains any pointer or `double`
and **4** otherwise. So the struct begins at record `+8` for seven of the
eight tables here and at record `+4` for `marketgroups`, whose fields are all
`int32` and `bool`.

This is checkable without trusting it: `struct_at + sizeof(struct)` must equal
the stride the container reports. **[V]** It does, for every table:

| table | struct at | struct size | total | stride found by `fsd.CfsdTable` |
|---|---:|---:|---:|---:|
| `typedogma` | 8 | 16 | 24 | 24 |
| `dogmaattributes` | 8 | 72 | 80 | 80 |
| `dogmaeffects` | 8 | 104 | 112 | 112 |
| `dogmaunits` | 8 | 24 | 32 | 32 |
| `dogmaattributecategories` | 8 | 24 | 32 | 32 |
| `requiredskillsfortypes` | 8 | 16 | 24 | 24 |
| `marketgroups` | **4** | 24 | 28 | 28 |
| `npccharacters` | 8 | 128 | 136 | 136 |

`tools/dogma.py` refuses to read a table whose schema and container disagree
on the stride, because a disagreement means one of them is wrong and neither
can be trusted alone.

**[V]** Within a struct the generator orders fields **by descending size, and
alphabetically within a size**: all 8-byte members first (pointers, `double`,
`int64`, interleaved alphabetically — `types` goes `basePrice`, `capacity`,
`designerIDs`, `mass`, `portionSize`, `radius`, `volume`, which is doubles and
pointers mixed in one alphabetical run), then the 4-byte members, then the
1-byte `bool`s, then one `uint32` presence bitfield. The bitfield is the last
field, so its offset is `sizeof(struct) - 4`. A field with a presence bit
reads as `None` when the bit is clear — this is not a convention invented
here, it is the branch the generated getter takes to `Py_None`.

That ordering is worth knowing for a second reason: it means a cFSD schema's
*declaration* order is lost, so there is no "column 1" to speak of. Offsets
are the only identity a field has.

### 1.2 One correction to `fsd.py`'s list convention

**[V]** A pointer field holds a payload-relative offset to the **first
element**, with the `uint64` element count in the eight bytes **before** it.
That is the same convention `fsd.CfsdTable.string` already uses. The generated
sequence code is unambiguous:

```
mov rax, qword ptr [rcx + 0x10]     ; self->data
mov rcx, qword ptr [rax]            ; p = the pointer field
cmp rdx, qword ptr [rcx - 8]        ; index vs *(uint64*)(p - 8)  == count
lea rbx, [rdx + rdx*2] ; shl rbx, 4 ; element = p + index * 48
```

`fsd.cfsd_list` instead takes the slot to point at the *count*, with elements
following at `+8`. The two read the same bytes only when `fsd`'s slot happens
to hold the real pointer minus eight — and §10(c) shows that that is a
coincidence which holds for 5,430 of `dungeons`' 5,516 rows and fails for the
other 86. **A list field is one record slot, not two**, so
`static-corpus.md`'s `listc`/`listd` pair describes one field plus its
neighbour. `tools/dogma.py` implements the loader's arithmetic.

### 1.3 Element strides come from the index arithmetic

**[V]** An element's size is the multiplier in its sequence-item function:
`shl rbx, 4` is 16, `lea rbx, [rcx + rdx*8]` is 8, `lea rbx, [rdx + rdx*2]`
then `shl rbx, 4` is 48. Each such function instantiates a specific generated
element type, and each getattr arm returns a wrapper of a specific type, so
the two are matched by that type's address in `.data`. That matching is the
one link in the chain that took reading rather than pattern-matching; §7's
closure tests would fail outright if it were wrong, since swapping
`typedogma`'s 16-byte and 8-byte element readings makes neither list close
into `dogmaattributes` or `dogmaeffects`.

---

## 2. `typedogma.fsdbinary` — 27,160 rows, the one that matters

The record is two list pointers and nothing else.

```
+0    uint32  typeID                        [V] the bucket key
+8    ptr     dogmaAttributes               [V] list of 16-byte elements
+16   ptr     dogmaEffects                  [V] list of 8-byte elements

dogmaAttributes element, 16 bytes:
  +0  f64     value                         [V]
  +8  uint32  attributeID                   [V] -> dogmaattributes

dogmaEffects element, 8 bytes:
  +0  uint32  effectID                      [V] -> dogmaeffects
  +4  bool    isDefault                     [V]
```

**[V]** All 27,160 rows decode. 648,726 attribute values, 54,307 effect
entries, 2,147 distinct attribute ids and 3,309 distinct effect ids — **every
one of them a key of `dogmaattributes` and `dogmaeffects` respectively, with
zero exceptions** — and every one of the 27,160 keys is a key of `types`.

27,160 of 53,288 types (51%) have a dogma row; 16,204 of those carry at least
one effect. Rows carry between 0 and 137 attribute values.

`isDefault` marks the default member of a mutually-exclusive effect set (the
client's `godma` picks one such effect per item). **[I]** on the semantics;
the field name and type are **[V]**.

**What this table is for.** It is the entire reason to care. A Rifter's three
high slots, its 35 m signature radius, its 350 structure, 450 shield, 450
armor, 250 GJ capacitor, 365 m/s and 41 MW of powergrid are *in here*, as
CCP authored them, keyed by `typeID`. Nothing has to be balanced, invented or
approximated.

---

## 3. `dogmaattributes.fsdbinary` — 2,871 rows

```
+0    uint32  attributeID (bucket key)      [V]
+8    ptr     description           str     [V] optional, bit 0x4
+16   ptr     name                  str     [V] always
+24   uint32  attributeID                   [V] always; == the key, all 2,871
+28   uint32  categoryID                    [V] optional, bit 0x1
+32   uint32  chargeRechargeTimeID          [V] optional, bit 0x2
+36   uint32  dataType                      [V] always
+40   f32     defaultValue                  [V] always  <- float, not double
+44   uint32  displayNameID         msgid   [V] optional, bit 0x8
+48   uint32  iconID                        [V] optional, bit 0x20
+52   uint32  maxAttributeID                [V] optional, bit 0x40
+56   uint32  minAttributeID                [V] optional, bit 0x80
+60   uint32  tooltipDescriptionID  msgid   [V] optional, bit 0x100
+64   uint32  tooltipTitleID        msgid   [V] optional, bit 0x200
+68   uint32  unitID                        [V] optional, bit 0x400
+72   bool    displayWhenZero               [V] optional, bit 0x10
+73   bool    highIsGood                    [V] always
+74   bool    published                     [V] always
+75   bool    stackable                     [V] always
+76   uint32  presence bitfield             [V]
```

Note `defaultValue` is a **`float`**, widened to a double by `cvtps2pd` before
boxing — read it as four bytes, not eight. That is the kind of thing a
calibration pass gets wrong and the generated code states.

Census, all **[V]**: 1,277 of 2,871 attributes are `published`; 244 are not
`stackable` (i.e. they stack with diminishing returns); 288 have
`highIsGood = false`; 536 carry a non-zero `defaultValue`; 8 have a
`minAttributeID` and 29 a `maxAttributeID` (a bound expressed as another
attribute); 1,251 have a `displayNameID`.

`categoryID` closes into `dogmaattributecategories` (37 rows) with no
exceptions, and the categories are the ones the client's Show Info groups by:
*Fitting, Shield, Armor, Structure, Capacitor, Targeting, Miscellaneous,
Required Skills, NULL, Drones, AI, Speed and Travel, Loot, Remote Assistance,
seven EW categories, Turrets, Missile, Graphics, Entity Rewards, Entity Extra
Attributes, Fighter Abilities, EW - Resistance, Bonuses, Fighter Attributes,
Superweapons, Hangars & Bays, On Death, Behavior Attributes, Mining, Heat.*

`unitID` closes into `dogmaunits` (60 rows) with no exceptions.

`dataType` is a small enum, distribution `{0:31, 1:10, 2:28, 3:45, 4:651,
5:1950, 6:2, 7:1, 8:20, 9:5, 10:11, 11:68, 12:46, 13:3}`. **[I]** it is the
storage/display type of the value (4 and 5 dominating, which would be int and
float). The mapping is `unknown`: the client's dogma engine is native and no
`.pyc` in `out/pyc` names these.

---

## 4. `dogmaattributecategories` (37) and `dogmaunits` (60)

```
dogmaattributecategories, stride 32
  +0   uint32  categoryID (key)   [V]
  +8   ptr     description  str   [V] optional, bit 0x1
  +16  ptr     name         str   [V] always
  +24  uint32  presence bitfield  [V]

dogmaunits, stride 32
  +0   uint32  unitID (key)       [V]
  +8   ptr     name         str   [V] always
  +16  uint32  descriptionID msgid[V] optional, bit 0x1
  +20  uint32  displayNameID msgid[V] optional, bit 0x2
  +24  uint32  presence bitfield  [V]
```

**[V]** All 56 `dogmaunits` rows that carry a `displayNameID` resolve in the
English corpus. And 30 of the 34 `unitXxx = N` constants in the client's own
`dogma/const.py` name exactly the row the data holds — `unitLength = 1` ↔
*"Length"*, `unitMass = 2` ↔ *"Mass"*, `unitHitpoints = 113` ↔ *"Hitpoints"*,
`unitTeraflops = 106` ↔ *"Teraflops"*, `unitMegaWatts = 107` ↔ *"MegaWatts"*.
The four that differ are spelling drift in CCP's own constant names, not
offset error (`unitInversedModifierPercent = 111` ↔ *"Inversed Modifier
Percent"* matches; the misses are of the same kind).

---

## 5. `dogmaeffects.fsdbinary` — 3,427 rows, and the modifiers

```
+0    uint32  effectID (bucket key)                 [V]
+8    ptr     effectName                      str   [V] always
+16   ptr     guid                            str   [V] optional, bit 0x80
+24   ptr     modifierInfo                    list  [V] optional, bit 0x200
+32   ptr     sfxName                         str   [V] optional, bit 0x4000
+40   uint32  descriptionID                   msgid [V] optional, bit 0x1
+44   uint32  dischargeAttributeID                  [V] optional, bit 0x2
+48   uint32  displayNameID                   msgid [V] optional, bit 0x4
+52   uint32  distribution                          [V] optional, bit 0x8
+56   uint32  durationAttributeID                   [V] optional, bit 0x10
+60   uint32  effectCategory                        [V] always
+64   uint32  effectID                              [V] always; == the key
+68   uint32  falloffAttributeID                    [V] optional, bit 0x20
+72   uint32  fittingUsageChanceAttributeID         [V] optional, bit 0x40
+76   uint32  iconID                                [V] optional, bit 0x100
+80   uint32  npcActivationChanceAttributeID        [V] optional, bit 0x400
+84   uint32  npcUsageChanceAttributeID             [V] optional, bit 0x800
+88   uint32  rangeAttributeID                      [V] optional, bit 0x1000
+92   uint32  resistanceAttributeID                 [V] optional, bit 0x2000
+96   uint32  trackingSpeedAttributeID              [V] optional, bit 0x8000
+100  bool    disallowAutoRepeat                    [V] always
+101  bool    electronicChance                      [V] always
+102  bool    isAssistance                          [V] always
+103  bool    isOffensive                           [V] always
+104  bool    isWarpSafe                            [V] always
+105  bool    propulsionChance                      [V] always
+106  bool    published                             [V] always
+107  bool    rangeChance                           [V] always
+108  uint32  presence bitfield                     [V]

modifierInfo element, 48 bytes:
  +0   ptr     domain                         str   [V] always
  +8   ptr     func                           str   [V] always
  +16  uint32  effectID                             [V] optional, bit 0x1
  +20  uint32  groupID                              [V] optional, bit 0x2
  +24  uint32  modifiedAttributeID                  [V] optional, bit 0x4
  +28  uint32  modifyingAttributeID                 [V] optional, bit 0x8
  +32  int32   operation                            [V] optional, bit 0x10
  +36  uint32  skillTypeID                          [V] optional, bit 0x20
  +40  uint32  presence bitfield                    [V]
```

`operation` is **signed**: 18 entries hold `-1`, which reads as 4,294,967,295
if taken as unsigned. The generated getter boxes it with `PyInt_FromLong` from
a `mov ecx, dword ptr [...]`, so the sign is the client's reading too.

**The nine attribute-reference columns all close.** **[V]** Every non-null
`dischargeAttributeID` (172), `durationAttributeID` (224),
`falloffAttributeID` (51), `rangeAttributeID` (246), `resistanceAttributeID`
(39), `trackingSpeedAttributeID` (6), `fittingUsageChanceAttributeID` (12),
`npcActivationChanceAttributeID` (19) and `npcUsageChanceAttributeID` (7) is a
key of `dogmaattributes`. Nine independent closure tests, zero exceptions.
A wrong offset does not close nine times.

**`modifierInfo` is the prize.** 5,178 entries across 3,210 effects, and it is
the rule table of the whole bonus system: *this* effect applies *this*
operation to *this* attribute of *that* set of items, scaled by *that*
attribute.

**[V]** `domain` takes exactly seven values — `shipID` (3,603), `charID`
(1,065), `itemID` (221), `structureID` (177), `otherID` (55), `targetID` (47),
`target` (10) — and `func` exactly six: `ItemModifier` (1,800),
`LocationRequiredSkillModifier` (1,611), `OwnerRequiredSkillModifier` (923),
`LocationGroupModifier` (805), `LocationModifier` (29), `EffectStopper` (10).
Those are EVE's own dogma modifier vocabularies, spelled as EVE spells them,
and nothing else appears. **[V]** all 10,336 `modifiedAttributeID` /
`modifyingAttributeID` values are keys of `dogmaattributes`; all 805
`groupID`s are keys of `groups`; all 2,534 `skillTypeID`s are keys of `types`.

Worked example, which is the whole system in one row. Effect 7248,
`shipPBonusROFMF` — a Minmatar frigate bonus, and one of the Rifter's two
effects:

```
domain  shipID
func    LocationRequiredSkillModifier
modifies      attribute  speed            (rate of fire)
by            attribute  shipBonusMF      (the hull's per-level bonus)
operation     6
for items requiring skillTypeID 3302      (Small Projectile Turret)
```

That reads, in prose: *for every module on this ship that requires Small
Projectile Turret, scale its rate of fire by the hull's Minmatar-frigate
bonus.* That is the real rule, from the real client, and we did not write it.

Census, **[V]**: `effectCategory` distribution `{0:2930, 1:145, 2:120, 3:1,
4:98, 5:20, 6:4, 7:109}`; 115 effects are `published`; 1,640 carry a `guid`
(a stable string key, e.g. for scripting); 292 a `sfxName`; 760 a
`descriptionID` and 70 a `displayNameID`, **all of which resolve** in the
English corpus; `distribution` appears on only 75 rows, values 1 and 2.

**`unknown`, stated plainly:**

- **The numeric meaning of `operation`.** Observed `{-1:18, 0:649, 2:213,
  3:5, 4:305, 5:55, 6:3793, 7:129, 9:1}`. The published EVE operand ordering
  (preAssign, preMul, preDiv, modAdd, modSub, postMul, postDiv, postPercent,
  postAssignment) fits the shape and would make `-1` preAssignment and `6`
  postPercent — which the `shipPBonusROFMF` example above is consistent with —
  but **this build's bytecode does not contain those names anywhere** (zero
  hits for `preAssign`, `postPercent`, `modAdd` across `out/pyc`), so there is
  no second source here and it is not claimed. The one entry with `operation =
  9` is outside any published set and is `unknown`.
- **`effectCategory`'s enum.** Same reason: no name table in the bytecode.
- **`distribution`**, **`electronicChance`**, **`propulsionChance`**,
  **`rangeChance`**. Names and types are **[V]**; meanings `unknown`.
- **How the client composes modifiers.** Stacking-penalty order, the role of
  `stackable`, and what `EffectStopper` stops are all in the native dogma
  engine, not in anything decoded here.

---

## 6. `requiredskillsfortypes.fsdbinary` — 9,573 rows, and a dict decoded

This one has **no attribute names in its loader at all**, because the row
value is a bare dict rather than a generated object — so §1's method gives
nothing and the shape came from the dict template's own index arithmetic plus
the data.

```
+0    uint32  typeID (bucket key)   [V]
+8    ptr     skills      dict      [V]
+16   uint64  skillCount            [V] the dict's total entry count
```

**[V]** A cFSD dict is a *nested hash map*: the pointer leads to an array of
bucket pointers (count in the `uint64` at `-8`), and each bucket is an array
of fixed-size entries (count likewise at `-8`). Here the entries are 8 bytes,
`{uint32 skillTypeID, uint32 level}` — which is exactly what the generated
`mp_subscript` reads: `mov ecx, dword ptr [rcx + rdx*8 + 4]` for the value
beside `mov ecx, dword ptr [rcx + rdx*8]` for the key.

**[V]** All 9,573 rows decode; `skillCount` equals the decoded dict's size for
every row; all 438 distinct skill ids are keys of `types` (435 of them in
category 16, *Skill*); all 9,573 keys are keys of `types`.

**And then the check that settles it.** `typedogma` separately carries the
attribute pairs `requiredSkill1..6` / `requiredSkill1Level..6Level`
(attributes 182/277, 183/278, 184/279, 1285/1286, 1289/1287, 1290/1288).
Reconstructing each type's skill map from those attributes and comparing it
with this table's dict gives **9,573 rows in agreement and 0 disagreements** —
two tables decoded by completely different readings (a 16-byte element list in
an arena versus a nested hash map of 8-byte entries) that have to agree and
do, on every row.

Spot values: Rifter → *Minmatar Frigate* 1. Raven → *Caldari Battleship* 1.
Crusader → *Amarr Frigate* 5, *Interceptors* 1. Warrior II → *Drones* 5,
*Light Drone Operation* 5, *Minmatar Drone Specialization* 1. Those are the
published prerequisites.

`unknown`: one entry has `level = 0`, and three of the 438 skill types are in
category 29 rather than 16. Curiosities, not resolved.

`requiredskillsindex.fsdbinary` (421 rows, stride 24) is the reverse index and
is **`unknown`**. It has the same record shape, but its dict's values are
themselves references rather than scalars, and the obvious reading
(`{skillTypeID: {level: [typeID, ...]}}`) produced type ids that do not make
sense next to the skill — *Large Autocannon Specialization* yielding *Large
Cap Battery II*. It was not pursued: nothing needs it, since
`requiredskillsfortypes` inverts in one pass.

---

## 7. How this was validated, and what was calibrated on what

This is the part that decides whether the layouts can be trusted, so it is
stated precisely.

**There is no numeric calibration set.** Not one offset was found by matching
a value against a known EVE number. Every offset, type, name and presence bit
came from the generated loader's own code (§1), and the only thing the data
was used for *before* testing was confirming the record model —
`struct_at + sizeof(struct) == stride`, which is arithmetic over the container
header the `fsd-reader` lane had already established, not a fit to values.
`tools/dogma.py derive <table>` reproduces the layouts from the client.

That matters because of the failure mode `static-corpus.md` recorded: a lane
calibrated `types` against 14 known pairs, called a double `volume`, and it
was `capacity`. That cannot happen here, because the getter for
`types.capacity` says `capacity`.

**The test set, none of which was used to find anything:**

| test | result |
|---|---|
| `attributeID == bucket key` | 2,871 / 2,871 |
| `effectID == bucket key` | 3,427 / 3,427 |
| `dogmaattributes.displayNameID` resolves in en-us | **1,251 / 1,251 (100%)** |
| `dogmaeffects.descriptionID` / `displayNameID` resolve | 760 / 760 and 70 / 70 |
| `dogmaunits.displayNameID` resolves | 56 / 56 |
| `dogmaattributes.categoryID` → `dogmaattributecategories` | all, 0 exceptions |
| `dogmaattributes.unitID` → `dogmaunits` | all, 0 exceptions |
| nine `dogmaeffects` attribute-reference columns → `dogmaattributes` | all nine, 0 exceptions |
| `modifierInfo` modified/modifying attribute ids → `dogmaattributes` | 10,336 / 10,336 |
| `modifierInfo.groupID` → `groups` | 805 / 805 |
| `modifierInfo.skillTypeID` → `types` | 2,534 / 2,534 |
| `typedogma` attribute ids → `dogmaattributes` | 2,147 distinct, 0 exceptions |
| `typedogma` effect ids → `dogmaeffects` | 3,309 distinct, 0 exceptions |
| `typedogma` keys → `types` | 27,160 / 27,160 |
| `requiredskillsfortypes` vs `typedogma`'s `requiredSkillN` attributes | **9,573 agree, 0 disagree** |
| `requiredskillsfortypes.skillCount` vs decoded dict size | all rows |
| `modifier` `domain` / `func` string vocabularies | closed sets of 7 and 6, EVE's own |

**Names, against a second and entirely independent source.** The client's own
`dogma/const.pyc` — *bytecode*, not FSD — carries 1,341 `attributeXxx = N`
constants and 305 `effectXxx = N`. Of the 1,330 whose id exists in the table,
**1,235 name exactly the row the data holds**; the 95 that differ are drift in
CCP's constant names (`attributeCapacitorCharge = 18` is stored as `charge`;
`attributeDroneIsAggressive = 1275` is stored as `droneIsAgressive`, typo and
all). A wrong offset does not agree 1,235 times out of 1,330 with a file read
by a different parser.

**Published EVE values, as a test set.** These are the numbers the *reader*
was pointed at only after the layout was fixed:

- **Rifter (587)**: 3 high / 3 mid / **4** low, 3 turret and 2 launcher
  hardpoints, 3 rigs, signature radius **35 m**, structure 350, shield 450,
  armor 450, capacitor 250, 365 m/s, scan resolution 660, targeting range
  22.5 km, 41 MW powergrid, 130 tf CPU, agility 3.2. Every one is the
  Rifter's published value.
- **Raven (638)**: 7 / 7 / 5, 4 turret and 6 launcher, signature radius 410,
  structure 7,040, shield 7,700, armor 6,380, capacitor 5,500, 113 m/s,
  11,000 MW, 750 tf, 75 m³ drone bay.
- **Attribute ids from `dogma/const.py`**: 280 → `skillLevel`, 276 →
  `skillPoints`, 275 → `skillTimeConstant`, 14 → `hiSlots`, 13 → `medSlots`,
  12 → `lowSlots`, 552 → `signatureRadius`.
- **Effect ids**: 11 → `loPower`, 12 → `hiPower`, 13 → `medPower`.

**Two corrections to the brief that commissioned this lane**, because a brief
that is quietly wrong is worse than one that is corrected: the Rifter has
**4** low slots, not 3, and its signature radius is **35 m**, not 400. (410 is
roughly a battleship's; the Raven's is 410.) The data says 4 and 35, every
other Rifter number in the row is the published one, and the brief's figures
are not in the file under any offset.

---

## 8. What the server can now do that it could not before

This is the section to act on.

**1. `dogmaIM.GetAllInfo` can carry real attributes.** Today
`service.rs::capsule_dogma_row` and `character_dogma_row` send
`("attributes", PyValue::Dict(vec![]))` — an empty dict, which is enough to
get `godma.Prime` past its unconditional read and no further. With
`typedogma` decoded, that dict can be the type's real attribute set, so the
fitting window, Show Info and the HUD show the ship the client already has
art and names for. The client reads attributes off our row for everything but
sublocations (see `item.rs`), so a real dict is believed; an empty one is
also believed, which is why the UI is currently thin rather than broken.

**2. A ship or an NPC built from a real `typeID` fights with real numbers.**
Everything a combat model needs is now readable and joins on integers:
`typedogma` → attribute values; `dogmaattributes` → what each value means,
its unit, whether it stacks and whether high is good; `dogmaeffects` →
what a module or hull bonus *does*; `requiredskillsfortypes` → what a pilot
needs to use it. **Nobody has to author a balance decision.** That was the
single biggest content lever in the client and it is open.

**3. The bonus system is expressible, not just the numbers.** `modifierInfo`
gives 5,178 rules over a closed vocabulary of six `func`s and seven
`domain`s. A dogma evaluator is now a *finite* job: implement six modifier
functions and an operation table, and the 3,427 effects come for free. The
one gap is the operation enum (§5) — nine distinct values whose ordering is
`unknown` from this build, and which a single packet capture of an item with
a known bonus would pin down.

**4. `all_info`'s empty "brain" has a path forward.** The effect lists are
empty today because their elements are server-side objects with
`.GetLiteralKey()` and `.modifierType`. `dogmaeffects` now supplies the
effect identity (`effectID`, `effectName`, `guid`, `effectCategory`) and the
modifier shape those objects wrap, so the shape of what to build is known
rather than guessed.

**5. Skill prerequisites are real.** 9,573 types with their exact skill
requirements, cross-checked two ways. Fitting validation, skill-queue
plausibility and "can this character board this hull" all become lookups.

**6. And the method opens the rest of the corpus.** §1 generalises: the
remaining ~140 cFSD tables are one `tools/dogma.py derive` away from a full,
named, typed, offset-exact schema. That is the largest single piece of
leverage in this document, because it retires the whole "columns unknown"
column of `findings/corpus-inventory.tsv`. In the time it took to write §9 it
has already: confirmed every published `types`/`groups`/`categories` offset
and corrected `portionSize`'s type (§9.1); opened `marketgroups` and
`npccharacters` (§9); named the dungeon spawn/trigger and loot fields that
were recorded as not found (§9.2); and settled the `missions.messages`
dispute, including *why* the old reader worked on 2,851 of 2,892 rows and
which 41 it fails on (§9.3). Four of those are corrections to claims nobody
had reason to doubt, which is the argument for doing the rest.

**What this does *not* give us.** No dogma *engine*: stacking penalties,
effect activation, and how the client composes a modifier chain are in native
code and `unknown`. No state model: these are definitions, not an item's
current attribute values. And nothing here is committed — the tables stay in
the client.

---

## 9. Bonus: two non-dogma tables, to show the method travels

Priority 5 of this lane's brief, done with the same extractor and the same
closure discipline.

```
marketgroups.fsdbinary, 2,114 rows, stride 28 -- struct at +4, not +8
  +0   uint32  marketGroupID (key)   [V]
  +4   uint32  descriptionID  msgid  [V] optional, bit 0x1
  +8   uint32  iconID                [V] optional, bit 0x2
  +12  uint32  nameID         msgid  [V] always
  +16  uint32  parentGroupID         [V] optional, bit 0x4
  +20  bool    hasTypes              [V] always
  +24  uint32  presence bitfield     [V]
```

**[V]** All 2,114 `nameID`s resolve in en-us; `parentGroupID` closes into the
table itself for every non-root row, leaving 19 roots — i.e. the market tree
is complete and acyclic. Samples: 1057 *"Large"* under 537; 2114 *"Capital
Industrial Ships"* under 1968; 2 *"Blueprints & Reactions"*, a root.

```
npccharacters.fsdbinary, 11,424 rows, stride 136
  +0   uint32  characterID (key)     [V]
  +8   ptr     agent          ref    [V] optional, bit 0x1 -- nested object
  +40  ptr     description    str    [V] optional, bit 0x8
  +48  ptr     skills         dict   [V] optional, bit 0x20
  +56  ptr     startDate      str    [V] optional, bit 0x80
  +64  uint32  ancestryID            [V] optional, bit 0x2
  +68  uint32  bloodlineID           [V] always
  +72  uint32  careerID              [V] optional, bit 0x4
  +76  uint32  charisma              [V] always
  +80  uint32  corporationID         [V] always
  +84  uint32  intelligence          [V] always
  +88  uint32  memory                [V] always
  +92  uint32  nameID         msgid  [V] always
  +96  uint32  perception            [V] always
  +100 uint32  raceID                [V] always
  +104 uint32  schoolID              [V] optional, bit 0x10
  +108 uint32  specialityID          [V] optional, bit 0x40
  +112 uint32  stationID             [V] optional, bit 0x100
  +116 uint32  willpower             [V] always
  +120 bool    ceo                   [V] always
  +121 bool    deleted               [V] always
  +122 bool    employmentRecord      [V] always
  +123 bool    gender                [V] always
  +124 bool    uniqueName            [V] always
  +128 uint32  presence bitfield     [V]

  the nested `agent` object (read by derive, not decoded here):
       storyLineMissions list, agentDivisionID, agentLevel, agentType,
       agentLocateCharacter
```

**[V]** All 11,424 `nameID`s resolve to real character names (*"Ansan
Yoosstet"*, *"Riasan Sussal"*); every `corporationID` is a key of
`npccorporations`; 10,972 of the 11,424 carry the nested `agent` object and
422 carry a `skills` dict; the five character attributes sit in the 5–21 range
EVE's NPCs use. `agent` and `skills` were **not** followed — they are `ref`
and `dict` fields whose element layout `derive` names but whose decoding
nobody has needed yet.

---

## 9.1 `types`, `groups`, `categories`: the `fsd-reader` lane, checked

`static-corpus.md` §5 recovered `types`/`groups`/`categories` by profiling and
closure. Running `tools/dogma.py derive` on all three is a free audit of that
work, so here it is. **Every single offset it published agrees.**

```
types.fsdbinary, stride 152, struct at +8 -- 28 fields, all named
  agrees with static-corpus.md on all of: basePrice +8, capacity +16,
  mass +32, radius +48, volume +56, descriptionID +68, groupID +84,
  marketGroupID +96, quoteAuthorID +108, quoteID +112, raceID +116,
  typeID +128, typeNameID +132
  and adds: designerIDs(list) +24, certificateTemplate +64, factionID +76,
  graphicID +80, iconID +88, isisGroupID +92, metaGroupID +100,
  metaLevel +104, soundID +120, techLevel +124,
  variationParentTypeID +136, wreckTypeID +140,
  isDynamicType(bool) +144, published(bool) +145, presence +148

groups.fsdbinary, stride 32, struct at +4 -- all-int32 struct, so +4 not +8
  agrees: categoryID +4, groupID +8, groupNameID +12
  adds: iconID +16, anchorable +20, anchored +21,
  fittableNonSingleton +22, published +23, presence +28
  (the four bytes at +24 are padding the generator left; nothing reads them)

categories.fsdbinary, stride 40, struct at +8
  agrees: categoryID +16, categoryNameID +20
  adds: sofBuildClass(str) +8, iconID +24, published +28, presence +32
```

**One correction, and it is a type rather than an offset.**
`fsd.CFSD_COLUMNS['types']` has `portionSize` as `u32` at `+40`. **[V]** The
getter is `mov rcx, qword ptr [rbx + 0x20]` boxed with `PyLong_FromLongLong`:
`portionSize` is a **signed 64-bit integer**. Reading its low 32 bits is
correct for every value the file actually holds — the observed range is
1..5000 — so nothing downstream is wrong today, but the declared type is, and
a `reprocessing` or `refining` feature that assumed 32 bits would be building
on a coincidence.

**[V]** Also worth recording: `types` carries far more than the lane needed,
including `metaGroupID`, `metaLevel`, `techLevel`, `factionID`,
`variationParentTypeID` and `wreckTypeID` — i.e. tech level, meta level and
faction are a lookup away, not a research question. And `groups.anchorable`,
`groups.anchored` and `groups.fittableNonSingleton` are the three group flags
the client's inventory code branches on.

---

## 9.2 The dungeon content model, read from the loader

Not this lane's table, and **not value-validated** — every line below is
**[V]** as *read out of `bin64/dungeonsLoader.pyd`* and **[I]** as a claim
about what the bytes hold, because no closure or published-value test was run
on it. It is here because it answers two questions `static-corpus.md` recorded
as open, and because re-deriving it costs one command.

`tools/dogma.py derive dungeons` yields eight structs. Record offsets below
assume the struct begins at `+8` for the hash-mapped dungeon row and at `+0`
for the arena records, which are reached by pointer and carry no key.

```
the dungeon row (struct at record +8; stride 120)
  +24 connections list   +32 contentTags list   +40 rooms list
  +56 standingRestrictions list
  +64 archetypeID   +68 descriptionID   +72 difficulty   +76 dungeonID
  +80 dungeonNameID   +84 entryObjectID   +88 entryTypeID   +92 factionID
  +96 gameplayDescriptionID   +100 missionBriefingID   +104 objectivesID
  +108 useDefaultSpaceMusic(bool)   +112 presence

the room record    +32 objects list, +48 roomName, +56 shortName,
                   +64 emptyResetDelay, +68 messageHeaderID,
                   +72 messageTextID, +16 messageIcon, +24 notes

the object record  +16 entities list, +32 groupTag, +40 localName, +48 notes,
                   +56 objectiveTargetGroup, +64 guardCommand,
                   +100 objectID, +104 objectNameID, +108 order,
                   +112 pitch(f32), +128 radius(f32), +132 roll(f32),
                   +136 roomID, +140 typeID, +144 yaw(f32), +148 presence

the entity record  +32 additionalLootId, +36 entityID, +40 entityNameID,
                   +44 npcGroupingID, +48 objectID, +52 triggerSpawn(bool),
                   +0 behaviorName, +8 groupBehaviorName, +16 notes,
                   +24 objectiveTargetGroup, +56 presence

guardCommand       aggressionRange(f32), orbitRange(f32), jitterRadius,
                   entityMinRespawnDelay, entityMaxRespawnDelay,
                   objectMinRespawnDelay, objectMaxRespawnDelay,
                   entityDelaySpawn, entitySpawnOnce, objectDelaySpawn,
                   objectTriggerSpawn

standingRestriction  {corpID, minStandings(f32)}
connection           {allowedShipRaces, allowedShipsList,
                      disallowedFittedTypesList, fromObjectID}
```

**[V]** The object record's `typeID` at struct `+140` is record `+148` for an
arena record reached by pointer, which is exactly where
`fsd.CFSD_OBJECT_COLUMNS` puts it — independent confirmation of the one
column `static-corpus.md` claimed there, and the first time the rest of that
record has a name.

Two of `static-corpus.md`'s open questions are answered by names:

- **"Spawn groups and triggers. Not found."** They are
  `entity.triggerSpawn`, `entity.npcGroupingID` and the whole
  `guardCommand` struct (`objectTriggerSpawn`, `entityDelaySpawn`,
  `entitySpawnOnce`, respawn delays, aggression and orbit ranges). A dungeon
  object has an `entities` list, and each entity carries a behaviour name, a
  grouping id and a trigger flag.
- **"No loot table was found."** `entity.additionalLootId` is a loot
  reference on the spawn record. Where it points is `unknown` —
  `crates.fsdbinary` and `grouprewards.fsdbinary` remain the candidates and
  are each one `derive` away.

**[I]** `static-corpus.md`'s `dungeons.typeID @+88` is `entryTypeID`, its
"prose A" `@+96` is `gameplayDescriptionID`, and its `objectiveTextID @+100`
is `missionBriefingID`. All three offsets match; only the names are new.

**`unknown`:** the object record's **position**. Offsets struct `+116`,
`+120` and `+124` are unaccounted for and sit between `pitch` and `radius`,
which is where three `f32`s would go, but the extractor recovers no name for
them — most likely because they are called `x`, `y`, `z` and a one-character
string literal is below its string-scanner's two-character floor. That is a
specific, cheap thing for the next person to confirm.

---

## 9.3 `missions.messages`: the `agents-missions` lane is right

The coordinator relayed a sibling lane's report that `static-corpus.md` §6.2
has the mission message record wrong — that it is two `uint64`s, the first a
pointer to a name string, not two `uint32`s — and that `tools/fsd.py`
mis-resolves a handful of those pointers into adjacent variable-length data.
Both are **confirmed**, independently, and the second is now quantified.

**[V]** `tools/dogma.py derive missions` puts `messages` at struct `+64`,
i.e. **record `+72`** — not at `+56`/`+64`. Its entries are 16 bytes,
`{char* label, uint64 messageID}`.

**[V]** Decoding it as a cFSD dict (§6: pointer → bucket-pointer array →
buckets of entries) gives **17,268 entries over 2,892 missions, every single
one of whose values is a message id present in the English corpus** — 100%
resolution, 17,268 for 17,268. And the message id the `localization-corpus`
lane supplied adversarially, **145165, occurs exactly once** as a value, which
is the same single occurrence `static-corpus.md` reported finding by scanning
the arena.

The keys are better than "a uint32 key": they are **the prose slot's name**.
27 distinct labels, with counts:

```
messages.mission.briefing                         2885
messages.mission.offered.agentsays                1983
messages.mission.accepted.agentsays               1432
messages.mission.completed.agentsays              1292
messages.mission.declined.agentsays               1015
messages.mission.quitted.agentsays                 934
messages.mission.accept.charsays                   919
messages.mission.ask.charsays                      916
messages.mission.complete.charsays                 823
messages.mission.completed.nextmission.agentsays   692
messages.mission.decline.charsays                  626
messages.mission.extrainfo.body                    582
... and fifteen more (quit.charsays, timeout.emailbody,
    offerexpired.email.header, ...)
```

So a mission's briefing is `messages['messages.mission.briefing']`, by name,
and the agent's offer, acceptance, refusal, completion and timeout email are
each addressable the same way. That is a materially better join than an
opaque integer key.

**Why `fsd` worked anyway, and exactly when it does not.** **[V]** `messages`
is at record `+72`; `extraStandings` is at record `+56`. For 2,851 of the
2,892 missions `u64(+72) == u64(+56) + 8`, by the same arena-adjacency
coincidence as `dungeons` (§10(c)): an empty vector's element pointer lands
one word before the next vector's count. **The 41 missions where it fails are
exactly the 41 that have a non-empty `extraStandings`** — the presence bit
`0x80` is set on 41 rows, and the equality fails on 41 rows. That is the
"handful of pointers resolving into adjacent variable-length data" the sibling
lane hit, named and counted.

**[V]** The same derivation confirms four of `static-corpus.md`'s missions
offsets exactly — `contentTemplateKey` `+48` (CCP calls it
`contentTemplate`), `npcCorporationID` `+92` (`corporationID`), `factionID`
`+120`, `missionNameID` `+196` (`nameID`) — and names the columns that lane
listed only as shapes: `agentTypeID` `+88`, `divisionID` `+112`,
`expirationTime` `+116`, `fixedLpRewardAlpha` `+124`,
`fixedLpRewardOmega` `+128`, `initialAgentGiftTypeID` `+136`,
`initialAgentGiftQuantity` `+132`, `nodeGraphID` `+200`,
`solarSystemID` `+204`, `stationID` `+208`,
`hasStandingRewards` `+212`, plus nested `courierMission`, `killMission`,
`missionRewards` (`{rewardTypeID, rewardQuantity}` and a
`{bonusReward, bonusTimeInterval, reward}` wrapper), `clientObjectives`
(`{objectiveTypeID, objectiveQuantity, objectiveSingleton}`) and
`contentTags`.

**[I]** `static-corpus.md`'s `dungeonID @+144` is probably the first field of
the inline `killMission` object, which the extractor places around struct
`+132`–`+136`; a kill mission needs a dungeon. **Not confirmed** — the
extractor leaves several 8-byte slots in this table unnamed, so the honest
statement is that `+144` was not re-derived and the lane's closure test (1,461
distinct values, all keys of `dungeons`) is the evidence for it, not anything
here.

---

## 9.4 Two independent extractors, and they agree

A sibling lane arrived at §1's method in the same round and built
`tools/pyd_schema.py` (1,215 lines, run over all 149 loaders, written up in
`docs/re/native-loaders.md`). Neither extractor saw the other. Comparing
`findings/pyd-schemas.tsv` against `tools/dogma.py`'s committed schemas:

**[V]** For `dogmaattributes` and `dogmaeffects` the two agree on **every
field: name, record offset, width, and presence bit** — all 18 and all 27,
including `defaultValue` as an `f32` at `+40` and both presence bitfields at
`+76` and `+108`. For `typedogma` they agree on the record and on
`dogmaAttributes @+8` / `dogmaEffects @+16`. And `pyd_schema.py` derives the
record model independently, calling `struct_at` a `delta` and reading it from
the `lea` immediate in the row factory rather than from alignment arithmetic —
two different derivations, same answer of 8.

Two differences, both of labelling rather than substance: that tool reports
`i32` and `u8` where this one reports `u32`/`msgid` and `bool` (same widths;
the `msgid` tag here is semantic, and §7's 100%-resolution tests are what
justify it), and it reports the three list fields as `list<unknown>` or
`list<struct:unnamed_...>`. **The element layouts and strides in §2 and §5 of
this document are the piece that fills those in** — `modifierInfo`'s 48-byte
record is the one that matters, since without it `dogmaeffects` is a table of
names.

That is the best available evidence that §1 is sound: two readers, written
from scratch against the same binaries, producing identical offsets on 45
fields across two tables. It also means the dogma schemas are recorded twice
in this repository; `tools/dogma.py` is the one with a *reader* behind it and
the one §7's validation runs against.

---

## 10. Suggested patch to `tools/fsd.py`, for the coordinator

This lane does not own `tools/fsd.py`, so the changes it would want are
written here as a diff instead. None is required for `tools/dogma.py` to
work; all three remove a sharp edge.

**(a) A public row iterator.** Every consumer — `fsd.cfsd_rows`,
`fsd.accounting_entry_types`, `fsd.dungeon_rooms` and now `tools/dogma.py` —
calls `CfsdTable._locate()` with a `noqa: SLF001`. Make it public:

```diff
-    def _locate(self):
+    def records(self):
+        """``(key, payload-relative record offset)`` for every row."""
         stride = self.stride
         if stride is None:
             raise FsdError("%s: record stride unknown" % self.name)
         for b, (o, c) in enumerate(zip(self.offsets, self.counts)):
             for i in range(c):
                 r = o + i * stride
                 yield self._u32(r), r
+
+    _locate = records   # keep the old private name working for one round
```

**(b) Record the record model in the docstring.** `CfsdTable`'s docstring says
"A record is a fixed number of bytes (`stride`), the same for every row of a
table, and begins with `uint64 key`. The rest is the row, and **its field
layout is `unknown`**". Two corrections: the key is a `uint32` (the class
already reads it with `_u32`; the prose lags), and the layout is no longer
unknown — it is `uint32 key`, padding to the row struct's alignment, then the
struct, per §1.1 here.

**(c) `fsd.dungeon_rooms` is wrong for 86 dungeons, and the list convention
is why.** This one is a bug, not a tidy-up, and it is the clearest
demonstration that the generated code is worth reading.

**[V]** `tools/dogma.py derive dungeons` says the dungeon row struct's list
fields are `connections` at struct `+16`, `contentTags` at `+24`, `rooms` at
`+32` and `standingRestrictions` at `+48` — so with the struct at record `+8`,
**`rooms` is a single slot at record `+40`**, and record `+32` is
`contentTags`. `CFSD_COLUMNS['dungeons']` instead has
`rooms listc @+32` / `rooms listd @+40`, and `fsd.dungeon_rooms` reads
`cfsd_list(table, r + 32)`.

Why that works anyway, almost always: the generator writes each vector into
the arena as `[uint64 count][elements...]` and writes them back to back. When
`contentTags` is **empty**, its element pointer points one word past its own
count — which is exactly where `rooms`' count then sits. So
`contentTags_ptr + 8 == rooms_ptr`, and `cfsd_list` at `+32` reads the right
bytes by accident.

**[V]** 87 of 5,516 dungeons have a non-empty `contentTags` (presence bit
`0x4`). For 86 of them `u64@+40 != u64@+32 + 8`, and `fsd.cfsd_list(t, r+32)`
returns garbage — for dungeon 12454 it reports a list of 644,680 elements
where the real room list has 2. So:

The minimal fix is a helper that reads a list the way the loader does —
`p = u64(slot)`, `n = u64(p - 8)`, elements from `p` — and a `rooms` entry at
`+40` in place of the `listc`/`listd` pair at `+32`/`+40`.

**But do not patch this offset by offset.** §9.2's derivation also puts the
*room* record's `objects` list at room struct `+32`, where
`fsd.CFSD_ROOM_COLUMNS` has `listc @+8` / `listd @+40`, and names none of the
three 8-byte fields at room struct `+0`, `+8` and `+40` — so
`CFSD_ROOM_COLUMNS`' `roomID @+0` is reading something the loader does not
call `roomID`. At the same time, `dungeon_rooms`' *results* were
cross-validated against `asteroidcountbytypeidroomiddungeonid.static` and
agreed on the room id space, and `CFSD_OBJECT_COLUMNS`' `typeID @+148` is
confirmed exactly by §9.2. So something in there is self-consistent and the
discrepancy deserves one careful sitting with
`tools/dogma.py derive dungeons` beside the data — in particular the question
of whether the rooms list holds *pointers to* room records or **inline**
room structs, which is what would reconcile the two readings. This lane did
not resolve it and does not guess.

The dungeon *contents* `static-corpus.md` reported are not in doubt either
way: the rooms and objects it quotes are from dungeons with an empty
`contentTags`, and they were checked against a second file.

---

## 11. Scope note

Nothing from the client is committed. The three `findings/corpus-*.tsv` files
carry each table's *shape* — field, offset, type, presence bit — plus a
handful of sample rows whose only job is to show the reader produces the right
values. The sample values quoted in this document are there for the same
reason. Anyone who wants the data obtains the client themselves, legally, as
the owner's standing rule requires.

---

## 12. The generated row layout is predictable from the field names

Added 2026-10-21, deriving the five tables `docs/port-tracker.md` §15.6 asked
for: `types`, `typedogma`, `dogmaattributes`, `schematics`, `globaltypes`.
Three of them produced a result bigger than the table.

**The rule, and it held on every table with a committed schema.** The
generated C++ row struct orders its fields by *size class first, then
alphabetically within the class*:

1. 8-byte fields (pointers, `f64`, `i64`) — alphabetical
2. 4-byte fields (`i32`/`u32`/`f32`) — alphabetical
3. `bool` fields — alphabetical
4. the `uint32` presence bitfield

and the presence **bits** are assigned alphabetically over the gated fields,
across all three size classes at once, starting at `0x1`.

Checked against four independently derived tables: `types` (28 fields),
`dogmaeffects` (27), `dogmaattributes` (18) and `marketgroups` (5). No
exceptions.

**Why this is worth writing down.** It turns the derivation from "disassemble
each loader" into "predict, then check against the stride", and it makes a
derived layout *falsifiable in two ways*:

- a missing 8-byte, 4-byte or `bool` field shows up as a gap in the
  alphabetical run of its class
- a missing **gated** field shows up as a gap in the presence bits, because
  the bits are contiguous from `0x1`

### 12.1 The `types` hole, and why it is not a missed column

`typesLoader.pyd` yields 28 fields and a struct of 144 bytes, which
reconciles: `8 + 144 = 152` is the stride the container reports. But there is
a 4-byte hole at struct `+0x40` (record `+72`), between `descriptionID` and
`factionID`.

Both rules say nothing of ours is absent. The 25 presence bits run
`0x1`..`0x1000000` with **no gap**, so a 26th gated field would have taken
`0x2000000` and there is none; and the 4-byte class is alphabetically
complete. So the hole's owner has **no presence bit** and a name sorting
between `descriptionID` and `factionID` — a field with no Python getter, or
padding left behind by one that was removed. Either way it is not a column a
reader of this tree should go looking for.

This is the kind of thing worth stating positively. "There is a hole" invites
the next reader to hunt for a column; "there is a hole and here are the two
reasons no column is missing" does not.

### 12.2 `schematics` — 68 rows, the PI recipes

Struct 32 bytes, `8 + 32 = 40` = the stride.

| field | at | kind |
|---|---|---|
| `pins` | `+0` | **list** of facility `typeID` (eight, in every row seen) |
| `types` | `+8` | **dict** `{typeID: {quantity, isInput}}` |
| `typeCount` | `+0x10` | the dict's total entry count |
| `cycleTime` | `+0x18` | seconds |
| `nameID` | `+0x1c` | message id |

`typeCount` beside the pointer is the same shape `requiredskillsfortypes`
has (§6), which is the second table to show it: **a dict field carries its
total entry count in the record slot after the pointer**, while a list field
carries its count in the payload at `pointer - 8`.

A dict *entry* is the record model one level down — key, padding to the value
struct's alignment, then the struct. Here the value struct is all 4-byte, so
an entry is `4 + 8 = 12` rather than `8 + 8`.

Checked against the recipes rather than only against the stride:

- key 84: inputs 2395 and 2396 at 40 each, output 3725 at 5, cycle 3600s
- key 105: inputs 3725 and 3775 at 10 each, output 28974 at 3, cycle 3600s

That is a P2 consuming two P1s, and a P3 consuming two P2s. The `isInput`
flag distinguishes the two sides, and every row seen has exactly one output.

### 12.3 `globaltypes` — 8 rows, and the name is the misleading part

Struct 16 bytes, `8 + 16 = 24` = the stride. Two fields: a dict pointer at
`+0` and its entry count at `+8`.

The eight keys are **11, 12, 13, 2014, 2015, 2016, 2017 and 2063 — planet
type ids**, and each row holds fifteen `{resource typeID: weight}` pairs, with
`resourceCount == 15` for all eight. This is the **planetary-interaction
resource distribution**, from the client rather than from a wiki, and it lands
squarely in the PI lane's area.

Entry layout: 4-byte key, 4 bytes of padding, `f64` value — alignment 8, so
the entry is 16. Planet type 12, for example, reads 2268 at 0.95, 2272 at
1.0, 2286 at 0.95, 2310 at 0.9 and the remaining eleven at 0.0.

### 12.4 What this changed in the tree

- `tools/dogma.py` `SCHEMAS` gained `types`, `schematics` and `globaltypes`.
  `typedogma` and `dogmaattributes` were already there, and the derivation
  **confirmed both unchanged** — which is the useful kind of null result,
  because those two are what `skill.rs`'s ranks are read through.
- `Table._dict` learned a **struct-valued** dict and an explicit value offset.
  The one-scalar form could not express `{typeID: {quantity, isInput}}`, and
  the hardcoded `+4` was right only for 4-byte values.
- `Table._scalar` learned `i64` (`types.portionSize`).
- `tools/fsd.py` learned presence bits: `CFSD_PRESENCE` says where the
  bitfield is, a column may carry a mask, and a clear bit now yields `None`
  rather than whatever is in the struct. A column with a mask in a table with
  no presence entry is an **error**, not a column read anyway.
- `CFSD_COLUMNS["types"]` went from the 15 columns profiling had found to all
  27 scalar columns, with names. The 15 agreed offset for offset — including
  `portionSize` being an `i64`.

Thirteen newly named `types` columns are the practical gain: `published`,
`metaGroupID`, `metaLevel`, `techLevel`, `iconID`, `graphicID`, `factionID`,
`soundID`, `isisGroupID`, `variationParentTypeID`, `wreckTypeID`,
`certificateTemplate`, `isDynamicType`.

Spot-checked against published values, because a layout that reconciles can
still be read wrong: Rifter mass 1,067,000 / volume 27,289 / cargo 140 /
group 25, Dominix group 27, Avatar volume 155,000,000, Veldspar
`portionSize` 100, Tritanium volume 0.01. `tools/dogma.py validate` is green,
0 failures.
