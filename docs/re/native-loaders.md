# The native cFSD loaders: recovering every table's record layout

What this is: how the field layout of all 149 `res:/staticdata/*.fsdbinary`
tables was read back out of the generated `bin64/*Loader.pyd` extensions,
mechanically, by one extractor; what that produced; and how hard it was
checked. Client build V24.01 (3619879).

Tooling: [`tools/pyd_schema.py`](../../tools/pyd_schema.py) is the extractor
and carries the PE and getter-shape reasoning in its module docstring.
[`findings/pyd-schemas.tsv`](../../findings/pyd-schemas.tsv) is the output.
Read [`docs/re/static-corpus.md`](static-corpus.md) §3.4 first for the
container, and [`docs/re/dogma.md`](dogma.md) for a second, independent
implementation of the same idea on eight tables.

Claims are marked **[V]** (read out of the binary and reproduced across all
149 loaders, or checked against something outside the loader) and **[I]**
(inferred). Where neither applies this document says `unknown`.

---

## 1. Headline

**The method works, and it is general.** One extractor, run over
`bin64/`, recovers a named, typed, offset field layout for **139 of the 149**
`.fsdbinary` tables — **1,030 record fields** plus **771 fields across 225
nested record types** — with **zero fields typed `unknown`**.

The remaining 10 are not failures: **[V]** they have no `tp_getattro` anywhere
in the module, because their row is a bare scalar or a bare list rather than a
struct, so there is no field to name. They are `agenttypes`,
`challengeconditions`, `challengeevents`, `compressibletypes`,
`manufacturableitems`, `requiredskillsfortypes`, `requiredskillsindex`,
`ship_skin_design_component_point_values`,
`ship_skin_design_components_post_process` and
`ship_skin_design_tier_thresholds`.

**It reproduces all six previously confirmed layouts independently**, and it
agrees field-for-field with the `dogma-corpus` lane's hand decoding of
`typedogma`, `dogmaattributes` and `dogmaeffects` — two separately written
extractors, same answer, including presence bits and nested element strides.

| | before tonight | now |
|---|---:|---:|
| tables with a per-field layout | 6 | **139** |
| tables with a record stride | 121 | **138** |
| named record fields | ~60 | **1,030** (+771 nested) |
| nested record types named | 0 | **225** |

**[V] 69 of the 71 `*ID` columns in the corpus close 100% into the cFSD table
they name**, over every row of every table; the two that do not are explained
in §4.2 and neither is an offset. That sweep was added after the coordinator
used the TSV and reported `npccorporations` as wrong — it was not; the read
had applied `delta` twice. See §4.3, and the one-line contract: **offsets in
this file are record-relative and already include `delta`.**

---

## 2. The method, in one page

The full reasoning is in `tools/pyd_schema.py`'s docstring. The short form,
because the first plan does not survive contact with the binary:

**[V] There is no `PyGetSetDef` table to find.** `docs/re/static-corpus.md`
§7 and the brief for this lane both assumed one getter function per attribute,
reachable through a `{name, getter, setter, doc, closure}` array. That array
does not exist in these DLLs. `tp_getset` is null in every generated type
object; the only pointer-to-name arrays in `.data` are `PyMethodDef`s (32
bytes, `ml_flags` 1 or 4 in the third slot) for `load`, `__dir__`, `keys`,
`values`, `items`, `get`, `has_key` and the iterators.

**[V] Instead there is one `tp_getattro` per record type, holding the whole
attribute set as a linear comparison chain.** The names are 8-byte-aligned
NUL-padded C strings in `.rdata`, referenced only by `lea reg, [rip+disp]`
from inside that one function — which is exactly why `fsd_dump.py names` could
scrape them but could not order or type them. Each arm is the same shape:

```
lea   rdx, [rip + N]                     ; N -> the attribute name
call  <strcmp>                / or an inlined movzx/cmp byte loop
jne   <next attribute>          ending in `cmp reg, len+1`
test  byte ptr [BASE + F], MASK          ; optional fields only: presence bits
je    <return Py_None>
mov   ecx, dword ptr [BASE + K]          ; the value
jmp   PyInt_FromLong                     ; the box
```

so the parser anchors on each name `lea`, then inside that arm takes the
`test`-ed presence word, the last base-relative access that is not the
presence word, and the boxing import. `BASE` is whichever register the
prologue loads `self->data` into (`mov BASE, qword ptr [rcx+0x10]`); it
differs between loaders, and in several of them the register that holds
`self->data` in one DLL holds the *attribute name* in the next, which is why
the parser rejects any memory operand with an index register.

**[V] The box names the width and kind.** Across all 149 loaders the python27
imports that can box a field value are exactly five: `PyInt_FromLong`,
`PyFloat_FromDouble`, `PyLong_FromLongLong`, `PyString_FromStringAndSize` and
`PyUnicodeUCS2_FromUnicode` (one loader, `dungeons`). Paired with the load
instruction they give `f64 f32 i64 i32 u16 u8 str wstr`. **[V]** No loader
imports `PyBool_FromLong`, so nothing in the binary says "bool": a 1-byte
field is emitted as `u8` with "probably bool" as a note, not a claim.

**[V] Containers and nested structs go through `_PyObject_New(T)`**, usually
by tail call into a one-line helper, with the record pointer offset by a `lea`
(or `mov rcx, BASE` when the offset is zero). `T`'s `tp_name` says which:
`cfsd.list`, `cfsd.dict`, or `<module>.<struct>` for a nested record whose own
`tp_getattro` is another chain, parsed the same way, recursively. A list's
element type comes from that list type's `sq_item`; a dict's key and value
types from that dict type's `mp_subscript`. **[V]** A `position`-style field is
a `PyStructSequence` filled in a counted loop over consecutive slots
(`movss xmm0, dword ptr [rbx + rdi*4 + 0x74]`, `cmp rdi, 3`), and the member
names come from the `PyStructSequence_Desc` — which is how `vec3<f32>` fields
get recovered with their `x,y,z` member names.

**[V] The stride and the base delta are literal in the row factory.** The
offsets in `tp_getattro` are *not* record offsets: the row object's
`[self+0x10]` points past the record's `uint32` primary key, padded up to the
first field's alignment. The factory states both numbers:

```
imul rbx, rcx, 0x98                 ; types: stride 152
add  rbx, qword ptr [rax + rdx*8]   ; + the bucket base
lea  rcx, [rbx + 8]                 ; types: delta 8
mov  qword ptr [rax + 0x10], rcx
```

The stride is often folded into address arithmetic instead
(`lea rcx,[rcx+rcx*4]` then `lea rbx,[rax+rcx*8]` is a multiply by 40), so it
is **also** computed from the recovered layout as
`align_up(delta + span, align)`. Both are reported and `pyd_schema.py check`
lists any disagreement; there are none.

**[V] `delta` is 8 for 125 tables, 4 for 12, and 0 for one.** It is 4 exactly
when every field is 4 bytes or smaller (`groups`, `marketgroups`), 8 when the
struct contains a pointer, `double` or `int64`, and 0 for `effectsequences`,
whose records are list elements reached by pointer and carry no key to skip.
This is the same rule `docs/re/dogma.md` §1.1 states, arrived at separately.

### 2.1 Two traps worth recording

**[V] MSVC splits one function across several `.pdata` ranges.** The
`dungeons` object-record chain is three `RUNTIME_FUNCTION` entries and
`typeDogmaLoader`'s list indexer is two; a parser that stops at the first
range loses two thirds of the object record and reports
`dogmaEffects: list<unknown>`. Consecutive ranges are joined while the next
one's start address is not *referenced* anywhere — not a call or jump target,
not a pointer in `.data`/`.rdata`. A continuation range is by definition
unreachable except by falling into it, so that test is exact. Joining on
"the previous range did not end in `ret`" is *not* exact and misses the
`dungeons` case, because each arm of a `tp_getattro` ends in `ret`.

**[V] Type objects are not all in `.data`.** Many generated type objects are
zero-initialised, with `tp_name` and the sequence/mapping slots assigned at
run time by the module's init code. They are invisible to a `.data` scan, so
`pyd_schema.py` also builds a module-wide map of
`lea reg,[rip+V]` / `mov qword ptr [rip+S], reg` assignments and reads slots
through it. Without that, `dungeons`' nested room, object and entity records
cannot be reached at all.

---

## 3. The schema file

`findings/pyd-schemas.tsv`, six columns, one row per table per field.
**Offsets in the `offset` column are record-relative** — `delta` is already
added — so a reader can use them directly.

| column | meaning |
|---|---|
| `table` | the `.fsdbinary` stem, lowercase |
| `field` | CCP's own attribute name; `#record` for the per-table header row; `struct:<Name>/<field>` for a nested record type's field; `-` for a table with no attributes |
| `offset` | record-relative byte offset, or struct-relative for a `struct:` row |
| `type` | see below |
| `confidence` | `V` or `I` |
| `how_derived` | the instruction evidence, plus the optional marker |

The `#record` row carries `stride=<N> delta=<M>` in the `type` column.

Types: `f64 f32 i64 i32 u16 u8` scalars; `str` (an arena pointer with a
`uint64` byte length in the eight bytes before the data) and `wstr` (the same,
UTF-16); `list<T>`; `dict<K,V>`; `struct:<Name>`; `vec2<T>`/`vec3<T>`/`vec4<T>`
for struct-sequence vectors; `structseq` when a struct sequence's arity could
not be read (none at present). `unknown` would appear if a field's access
pattern were unrecognised; none is.

An optional field's `how_derived` ends with a strict, machine-readable marker:

```
optional(bit=0x40,word=+112)
```

The field is present when `u32_at(record + 112) & 0x40`, and the generated
getter returns `None` when it is clear — that is the branch in the binary, not
a convention invented here.

**[V] A `dict` field occupies 16 bytes of the record and a `list` field 8.**
Read off the gaps between consecutive fields in `dungeons` and `missions`
(every dict is followed 16 bytes later, every list 8), and confirmed against
the generated `keys()` code, which reads the data pointer at `data+0` and a
`uint32` count at `data+8`. `dungeons.rooms` is a dict at `+40` whose count at
record `+48` is 2, 3, 1, 1 and 6 for the first four dungeons and dungeon 47 —
i.e. real room counts. See §5.2.

---

## 4. Validation

Separated deliberately, because the cautionary tale in this lane's brief is a
lane that calibrated on known pairs and then validated on its own sample.

### 4.1 Calibration set — things looked at while building the extractor

- **`typesLoader.pyd`'s chain**, by hand, against the `fsd-reader` lane's
  data-confirmed `types` offsets. This is what established that `delta`
  exists and is 8 there.
- **`groupsLoader` and `categoriesLoader`**, which is what established that
  `delta` varies (4 vs 8) and that the row factory is often split in two.
- **`accountingEntryTypesLoader`**, for the string-field shape.
- **`dungeonsLoader`, `missionsLoader`, `effectSequencesLoader`,
  `typeDogmaLoader`**, for containers, nested types, split `.pdata` ranges and
  struct sequences.
- **The aggregate count of stride agreements** against
  `findings/corpus-inventory.tsv`, used as a regression signal while fixing
  three bugs (dict field width, record alignment, and thin wrappers voting on
  the delta). This is calibration, not a test, and is reported as such.

### 4.2 Test set — run once, after the extractor was frozen, all passed

**Published EVE values**, decoded from `types.fsdbinary` with the generated
schema and nothing else. Not used to derive any offset:

| item | typeID | groupID | mass | radius | capacity | volume | basePrice |
|---|---:|---:|---:|---:|---:|---:|---:|
| Rifter | 587 | 25 | 1,067,000 | 31 | 140 | 27,289 | 400,000 |
| Tritanium | 34 | 18 | 0 | 1 | 0 | **0.01** | 2 |
| Veldspar | 1230 | 462 | 4,000 | 1 | 0 | **0.1** | 2,000 |
| Drake | 24698 | 419 | 13,500,000 | 263 | 450 | 252,000 | 40,400,000 |
| Dominix | 645 | 27 | 100,250,000 | 250 | 750 | 454,500 | 153,900,000 |
| Avatar | 11567 | 30 | 2,400,000,000 | 6,800 | 11,250 | 155,000,000 | 60,000,000,000 |

Every published number in this lane's brief matches: Rifter group 25,
category 6 (through `groups`), mass 1,067,000 kg, radius 31 m, capacity
140 m³; Tritanium volume 0.01. One small correction to
`static-corpus.md` §5: the Avatar's mass in this build is **2.4e9**, not
2.43e9. Both lanes read the same eight bytes, so the doc's figure is a typo,
not a layout question.

**[V] The whole `typeID -> groupID -> categoryID -> English name` chain
resolves**, decoded end to end with the generated schemas for all three
tables: group 25 `Frigate` -> category 6 `Ship` (so the Rifter is group 25 /
category 6, as the brief says), group 18 `Mineral` -> 4 `Material`, group 462
`Veldspar` -> 25 `Asteroid`, groups 27 `Battleship` / 30 `Titan` / 419
`Combat Battlecruiser` -> 6 `Ship`.

**Foreign-key closure.** `types.groupID` closes into `groups` for
**53,288 / 53,288** rows, zero exceptions; `groups.categoryID` into
`categories` for **1,611 / 1,611**, zero exceptions. A wrong offset does not
close.

**Foreign-key closure on every `*ID` column, corpus-wide.** Added at the
coordinator's suggestion after the `npccorporations` report in §4.3, and it is
the strongest single test in this document. Every `i32` field whose name ends
in `ID` and whose target cFSD table can be identified was decoded for every
row of every table and tested against that table's keys: **71 columns, and 69
close at exactly 100%.** The two that do not are explained, and neither is an
offset:

- **`character_resources.typeID`**: 1,112 / 1,147 close. The 35 exceptions
  carry just two distinct values, `-1` (a sentinel) and `4055`, a typeID that
  is not a key of `types` in this build.
- **`dungeons.objectivesID`**: `unknown` target. Its 330 values run 2..106 and
  close 88% into `objectivetypes`, 38% into `objectivechains` and 5% into
  `objectives`, which is not good enough to name. The *offset* is corroborated
  by its neighbours — it sits in a dense run of `i32` fields and the record's
  span closes to the stride — so this is a question about what the id
  references, which §6 already says the binary does not state.

**[V] Three of those 71 were my own naming guess being wrong, not the
schema**, and they are worth recording because they are the obvious way to
mis-read this TSV:

- **`dungeons.entryTypeID` closes 100% into `types`** (5,499 / 5,499), not
  into `accountingentrytypes`. It is the typeID of the dungeon's entry object
  — which is also why `fsd-reader` called record `+88` "typeID".
- **`dogmaattributes.categoryID` closes 100% into `dogmaattributecategories`**
  (2,684 / 2,684), not into `categories` — 80% closure into the wrong table is
  exactly the sort of plausible-looking near-miss this test exists to expose.
- **`types.factionID` is polymorphic.** 1,324 of 1,383 values are keys of
  `factions`; **all 59 of the rest are keys of `npccorporations`**. So the
  column closes 100% into `factions ∪ npccorporations` and 95.7% into
  `factions` alone. An owner id, not a faction id, whatever CCP named it.

**Localisation closure.** Every `i32` field the extractor named `*NameID`,
`*nameID` or `*DescriptionID`, in every table with a stride, resolved against
the English corpus via `tools/loctable.py`'s reader:
**81,763 of 81,768 non-zero message ids resolve — 99.994%.**
`types.typeNameID` is 53,288/53,288 (100%), `groups.groupNameID` 1,611/1,611,
`categories.categoryNameID` 48/48. `typeID 587 -> 'Rifter'`,
`34 -> 'Tritanium'`, `11567 -> 'Avatar'`, `1230 -> 'Veldspar'`.

**[V] The five that do not resolve are dangling ids in CCP's data, not a wrong
offset.** They are `dungeons` 13874, 13875, 13876, 13877 and 14128, whose
`dungeonNameID`s (1034114, 1034120–1034122, 1042153) are in neither
`localization_fsd_en-us.pickle` nor the `main` label index. Partial resolution
on *one field of one table* with 100% on every other field is a data property;
a wrong offset gives partial resolution everywhere.

**Whole-corpus sweeps.** Decoding every row of all 137 tables whose container
also opens:

- **String pointers: 114,860 of 114,861 resolve** to a length-prefixed arena
  string when the presence bit is set. The single exception is
  `effectsequences.name` — a one-row table whose slot may simply hold an empty
  string; see §6.
- **`f64` values: 639,653 read, 317 implausible** (NaN or `|v| > 1e30`), all
  of them `types.mass`. The `fsd-reader` lane's confirmed layout puts `mass`
  at the same offset, so this is EVE's data, not a decoding error.

### 4.3 The `npccorporations` report: the schema is right, the read double-counted `delta`

The coordinator used the TSV and reported that `npccorporations` looked wrong:
corp 1000126's presence word read `0x0`, and no corporation had a `factionID`
in the plausible 500001.. range — offset 160 gave `1542`. Checked, and
**reproduced from the opposite direction: the layout is correct.**

**[V]** The `offset` column of this TSV is **record-relative with `delta`
already added**, and so is the `word=+N` in the optional marker (§3). The
reported read added `delta` a second time:

| | TSV, as written | `delta` applied twice |
|---|---|---|
| presence word | `record+220` = `0x77f7e` for corp 1000106 | `record+228` = `0x0` |
| `factionID` | `record+160` → **273/273** close into `factions` | `record+168` → nothing read, the bit at `+228` is never set |

`record+228` is past the end of a 224-byte record, so it reads into the next
one and lands on a zero; and `record+168` is `iconID`, whose value for corp
1000126 is exactly the reported **1542**.

**[V]** Decoded as written, corp 1000126 is coherent end to end:
`tickerName` ATC, `factionID` **500007**, `nameID` → *"Ammatar Consulate"*,
`descriptionID` → *"The Ammatars have a semi-sovereign state…"*, `ceoID`
3004001, `enemyID` 1000050, `friendID` 1000078, `solarSystemID` 30000001,
`stationID` 60012526, `shares` 100,000,000, `memberLimit` -1. All 283 rows'
`nameID`s resolve in English, and all 273 rows whose `factionID` presence bit
is set close into `factions`.

**The coordinator's methodological point stands regardless, and is now acted
on:** offset 160 *would* have returned a plausible small integer under a
wrong reading, and FK closure against `factions`' 27 keys is what settles it
in one line. That check now runs over all 71 `*ID` columns in the corpus
(§4.2) rather than on the tables this lane happened to look at.

**And the distinction matters:** `dogma-corpus`'s independently written
extractor agrees with this one on all 45 fields of `dogmaattributes` and
`dogmaeffects`, every offset in all six `fsd-reader` tables is reproduced, all
109 data-derived strides agree, and 69 of 71 `*ID` columns close completely.
**The method is sound.** Nothing found so far is a per-table defect either —
this particular table was never wrong.

To avoid the same mis-read next time, the one-line contract is: **add nothing
to the `offset` column.** `delta` is published in the `#record` row for
readers that want to reconstruct the struct frame, not for readers to apply.

---

## 5. Cross-checks against the other two lanes

### 5.1 `fsd-reader`'s six data-confirmed tables: reproduced

All six were decoded by profiling the data files and confirming each column
against something outside the file. The extractor reproduces **every offset**
in all six, and the stride of all six, from the binary alone:

| table | stride | lane's confirmed columns | reproduced |
|---|---:|---:|---|
| `types` | 152 | 14 (+ key) | 14/14, names identical |
| `groups` | 32 | 3 (+ key) | 3/3, names identical |
| `categories` | 40 | 2 (+ key) | 2/2, names identical |
| `accountingentrytypes` | 40 | 3 (+ key) | 3/3, names identical |
| `dungeons` | 120 | 9 (+ key) | 9/9 offsets; 4 better names, see below |
| `missions` | 224 | 7 (+ key) | 7/7 offsets; 3 re-attributed, see §5.3 |

**[V] And all 109 of the inventory's data-derived strides agree exactly, with
zero disagreements.** The six column tables' strides make 115; the extractor
additionally supplies a stride for 23 tables the data could not constrain
(too few multi-row buckets), for 138 in total.

Names the extractor supplies where the lane had a shape or a guess:
`types +24` is `designerIDs` (a `list<i32>`, the lane's only gap in that
record); `dungeons +88` is `entryTypeID` not `typeID`, `+96` is
`gameplayDescriptionID` not "prose A", `+100` is `missionBriefingID` not
`objectiveTextID`; `missions +48` is `contentTemplate`.

**One correction, and it is a real one.** **[V]** `types.portionSize` is an
**`int64` at record `+40`** (`mov rcx, qword ptr [rbx+0x20]` /
`PyLong_FromLongLong`), not a `uint32`. Reading the low 4 bytes happens to
work on little-endian for the values this table holds, so nothing downstream
is wrong today.

**[V] `accountingentrytypes`' two unordered optional ids are now ordered**, and
the `fsd-reader` lane's lean was the wrong way round: `+24` is
`entryJournalMessageID` (presence bit 1) and `+28` is
`entryTypeDescriptionID` (bit 2). That lane said "the lean is A = description
id… that is a lean, not a finding"; it was a lean, and it was wrong.

### 5.2 `dogma-corpus`: two independent extractors, same answer

The `dogma-corpus` lane wrote its own loader reader (`tools/dogma.py
derive_schema`) tonight. Running this lane's generic extractor on its three
tables gives **byte-for-byte identical schemas**: every field name, every
record offset, every presence bit, and both nested element layouts
(`typedogma.dogmaAttributes` elements `{value: f64 @+0, attributeID @+8}`,
stride 16; `dogmaEffects` elements `{effectID @+0, isDefault @+4}`, stride 8;
`dogmaeffects.modifierInfo` elements `{domain @+0, func @+8, effectID @+16,
groupID @+20, modifiedAttributeID @+24, modifyingAttributeID @+28,
operation @+32, skillTypeID @+36, presence @+40}`). Strides 24, 80 and 112
match the inventory and the `imul` immediates.

The only differences are vocabulary, and they are deliberate on this side:
that lane writes `u32`, `msgid` and `bool` where this one writes `i32`, `i32`
and `u8`. **The binary does not distinguish them.** The load is a 4-byte `mov`
boxed with `PyInt_FromLong`, which is signed-agnostic; "this integer is a
localisation message id" is a fact about the name and the corpus, not about
the instruction; and no loader imports `PyBool_FromLong`. Their vocabulary is
more useful and is almost certainly right — this lane simply does not claim
it from this evidence.

**[V] Two things this lane's extractor adds to that lane's §9.2 dungeon
model.** Both were `unknown` there:

- **`position` is a `vec3<f32>` at the object record's struct `+116`**, filling
  exactly the three slots §9.2 lists as unaccounted. It is a
  `PyStructSequence` whose members are named `x`, `y`, `z` in the descriptor,
  read in a three-iteration loop at `[rbx + rdi*4 + 0x74]`. §9.2 guessed the
  names were below its scanner's two-character floor; the real reason is that
  the three floats are one attribute, not three.
- **`assetManipulations`** is a `dict<str,…>` field present on the dungeon row
  (`+8`), the room record (`+0`) and the object record (`+0`), which §9.2 does
  not list.

**One disagreement to resolve, not resolved here.** §9.2 calls the dungeon
row's `rooms` (`+40`) and the room record's `objects` a **list**; this
extractor says both are **`cfsd.dict`**, because the type object handed to
`_PyObject_New` in that arm is the module's dict type, not its list type, and
because record `+48` holds a plausible room count (6 for dungeon 47, matching
the six rooms reachable from `+40`) which a list field would not have room
for. A list field is 8 bytes and a dict field 16; the gap from `rooms` (+40)
to `standingRestrictions` (+56) is 16. Two methods disagree; the coordinator
should settle it, and the cheap test is whether `+48` equals the room count
for all 5,516 dungeons.

### 5.3 `missions.messages`: both other lanes are right, and here is the third witness

**[V]** `missions.messages` is at record **`+72`**, a `dict`, not at `+56`.
Record `+56` is **`extraStandings`**, also a dict. `fsd.CFSD_COLUMNS` places
`messages` at `+56`/`+64` as a `listc`/`listd` pair; that is `extraStandings`
plus its count word. This independently confirms what `agents-missions`
reported and `docs/re/dogma.md` §9.3 quantified.

**[V]** Likewise `fsd.CFSD_COLUMNS["missions"]["dungeonID"] @+144` is really
`missions.killMission.dungeonID`: the extractor puts a nested
`killMission` struct at record `+140` with `dungeonID` at its struct `+4`.
`140 + 4 = 144`. The value that lane read was right; the field it belongs to
now has a name.

**[V]** And `dungeons.rooms @+32/+40` read as a `listc`/`listd` pair works
only by coincidence: `+32` is `contentTags`, and in the dungeons sampled it is
*absent*, with its slot pointing at a zero-length arena marker that sits
exactly eight bytes before the rooms array — so `fsd.cfsd_list`'s "count at
`*slot`" lands on the rooms array's own length word. `docs/re/dogma.md` §1.2
and §10(c) measure where that coincidence fails (86 of 5,516 rows), which is
the same bug seen from the other side.

---

## 6. What is still `unknown`

- **`seasonselection`** has an attribute chain and a layout but **no stride and
  no delta**: it is the header-only singleton with no hash map
  (`static-corpus.md` §3.4), so there is no row factory with index arithmetic
  to read them from.
- **`effectsequences`** is the one table whose data did not corroborate the
  schema: `delta` comes out 0 (its records are list elements) and one of its
  two string fields does not resolve. It is a 1-row table, so there is nothing
  to average over. Treat its `delta` as **[I]**.
- **`dict` value types** resolve to a struct or a scalar in most cases, but
  `dict<str,unknown>` appears where the `mp_subscript` neither boxes a single
  scalar nor builds a recognisable object. The *key* type is read from how the
  subscript coerces its argument and is `str` or `i32`.
- **Signedness** of 4-byte integer fields: `unknown`, see §5.2. Emitted `i32`.
- **Whether a 1-byte field is a bool**: `unknown` from the binary. Emitted
  `u8`. Every one seen reads as a flag.
- **Enum fields** are indistinguishable from `i32`; the value-to-name mapping
  is not in the loader.
- **What an integer references** (`typeID` vs `localizationID` vs a count) is
  not stated in the binary. The classic FSD schemas *do* state it
  (`static-corpus.md` §3.1), so the join graph for cFSD tables has to come
  from the names or from closure tests, not from here.
- **The 24-byte container tag** is untouched by this lane. **[V]** The loader
  does compare it: `typesLoader` carries the strings
  `"Unexpected cFSD schema version. Loader schema: ("`,
  `" Header schema field: ("`, `" Data hash: 0x"` and
  `"Unexpected cFSD hash. Header hash field: 0x"`, so the prologue holds a
  schema tuple and a data hash the loader validates. Decoding that is a
  separate, bounded job and is the obvious next thing in this lane.
- **The two `headerSize != 24` files.** `audiometadata` now has a full record
  layout (stride 72, delta 8) even though its *container* header is still
  undecoded, so it is layout-ready the moment the container is.

---

## 7. Proposed patch to `tools/fsd.py`

**Not applied by this lane** — `tools/fsd.py` belongs to `fsd-reader` and the
coordinator is rewriting around it. Note also that
[`docs/re/dogma.md`](dogma.md) §10 proposes its own patch to the same file,
correcting `cfsd_list` and adding dict decoding. **These two patches are
complementary and the dogma one should land first:** it fixes the
variable-length primitives, and this one adds the generic schema loader that
feeds them. This patch deliberately does not touch `cfsd_list`.

What it adds: `fsd.py` learns to read `findings/pyd-schemas.tsv`, so
`CFSD_COLUMNS` stops being a hand-maintained list of eight tables and becomes
a cross-check on 139.

```diff
--- a/tools/fsd.py
+++ b/tools/fsd.py
@@
 import os
 import pickle
+import re
 import sqlite3
 import struct
@@
 __all__ = [
     "Client",
     "FsdError",
     "SafeUnpickler",
     "load_classic",
     "load_pickle",
     "read_cfsd_header",
     "cfsd_attribute_names",
+    "PydField",
+    "PydSchema",
+    "load_pyd_schemas",
+    "cfsd_native_rows",
     "describe",
 ]
+
+# --- schemas recovered from the native loaders -----------------------------
+
+#: Where `tools/pyd_schema.py` leaves the per-table record layouts it reads
+#: out of `bin64/*Loader.pyd`. Regenerate with
+#: `tools/analysis.sh python3 tools/pyd_schema.py tsv --client /client
+#:  --out findings/pyd-schemas.tsv`. Format: `docs/re/native-loaders.md` §3.
+PYD_SCHEMA_TSV = os.path.join(
+    os.path.dirname(os.path.abspath(__file__)),
+    os.pardir,
+    "findings",
+    "pyd-schemas.tsv",
+)
+
+_PYD_OPTIONAL = re.compile(r"optional\(bit=(0x[0-9a-f]+),word=\+(\d+)\)")
+
+
+class PydField:
+    """One field of a cFSD record, as the generated loader reads it.
+
+    ``offset`` is record-relative for a row field and struct-relative for a
+    field of a nested record type. ``bit``/``word`` are set for an optional
+    field: it is absent when ``u32_at(base + word) & bit`` is zero, which is
+    the branch the generated getter takes to ``Py_None``.
+    """
+
+    __slots__ = ("name", "offset", "kind", "bit", "word")
+
+    def __init__(self, name, offset, kind, bit=None, word=None):
+        self.name = name
+        self.offset = offset
+        self.kind = kind
+        self.bit = bit
+        self.word = word
+
+    def present(self, table, base):
+        if self.bit is None:
+            return True
+        return bool(table._u32(base + self.word) & self.bit)  # noqa: SLF001
+
+
+class PydSchema:
+    """A table's record layout: ``stride``, ``delta``, fields, nested types."""
+
+    __slots__ = ("table", "stride", "delta", "fields", "structs", "status")
+
+    def __init__(self, table):
+        self.table = table
+        self.stride = None
+        self.delta = None
+        self.fields = []
+        self.structs = {}
+        self.status = "ok"
+
+
+def load_pyd_schemas(path=None):
+    """``{table: PydSchema}`` from `findings/pyd-schemas.tsv`.
+
+    These layouts are generated, not hand-maintained: every name, offset and
+    type comes from the client's own `bin64/<table>Loader.pyd`. See
+    `docs/re/native-loaders.md` for the method and the validation, including
+    the fact that the 109 strides `CfsdTable.stride` derives from the data
+    agree with the loaders' own arithmetic with no exceptions.
+    """
+    out = {}
+    with open(path or PYD_SCHEMA_TSV, "r", encoding="utf-8") as fh:
+        header = fh.readline()
+        if not header.startswith("table\tfield\toffset\ttype"):
+            raise FsdError("pyd-schemas.tsv: unexpected header %r" % header)
+        for line in fh:
+            parts = line.rstrip("\n").split("\t")
+            if len(parts) != 6:
+                continue
+            table, field, offset, kind, _conf, how = parts
+            sch = out.setdefault(table, PydSchema(table))
+            if field == "#record":
+                for item in kind.split():
+                    key, _eq, val = item.partition("=")
+                    setattr(sch, key, None if val == "None" else int(val))
+                continue
+            if field == "-":
+                sch.status = how.split(":")[0]
+                continue
+            bit = word = None
+            m = _PYD_OPTIONAL.search(how)
+            if m:
+                bit, word = int(m.group(1), 16), int(m.group(2))
+            f = PydField(field, int(offset), kind, bit, word)
+            if field.startswith("struct:"):
+                name, _slash, member = field[len("struct:") :].partition("/")
+                f.name = member
+                sch.structs.setdefault(name, []).append(f)
+            else:
+                sch.fields.append(f)
+    return out
+
+
+#: Scalar readers, by the `type` column of `findings/pyd-schemas.tsv`.
+_PYD_SCALARS = {
+    "f64": lambda t, p: _F64.unpack_from(t.pl, p)[0],
+    "f32": lambda t, p: _F32.unpack_from(t.pl, p)[0],
+    "i64": lambda t, p: struct.unpack_from("<q", t.pl, p)[0],
+    "i32": lambda t, p: _I32.unpack_from(t.pl, p)[0],
+    "u16": lambda t, p: struct.unpack_from("<H", t.pl, p)[0],
+    "u8": lambda t, p: t.pl[p],
+    "str": lambda t, p: t.maybe_string(_U32.unpack_from(t.pl, p)[0]),
+}
+
+
+def cfsd_native_rows(table, schema, limit=None):
+    """``{key: {name: value}}`` for a cFSD table, using a `PydSchema`.
+
+    Scalars and strings decode. A container field (`list<...>`, `dict<...>`,
+    `struct:...`, `vec3<...>`) comes back as its record offset wrapped in a
+    `('@', offset, kind)` tuple, because reading those needs the
+    variable-length primitives, which belong to the dogma lane's patch.
+    An absent optional field is ``None``.
+    """
+    if schema.stride and table.stride and schema.stride != table.stride:
+        raise FsdError(
+            "%s: loader says stride %d, container says %d"
+            % (schema.table, schema.stride, table.stride)
+        )
+    if table.stride is None and schema.stride:
+        table._stride = schema.stride  # noqa: SLF001
+    out = {}
+    for key, rec in table._locate():  # noqa: SLF001
+        row = {}
+        for f in schema.fields:
+            if not f.present(table, rec):
+                row[f.name] = None
+                continue
+            reader = _PYD_SCALARS.get(f.kind)
+            row[f.name] = (
+                reader(table, rec + f.offset)
+                if reader
+                else ("@", rec + f.offset, f.kind)
+            )
+        out[key] = row
+        if limit and len(out) >= limit:
+            break
+    return out
```

With that in, two follow-ups this lane does **not** propose, because both
touch a second owner's file:

1. **Keep `CFSD_COLUMNS` as a regression fixture rather than the source of
   truth.** A test asserting that every offset in `CFSD_COLUMNS` appears at
   the same offset in `load_pyd_schemas()` would have caught the
   `missions.messages` and `dungeons.rooms` misattributions the day they
   landed.
2. **Make the `*ID` closure sweep of §4.2 a standing test.** For every `i32`
   field whose name ends in `ID`, decode it for every row and assert closure
   into the target table's keys. It needs a small `{field name: table}` map
   for the irregular cases — and getting that map wrong is itself informative:
   three of this lane's five apparent failures were a wrong *target*, and one
   of them (`types.factionID` closing into `factions ∪ npccorporations`) is a
   finding about the column rather than about the schema.

---

## 8. Scope note

Nothing from the client is committed. `findings/pyd-schemas.tsv` is a schema —
field names, offsets, widths and presence bits — which is this project's own
work product, derived from the shipped binary. No table contents are included;
the sample values in §4 exist to show the reader works, and anyone who wants
the data obtains the client themselves. The client was opened read-only
throughout, and `containers/analysis.Dockerfile` needed no change: `pefile`
and `capstone` were already in the analysis image.
