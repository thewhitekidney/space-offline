# The content schema: every header line, frozen

Coordinator step **X1** (W10.5), written by lane W10.5-F on 2026-10-25,
against HEAD `6bf23b7`. Plan v2 §6.1 asks for it, and every W11+ packet
that writes or reads a bundle or authored file pins its header line from
here.

**Precedence.** This file is the truth for column names, column order and
column types. Where C1 §2.7 and C4 §3.1 disagree, C4 won (plan §6.1); each
file below that departs from a C-doc says so in its notes. Ambiguous fields
were checked against build 3619879 (the reference until 2026-10-31, now 3662168; the
row counts in the notes are the former build's unless said otherwise) with `tools/fsd.py`'s `rows()` (K1) or
`fsd.Client.load` in `tools/analysis.sh`. Each such check is recorded as
"Checked:" in that file's notes, with the count it saw. **No row in this
document is a client row.** Every example uses invented ids and names.

**Changing a header** is a schema change. Edit this file, the writer and
every reader in one landing. Bump the manifest `format` minor (§1.2). The
loaders reject the old header, so a stale bundle fails loudly.

---

## 1. The rules every file follows

### 1.1 Encoding (C1 §5.3, plan §1.2)

- UTF-8, `\n` line ends, tab-separated. A field never contains a TAB, CR
  or LF; a writer replaces each with one space (`typedata.py` `_clean`).
- **Bundle files (`out/content/`)**: **line 1 is the header line**, `#`
  followed immediately by the column names joined by single TABs. No
  leading space, no trailing TAB, and nothing comes before it. Any later
  line starting with `#` is a comment (one exception: `celestials.tsv`'s
  `#system` lines, which are part of its format). This is the form
  `celestials.py` and `asteroids.py` already write, and the server's
  existing loaders already skip `#` lines (`asteroid.rs:193`,
  `typedata.rs:330`).
- **Authored files (`content/`)**: lines starting with `#` are comments
  and may appear anywhere, including before the header. **The header line
  is the first line that is neither empty nor a comment**: the column
  names joined by single TABs, with no `#`. Every following non-comment,
  non-empty line is a row. This is plan §2.4's "a header row", and it lets
  a file open with its `[A]` preamble.
- **Three files have no header line**, because phase 1 keeps them
  byte-identical (plan §1.1): `types/typedata.tsv`, `types/skills.tsv` and
  `planets/pi-statics.tsv`. Each line starts with a record kind. Their
  record layouts are frozen in §3 instead.
- **Loaders compare the header line byte for byte**, after removing the
  line end. A mismatch is an error naming the file, the expected line and
  the line found. A loader never reorders columns or matches them by name.
  The test that pins a header copies the line from this document.
- **Column types** in the tables below:

  | type | written as |
  |---|---|
  | `i64` | decimal, optional leading `-` |
  | `f64` | Python `repr(float)`, for example `0.4567`, `1e-05` or `-3.0`. Rust's `str::parse::<f64>` reads every one exactly. Exceptions: `celestials.tsv` writes `%.1f`, unchanged |
  | `bool` | `0` or `1` |
  | `str` | an identifier-like string with no TAB; it may contain spaces |
  | `text` | free text, the last column of its file ("names last") |
  | `enum(a\|b)` | exactly one of the listed strings |
  | `list<T>` | `a,b,c`. An empty field is the empty list. A `list<str>` element never contains `,` |
  | `map<K,V>` | `k=v;k=v`. An empty field is the empty map. Keys contain no `=` or `;` |
  | `T?` | may be empty, meaning absent (the client's presence bit clear, or `None`). A reader maps empty to `None`, never to 0, unless the column says otherwise |

- **Ids in the examples** are invented and sit in ranges this build does
  not use for that kind (for example systems `39000001+`, stations
  `69000001+`, types `990001+`, factions `599001+`, corporations
  `1999001+`). They are safe to copy into a `fixture()`.

### 1.2 `MANIFEST.tsv`

Written last by `tools/content.py` (CP1), read by `content::Bundle::open`
(CP3) and by `tools/up.sh` (CP3). It is the one bundle file with no header
line. Every non-comment line starts with a record kind:

```
# eve-offline content bundle. CCP's data: never commit this directory.
format	1.3
client_build	3662168
extractor	<sha256 hex, 64 chars>
systems	all
data_hash	<sha256 hex, 64 chars>
file	universe/systems.tsv	<rows>	<sha256 hex>	res:/staticdata/systems.static,res:/staticdata/solarsystemcontent.static
input	content/econ/price-overrides.tsv	<sha256 hex>
source	res:/staticdata/systems.static	<md5 hex from resfileindex.txt>
```

| kind | fields | rule |
|---|---|---|
| `format` | `MAJOR.MINOR` | `Bundle::open` refuses a `MAJOR` other than 1. A header change or a new file bumps `MINOR` (**1.5**: `universe/system-effects.tsv`; **1.4**: `dbuffs/collections.tsv` and `dbuffs/modifiers.tsv`; **1.3**: `fighters/abilities.tsv` and `fighters/slots.tsv`; **1.2**: `universe/centers.tsv`; an older reader ignores it, and a bundle without it still loads); a change to this grammar or to §1.1's encoding bumps `MAJOR`. Exactly one line |
| `client_build` | build number | the build of the install it was read from (any build `tools/builds.tsv` accepts). `Bundle::open` accepts it when it is in the server's accepted set (`--client-build`, default every `pinned`/`equivalent` build of `eve_marshal::build::BUILDS`), or when `data_hash` equals one the registry proved for a build in that set. Exactly one line |
| `extractor` | sha256 hex | plan §1.1: over `tools/content*.py` and every reader they import (`fsd dogma pyd_schema loctable celestials asteroids pi_statics typedata skills`), in sorted path order, each file's bytes. C1's "git sha" is superseded. Exactly one line |
| `systems` | `all`, or a comma list of system ids | the `--systems` restriction. Exactly one line |
| `data_hash` | sha256 hex | **format 1.1** (BA-3): sha256 over one `relpath<TAB>sha256<LF>` line per `file` record, **sorted by relpath**. It names the data and nothing else (no build, extractor or source md5), so two extractions hold the same data exactly when it agrees: `content.py --check --against OTHER`. `--check` and `Manifest::parse` refuse one that disagrees with the records. Absent from a 1.0 bundle, which still loads, checks and merges. At most one line |
| `file` | `relpath rows sha256 sources` | one per bundle file, in write order. `rows` counts data lines only (no header, no `#` lines; for the kind-tagged files, every non-`#` line). `sources` is a comma list of `res:` paths |
| `input` | `relpath sha256` | **added by X1**: one per authored `content/` file an extractor section read (`npc/region-pirates.tsv`, `econ/price-overrides.tsv`, `econ/meta-mult.tsv`, `agents/lp-overrides.tsv`). `up.sh` re-extracts when one differs, as it does for `source` and `extractor`. Without it, an edit to an override file would leave the old bundle in use |
| `source` | `res-path md5` | one per corpus file read, md5 from `resfileindex.txt` column 3 |

The singleton records come first, in the order shown, then every
`file`, then every `input`, then every `source`. The first line is the
comment exactly as shown.

**A `--sections` run over an existing bundle merges** (FX8):
- It rewrites only those sections' files and `file` rows, and keeps every
  other file and row.
- It keeps the old `extractor` hash. On an `input` or `source` key that
  both manifests list, the old digest wins. So `up.sh` never sees a merged
  bundle as fresher than its oldest file.
- It refuses a bundle whose `format` MAJOR, `client_build` or `systems`
  differ, or which fails `--check`. A MINOR difference merges and is
  written in the current format (a 1.0 bundle gains its `data_hash`,
  recomputed over the merged records).

---

## 2. Bundle files: `out/content/`

| file | writer | readers |
|---|---|---|
| `universe/systems.tsv` | CP1 | CP3b, CP21, CP30, CP26a, CP11b, CP24 |
| `universe/regions.tsv` | CP1 | CP3b, CP11b |
| `universe/constellations.tsv` | CP1 | CP3b |
| `universe/centers.tsv` | MAP-A | MAP-B (`celestial::Centers`) |
| `universe/celestials.tsv` | CP1 | CP3, CP3c, CP21, CP28a, CP13 |
| `universe/dock-locators.tsv` | CP1 | CP3c |
| `universe/system-effects.tsv` | SYS-1 (`content.py`, the `universe` section) | `system_effect` (`system_effect::Effects`: the effect beacon of a J-space system) |
| `belts/rocks.tsv` | CP1 | CP14 |
| `types/typedata.tsv` | CP1 | CP3, CP58a, CP20 |
| `types/skills.tsv` | CP1 | CP3, CP59a |
| `types/market.tsv` | CP12 | CP24, CP37a, CP35 |
| `types/marketgroups.tsv` | CP12 | none yet |
| `types/typelists.tsv` | CP12 | CP16 |
| `types/materials.tsv` | CP12 | CP32b, CP24 |
| `types/materials-random.tsv` | CP12 | CP32b |
| `types/compress.tsv` | CP12 | CP32b |
| `planets/pi-statics.tsv` | CP1 | CP3, CP60 |
| `dungeons/dungeons.tsv` | CP10 | CP16, CP21, CP28a, CP26a, CP29 |
| `dungeons/rooms.tsv` | CP10 | CP16 |
| `dungeons/objects.tsv` | CP10 | CP16, CP28a |
| `dungeons/gates.tsv` | CP10 | CP16 |
| `dungeons/spawns.tsv` | CP10 | CP16, CP28b, CP26b |
| `dungeons/archetypes.tsv` | CP10 | CP16 |
| `dungeons/resources.tsv` | CP10 | CP16, CP28a |
| `dungeons/room-rocks.tsv` | CP10 | CP16, CP10 |
| `dungeons/agents-in-space.tsv` | CP10 | none yet |
| `npc/factions.tsv` | CP11a | CP20, CP18 |
| `npc/corporations.tsv` | CP11a | CP20, CP37a, CP11b, CP26a |
| `npc/types.tsv` | CP11a | CP20, CP46 |
| `npc/components.tsv` | CP11a | CP20 |
| `npc/reactive-spawn.tsv` | CP11a | CP20 |
| `npc/bounty.tsv` | CP11a | CP20, CP22, CP38, CP26b |
| `npc/regions.tsv` | CP11b | CP20, CP21, CP26a, CP30 |
| `npc/home-pirate.tsv` | CP11b | CP20, CP19, CP48 |
| `npc/trades.tsv` | CP11b | CP35, CP24 |
| `npc/loot.tsv` | CP11b | CP20, CP22 |
| `npc/police.tsv` | NW-7 (`tools/content_police.py`) | `npc_data`, `concord` |
| `npc/concord.tsv` | `tools/content_police.py` | `npc_data`, `concord` |
| `agents/agents.tsv` | CP13 | CP19, CP26a, CP26b, CP41, CP48 |
| `agents/missions.tsv` | CP13 | CP19, CP26a, CP26b, CP41, CP48 |
| `agents/mission-messages.tsv` | CP13 | CP41 |
| `agents/agent-types.tsv` | CP13 | none yet |
| `agents/divisions.tsv` | CP13 | none yet |
| `agents/epic-arcs.tsv` | CP13 | CP41 |
| `agents/schools.tsv` | CP13, P-START-X | P-START-1, E5 |
| `agents/races.tsv` | P-START-X | E7 (`starter_ship`, `ship_type_id`), start-skills (`start`, `skills`) |
| `agents/bloodlines.tsv` | start-skills | start-skills (`start`) |
| `agents/npc-characters.tsv` | CP13 | none yet |
| `agents/lp-offers.tsv` | CP37a | CP37b |
| `industry/blueprints.tsv` | CP24 | CP33b, CP24 |
| `stations/stations.tsv` | CP24 | CP35, CP32b, CP31 |
| `stations/operations.tsv` | CP24 | CP35, CP32b |
| `econ/prices.tsv` | CP24 | CP31, CP32b, CP33b, CP35, CP44 |
| `explore/loot.tsv` | HK3 | HK3 (`hacking_loot`) |
| `explore/site-loot.tsv` | CAP-LOOT | CAP-LOOT (`hacking_loot`) |
| `salvage/wrecks.tsv` | X-SALV | X-SALV (`salvage`) |
| `salvage/yields.tsv` | X-SALV | X-SALV (`salvage`) |
| `fighters/abilities.tsv` | FT-A | FT-A (`fighters_data`) |
| `fighters/slots.tsv` | FT-A | FT-A (`fighters_data`) |
| `dbuffs/collections.tsv` | K-BOOST | K-BOOST (`dbuff_data`) |
| `store/catalogue.tsv` | L1 NES-BACKEND (`content_store.py`, the `store` section) | L1 (`nes`) |
| `store/types.tsv` | L1 NES-BACKEND (`content_store.py`) | L1 (`nes`: the item types the store delivers) |
| `skins/licenses.tsv` | L2 SKINS (`content_skins.py`, the `skins` section) | L2 (`skins`: licence types and their item types) |
| `skins/skins.tsv` | L2 SKINS (`content_skins.py`) | L2 (`skins`: the hulls a SKIN fits) |
| `facwar/systems.tsv` | FW-3 (`content_facwar.py`, the `facwar` section) | `facwar` (`facwar::Table`: the warzone territory list) |
| `facwar/max_lp.tsv` | #133 FW-KILLS (`content_facwar.py`) | `facwar` (`facwar::Table::max_lp_of`: the most a kill pays by hull group) |
| `mining/geodes.tsv` | L6A GEODES (`content_geodes.py`, the `geodes` section) | L6A (`eve-mod-econ` `geode`: the geode crate types) |
| `mining/geode-candidates.tsv` | L6A GEODES (`content_geodes.py`) | the owner, to pick from; the server does not read it |
| `dbuffs/modifiers.tsv` | K-BOOST | K-BOOST (`dbuff_data`) |
| `deploy/units.tsv` | hand-data (`content_deploy.py`, the `deploy` section) | `mtu` (`mtu::Specs`: the Mobile Depot and MTU space components) |
| `deploy/injectors.tsv` | hand-data (`content_deploy.py`) | `skill_trading` (`skill_trading::Rows`: the extractor and injector item rows) |

Every reader and writer above is a §6 packet of plan v2. "At extract time" means a later `content_<x>.py` section reads an earlier one's output through `DEPENDS`. The full reader list is in each section.

### `universe/systems.tsv`

- **Writer:** CP1 (`content.py`, section `universe`).
- **Readers:** CP3b (`celestial::UNIVERSE`; band, region and security for CP21, CP30, CP3b's crimewatch, CP26a); CP11b and CP24 read it at extract time through `DEPENDS`.
- **Key:** `system_id`.
- **Stable client ids and delta keys (ruling 4):** `system_id` is the stable id in the `site`, `dbs`, `fwvp` and `wh` deltas. `sys_rank` (plan §3.4) is computed from this file's id set and is never persisted.

```
#system_id	region_id	constellation_id	faction_id	owner_faction	security	security_class	hub	border	fringe	corridor	regional	international	wormhole_class_id	disallow_scanning	dscan_range	sun_type_id	name
```

| column | type | source and meaning |
|---|---|---|
| `system_id` | `i64` | `solarsystemcontent.solarSystemID` |
| `region_id` | `i64` | `systems.static.regionID` |
| `constellation_id` | `i64` | `systems.static.constellationID` |
| `faction_id` | `i64?` | `systems.static.factionID` (2,671 of 8,490 set [V]) |
| `owner_faction` | `i64?` | `factionsowningsolarsystems.static` (22 rows [V], C1) |
| `security` | `f64` | `solarsystemcontent.security`, unrounded; CP1's check compares it |
| `security_class` | `str?` | `solarsystemcontent.securityClass` (a letter code such as `B1`; 5,193 set [V]), else `systems.static.securityClass` |
| `hub` | `bool` | `solarsystemcontent.hub` (all 8,490 rows carry it [V]) |
| `border` | `bool` | `solarsystemcontent.border` |
| `fringe` | `bool` | `solarsystemcontent.fringe` |
| `corridor` | `bool` | `solarsystemcontent.corridor` |
| `regional` | `bool` | `solarsystemcontent.regional` |
| `international` | `bool` | `solarsystemcontent.international` |
| `wormhole_class_id` | `i64?` | `solarsystemcontent.wormholeClassID` where set (692 [V]), else `systems.static.wormholeClassID` (8,113 [V]) |
| `disallow_scanning` | `bool` | `solarsystemcontent.disallowScanning`; absent is 0 (set on 1 system [V]) |
| `dscan_range` | `f64?` | `solarsystemcontent.dscanRange` (set on 1 system [V]) |
| `sun_type_id` | `i64?` | `solarsystemcontent.sunTypeID` (8,089 [V]; empty for a system with no star) |
| `name` | `text` | `solarSystemNameID` through the en-us table |

Invented example row:

```
39000001	19000001	29000001	599001		0.4567	B1	0	1	0	0	0	0		0		990009	Example Prime
```

- C1 §2.1 claimed `disallowScanning`, `dscanRange` and `wormholeClassID` were per-system fields of `solarsystemcontent`. Checked with `fsd.Client.load` over all 8,490 rows: the first two are set on one system each, and `wormholeClassID` on 692. Most wormhole classes are on `systems.static` (8,113). The fallback order above reflects that.

### `universe/regions.tsv`

- **Writer:** CP1.
- **Readers:** CP3b; CP11b at extract time (R3 steps 0-1).
- **Key:** `region_id`.
- **Stable client ids and delta keys (ruling 4):** none (no delta is keyed by region).

```
#region_id	nebula_id	faction_id	wormhole_class_id	neighbours	name
```

| column | type | source and meaning |
|---|---|---|
| `region_id` | `i64` | `regions.static.regionID` |
| `nebula_id` | `i64?` | `regions.static.nebulaID`, a graphic id; a wormhole slim names the far region's as `nebulaType` (R-WH §3). Added for build 3662168 (lane ci-fix-d5); all 114 rows carry it [V] |
| `faction_id` | `i64?` | `regions.static.factionID` |
| `wormhole_class_id` | `i64?` | `regions.static.wormholeClassID` |
| `neighbours` | `list<i64>` | `regions.static.neighbours` |
| `name` | `text` | `nameID` through the en-us table |

Invented example row:

```
19000001	990011	599001		19000002,19000003	Example Reach
```

- Checked: `regions.static` (114 rows) has the columns `factionID`, `neighbours`, `wormholeClassID` and `nebulaID` [V].

### `universe/constellations.tsv`

- **Writer:** CP1.
- **Readers:** CP3b.
- **Key:** `constellation_id`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#constellation_id	region_id	faction_id	wormhole_class_id	neighbours	name
```

| column | type | source and meaning |
|---|---|---|
| `constellation_id` | `i64` | `constellations.static.constellationID` |
| `region_id` | `i64` | `constellations.static.regionID` |
| `faction_id` | `i64?` | `constellations.static.factionID` |
| `wormhole_class_id` | `i64?` | `constellations.static.wormholeClassID` |
| `neighbours` | `list<i64>` | `constellations.static.neighbours` |
| `name` | `text` | `nameID` through the en-us table |

Invented example row:

```
29000001	19000001			29000002	Example Cluster
```

### `universe/centers.tsv`

- **Writer:** MAP-A (`content.py center_rows`).
- **Readers:** MAP-B (`celestial::Centers`): light-year distance for jump drives. A bundle made before format 1.2 has no such file and loads; distances then answer "unknown".
- **Key:** `(kind, id)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#kind	id	x	y	z
```

| column | type | source and meaning |
|---|---|---|
| `kind` | text | `s` system, `c` constellation, `r` region; rows sorted in that order, then by id |
| `id` | `i64` | `solarSystemID`, `constellationID` or `regionID` |
| `x` `y` `z` | `f64` | the record's `center`, metres, galaxy frame (`systems.static`, `constellations.static`, `regions.static`) |

A `--systems` sandbox writes the selected systems and the constellations and regions that hold them. `--check` requires the per-kind row counts to equal the three tables'.

Invented example row:

```
s	39000001	1.0e16	-2.0e16	3.5e16
```

### `universe/celestials.tsv`

- **Writer:** CP1, which imports `celestials.rows_for`. CP2 adds stations under the star and under moons; the format does not change.
- **Readers:** `celestial::Catalogue` (installed by CP3); CP3c (a station's `type_id`, the key into `dock-locators.tsv`); CP21 and CP28a (anchors, C3 §4.3); CP13's check (every agent station exists).
- **Key:** `id`.
- **Stable client ids and delta keys (ruling 4):** `id` is a client item id. A belt's `id` is the stable key of the `rock` and `beltwait` deltas, and a station's `id` is the key of `npcorder` and `npcmult`.

```
#kind	id	type_id	radius	x	y	z	system	dest	dest_system
```

| column | type | source and meaning |
|---|---|---|
| `kind` | `enum(star\|planet\|moon\|belt\|station\|gate)` | the record kind; first, because readers scan for it |
| `id` | `i64` | the celestial's item id |
| `type_id` | `i64` | its type |
| `radius` | `f64` | written `%.1f` |
| `x` | `f64` | written `%.1f`; the star is the origin |
| `y` | `f64` | written `%.1f` |
| `z` | `f64` | written `%.1f` |
| `system` | `i64` | the solar system |
| `dest` | `i64` | a gate's destination gate; 0 otherwise |
| `dest_system` | `i64` | a gate's destination system; 0 otherwise |

Invented example row:

```
station	69000001	990002	0.0	1000.0	-2000.0	3000.0	39000001	0	0
```

- **Unchanged from `tools/celestials.py`** (`HEADER`, `celestials.py:72`), so the header line is the existing one.
- The file also carries one structured comment per system, `#system<TAB><system><TAB><constellation><TAB><region>`, before that system's rows (`celestials.py:207`). Readers that predate it skip it as a comment, and `celestial::Catalogue` reads it. It is part of the format.
- The bundle form is the `--all` form, optionally filtered by `--systems`. It has no `# solarSystemID` line.

### `universe/system-effects.tsv`

- **Writer:** SYS-1 (`content.py`, `universe` section; `effect_rows`).
- **Readers:** `eve-server` `system_effect::TABLE` (`system_effect::Effects`). Optional: without the file no system has an effect.
- **Key:** `system_id`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#system_id	beacon_type_id	sun_type_id	sun_item_id	x	y	z
```

| column | type | source and meaning |
|---|---|---|
| `system_id` | `i64` | `solarsystemcontent.solarSystemID` |
| `beacon_type_id` | `i64` | `secondarySun.effectBeaconTypeID`: a group 920 type in `types/typedata.tsv`, with category 7 effects. The client's tooltip subscripts this type, so it is what the server must send as `sourceTypeID` |
| `sun_type_id` | `i64?` | `secondarySun.typeID` (a sun type, not a beacon) |
| `sun_item_id` | `i64?` | `secondarySun.itemID` |
| `x` | `f64` | `secondarySun.position.x`, system frame, metres |
| `y` | `f64` | position y |
| `z` | `f64` | position z |

Invented example row:

```
39000001	990101	990102	990103	1.0	-2.0	3.5
```

- One row per system whose `secondarySun` has an `effectBeaconTypeID`; the 1,038 J-space systems of build 3662168 and no other [V-data] (r-wormhole §3). **Read the row, never compute the beacon from the system's class**: C13, the five Drifter systems and some C4 systems do not match [V-data].
- A system without a row has no effect. The group 920 types and their effects and modifiers reach the bundle through `types/typedata.tsv` (`tools/typedata.py`, `GROUPS`).
- `disallowedAnchorCategories` is **not** written: nothing reads it, and a new `systems.tsv` column would change every row fixture.

### `universe/dock-locators.tsv`

- **Writer:** CP1 (`universe` section, plan §1.2); the chain is `docking.rs`'s regeneration recipe (`types.graphicID` → `graphicids.graphicLocationID` → `graphiclocations`).
- **Readers:** CP3c (`docking::LOCATORS`).
- **Key:** `(station_type_id, name)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#station_type_id	graphic_id	graphic_location_id	category	name	px	py	pz	dx	dy	dz	hull
```

| column | type | source and meaning |
|---|---|---|
| `station_type_id` | `i64` | a group-15 station type |
| `graphic_id` | `i64` | `types.graphicID` |
| `graphic_location_id` | `i64` | `graphicids.graphicLocationID` |
| `category` | `str` | the directional locator's `category`, for example `undockPoint` |
| `name` | `str` | the locator's `name`, for example `capital01` |
| `px` | `f64` | position x, model space; the `f32` widened by `repr` |
| `py` | `f64` | position y |
| `pz` | `f64` | position z |
| `dx` | `f64` | direction x |
| `dy` | `f64` | direction y |
| `dz` | `f64` | direction z |
| `hull` | `text` | CCP's model name; kept so a reader can confirm a row (`docking.rs` `StationGraphic.hull`) |

Invented example row:

```
990002	99901	99	undockPoint	undockPoint	1.0	2.0	-3.0	0.0	0.0	-1.0	xx1
```

- One row per **directional** locator of every graphic that some group-15 type resolves to. Non-directional `locators` are not written, because nothing reads them (`docking.rs:296-298`).
- Positions and directions are `f32` in the client. They are written as `repr(float(v))` of the decoded `f32`, so `f32::from(f64)` round-trips exactly. This keeps the bit-exactness that `docking.rs` `Locator` documents.

### `belts/rocks.tsv`

- **Writer:** CP1 (imports `asteroids.py`). The order is fixed (plan §1.1): by `system`, then `belt_id`, then the client's own in-belt order, which is ascending `type_id` as `asteroids.py:130` writes it.
- **Readers:** CP14 (`asteroid.rs`, with an offset index over the sorted file).
- **Key:** `(belt_id, type_id)`.
- **Stable client ids and delta keys (ruling 4):** **`belt_id`** plus the rock ordinal `n` is the `rock` delta key (ruling 4). `n` is the index of the rock when this belt's rows are expanded in file order, `count` rocks per row. **`ordinal` is not stable**: it is the belt's rank among all belt ids, so a client update that adds a belt renumbers it. It feeds only the id formula `MIN_ROCK_ID + ordinal·IDS_PER_BELT + n` (`asteroid.rs:64-75`), which is recomputed on load and never persisted.

```
#belt_id	ordinal	type_id	count	system	growth	unit_size	max_radius
```

| column | type | source and meaning |
|---|---|---|
| `belt_id` | `i64` | the belt's item id |
| `ordinal` | `i64` | the belt's rank in the sorted belt ids of `asteroidcountbytypeidbeltid` (id band only) |
| `type_id` | `i64` | ore type |
| `count` | `i64` | rocks of that type in the belt |
| `system` | `i64` | solar system |
| `growth` | `f64` | attribute 1980, `repr` |
| `unit_size` | `f64` | attribute 1981, `repr` |
| `max_radius` | `f64` | attribute 2727, `repr` |

Invented example row:

```
49000001	7	990003	12	39000001	1.5	0.1	16255.0
```

- **Unchanged from `tools/asteroids.py`** (`HEADER`, `asteroids.py:63`). The single-system `# solarSystemID` comment line is never written into the bundle.

### `types/market.tsv`

- **Writer:** CP12 (`content_types.py`).
- **Readers:** CP24 and CP37a at extract time (`DEPENDS`: `variation_parent_type_id`, `meta_group_id`, `base_price`); CP35 (Tier A: BPO and skillbook `base_price`; Tier B: `meta_group_id`, `tech_level`).
- **Key:** `type_id`.
- **Stable client ids and delta keys (ruling 4):** `type_id` is the stable id in the `npcorder`, `npcmult` and `mktidx` deltas.

```
#type_id	group_id	category_id	market_group_id	base_price	meta_group_id	meta_level	tech_level	variation_parent_type_id	faction_id	race_id	wreck_type_id	portion_size	published
```

| column | type | source and meaning |
|---|---|---|
| `type_id` | `i64` | every row of `types` (53,288 [V]) |
| `group_id` | `i64` | `types.groupID` |
| `category_id` | `i64` | `groups.categoryID` |
| `market_group_id` | `i64?` | `types.marketGroupID` |
| `base_price` | `f64` | `types.basePrice`, 0 when absent |
| `meta_group_id` | `i64?` | `types.metaGroupID` (13,905 set [V]) |
| `meta_level` | `i64?` | `types.metaLevel` (8,288 set [V]) |
| `tech_level` | `i64?` | `types.techLevel` |
| `variation_parent_type_id` | `i64?` | `types.variationParentTypeID` (4,804 set [V]) |
| `faction_id` | `i64?` | `types.factionID` (1,383 set, the K1 offset +72 [V]) |
| `race_id` | `i64?` | `types.raceID` |
| `wreck_type_id` | `i64?` | `types.wreckTypeID` (5,703 set [V]) |
| `portion_size` | `i64` | `types.portionSize` |
| `published` | `bool` | `types.published` |

Invented example row:

```
990001	99001	6	99501	250000.0	1	0	1			1	990020	1	1
```

- X1 adds `group_id`, `category_id`, `tech_level` and `variation_parent_type_id` to C1 §2.3's ten columns. C5 §3.2 step 4 (the parent rule), C5 §4.3 (Tier B selects on `techLevel` and category) and C4 §5.2 (navy `reqItems`) need them for types outside `typedata.tsv`'s six categories. All four are `types` fields [V, `fsd.rows(c,'types')` key set].

### `types/marketgroups.tsv`

- **Writer:** CP12.
- **Readers:** no v2 packet reads it yet (the market browse tree is client-side); kept for the market lanes.
- **Key:** `market_group_id`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#market_group_id	parent_id	has_types	icon_id	name
```

| column | type | source and meaning |
|---|---|---|
| `market_group_id` | `i64` | key (2,114 rows [V]) |
| `parent_id` | `i64?` | `parentGroupID` |
| `has_types` | `bool` | `hasTypes` |
| `icon_id` | `i64?` | `iconID` |
| `name` | `text` | `nameID` through the en-us table |

Invented example row:

```
99501	99500	1		Example Frigates
```

### `types/typelists.tsv`

- **Writer:** CP12.
- **Readers:** CP16 (`dungeon_data`: `allowed_types` for gates).
- **Key:** `list_id`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#list_id	inc_types	inc_groups	inc_cats	exc_types	exc_groups	exc_cats
```

| column | type | source and meaning |
|---|---|---|
| `list_id` | `i64` | `typelist` key (469 rows [V]) |
| `inc_types` | `list<i64>` | `includedTypeIDs` |
| `inc_groups` | `list<i64>` | `includedGroupIDs` |
| `inc_cats` | `list<i64>` | `includedCategoryIDs` |
| `exc_types` | `list<i64>` | `excludedTypeIDs` |
| `exc_groups` | `list<i64>` | `excludedGroupIDs` |
| `exc_cats` | `list<i64>` | `excludedCategoryIDs` |

Invented example row:

```
9901		99001,99002		990001		
```

### `types/materials.tsv`

- **Writer:** CP12.
- **Readers:** CP32b (`reprocess::Table`); CP24 at extract time (the reprocess floor, C5 §3.2 step 6).
- **Key:** `(type_id, material_type_id)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#type_id	material_type_id	quantity
```

| column | type | source and meaning |
|---|---|---|
| `type_id` | `i64` | `typematerials` key (9,866 rows [V]) |
| `material_type_id` | `i64` | `materials[].materialTypeID` |
| `quantity` | `i64` | `materials[].quantity` |

Invented example row:

```
990001	990030	250
```

### `types/materials-random.tsv`

- **Writer:** CP12.
- **Readers:** CP32b.
- **Key:** `(type_id, material_type_id)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#type_id	material_type_id	qmin	qmax
```

| column | type | source and meaning |
|---|---|---|
| `type_id` | `i64` | `typematerials` key, where `randomizedMaterials` is set (10 types [V]) |
| `material_type_id` | `i64` | `randomizedMaterials[].materialTypeID` |
| `qmin` | `i64` | `quantityMin` |
| `qmax` | `i64` | `quantityMax` |

Invented example row:

```
990004	990030	10	40
```

- Checked: the element fields are `materialTypeID`, `quantityMin` and `quantityMax` [V].

### `types/compress.tsv`

- **Writer:** CP12.
- **Readers:** CP32b.
- **Key:** `type_id`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#type_id	compressed_type_id
```

| column | type | source and meaning |
|---|---|---|
| `type_id` | `i64` | `compressibletypes` key: the source type (212 rows [V]) |
| `compressed_type_id` | `i64` | the scalar row value: the compressed type |

Invented example row:

```
990003	990005
```

- Checked: `compressibletypes` rows are scalar ints, so this file has no `fsd` field names. The direction (key = source) is the one `reprocess/tables.rs:32-33` documents.

### `dungeons/dungeons.tsv`

- **Writer:** CP10 (`content_dungeons.py`).
- **Readers:** CP16 (`dungeon_data`), and through it CP21 (the draw), CP28a, CP26a (pocket layout) and CP29 (Agency and tracker fields).
- **Key:** `dungeon_id`.
- **Stable client ids and delta keys (ruling 4):** `dungeon_id` is the stored **value** of a `site` delta (checked on load; plan §4.2). It is never a key.

```
#dungeon_id	archetype_id	faction_id	difficulty	entry_object_id	entry_type_id	objectives_id	name_id	content_tags
```

| column | type | source and meaning |
|---|---|---|
| `dungeon_id` | `i64` | `dungeons.dungeonID` (5,516 rows [V]) |
| `archetype_id` | `i64` | `archetypeID` |
| `faction_id` | `i64?` | `factionID` |
| `difficulty` | `i64?` | `difficulty`; empty on 1,830 dungeons [V] |
| `entry_object_id` | `i64?` | `entryObjectID` |
| `entry_type_id` | `i64?` | `entryTypeID`: 28356 is an anomaly, 19728 a signature [V, C3 §4.2] |
| `objectives_id` | `i64?` | `objectivesID` |
| `name_id` | `i64?` | `dungeonNameID` (the client's field name; C1 called it `name_id`) |
| `content_tags` | `list<str>` | `contentTags` |

Invented example row:

```
999101	24	599001	2	8001	28356		999901	activity_combat
```

- Checked with `fsd.rows(c,'dungeons')`: the name field is `dungeonNameID`, not `nameID`, and `missionBriefingID`, `standingRestrictions` and `assetManipulations` are not written (no packet reads them).

### `dungeons/rooms.tsv`

- **Writer:** CP10.
- **Readers:** CP16.
- **Key:** `(dungeon_id, room_id)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#dungeon_id	room_id	empty_reset_delay	message_header_id	message_text_id	short_name
```

| column | type | source and meaning |
|---|---|---|
| `dungeon_id` | `i64` | parent dungeon |
| `room_id` | `i64` | `rooms` key |
| `empty_reset_delay` | `i64?` | `emptyResetDelay` |
| `message_header_id` | `i64?` | `messageHeaderID` |
| `message_text_id` | `i64?` | `messageTextID` |
| `short_name` | `text` | `shortName` (a `str` [V]) |

Invented example row:

```
999101	1				Room 1
```

- X1 moves `short_name` to the end, because it is the only free text (C1 §5.3 "names last"). Otherwise these are C1 §2.5's columns.

### `dungeons/objects.tsv`

- **Writer:** CP10.
- **Readers:** CP16, and through it CP28a (`site.rs`: one ball per room-0 object; `dunObjectID` is `object_id`).
- **Key:** `(dungeon_id, object_id)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#dungeon_id	room_id	object_id	type_id	x	y	z	radius	yaw	pitch	roll	object_min_respawn	object_max_respawn	object_delay_spawn	object_trigger_spawn	name_id
```

| column | type | source and meaning |
|---|---|---|
| `dungeon_id` | `i64` | parent dungeon |
| `room_id` | `i64` | `objects[].roomID` |
| `object_id` | `i64` | `objectID` |
| `type_id` | `i64` | `typeID` |
| `x` | `f64` | `position[0]`, room-local |
| `y` | `f64` | `position[1]` |
| `z` | `f64` | `position[2]` |
| `radius` | `f64?` | `radius` |
| `yaw` | `f64?` | `yaw` |
| `pitch` | `f64?` | `pitch` |
| `roll` | `f64?` | `roll` |
| `object_min_respawn` | `i64?` | `guardCommand.objectMinRespawnDelay` |
| `object_max_respawn` | `i64?` | `guardCommand.objectMaxRespawnDelay` |
| `object_delay_spawn` | `bool` | `guardCommand.objectDelaySpawn` |
| `object_trigger_spawn` | `bool` | `guardCommand.objectTriggerSpawn` |
| `name_id` | `i64?` | `objectNameID` |

Invented example row:

```
999101	1	8001	990060	0.0	0.0	0.0	2500.0						0	0	
```

- X1 adds the four object-level `guardCommand` fields. CP28b needs an object's own respawn window ("in-pocket respawn inside the object's [min, max]"), and C1's list had only the entity-level ones. Checked: the `guardCommand` key set is `aggressionRange entityDelaySpawn entityMaxRespawnDelay entityMinRespawnDelay entitySpawnOnce jitterRadius objectDelaySpawn objectMaxRespawnDelay objectMinRespawnDelay objectTriggerSpawn orbitRange` [V].
- **`objectiveTargetGroup` is never set** on any object or entity in this build (0 of 260,242 objects and 0 of 30,610 entities [V]). So CP28a's `objectiveTargetGroup` slim extra has no data source and is not a column. `groupTag` (a `str`, set on 104,842 objects) is not written, because no packet names a use for it.

### `dungeons/gates.tsv`

- **Writer:** CP10.
- **Readers:** CP16.
- **Key:** `(dungeon_id, from_object_id)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#dungeon_id	from_object_id	allowed_ships_list	disallowed_fitted_types_list	allowed_ship_races
```

| column | type | source and meaning |
|---|---|---|
| `dungeon_id` | `i64` | parent dungeon |
| `from_object_id` | `i64` | `connections[].fromObjectID` |
| `allowed_ships_list` | `i64?` | `allowedShipsList`, a `typelists.tsv` id |
| `disallowed_fitted_types_list` | `i64?` | `disallowedFittedTypesList`, a `typelists.tsv` id (an int, 103 set [V]) |
| `allowed_ship_races` | `list<i64>` | `allowedShipRaces` (a list, 24 set [V]) |

Invented example row:

```
999101	8002	9901		
```

### `dungeons/spawns.tsv`

- **Writer:** CP10.
- **Readers:** CP16, and through it CP28b (`site_spawn.rs`: squads per C6 R5) and CP26b (the reward base, see the note).
- **Key:** `(dungeon_id, object_id, entity_id)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#dungeon_id	room_id	object_id	entity_id	npc_grouping_id	behavior_name	group_behavior_name	trigger_spawn	additional_loot_id	entity_min_respawn	entity_max_respawn	spawn_once	delay_spawn	aggression_range	orbit_range	jitter_radius	entity_name_id
```

| column | type | source and meaning |
|---|---|---|
| `dungeon_id` | `i64` | parent dungeon |
| `room_id` | `i64` | the object's room |
| `object_id` | `i64` | the object that carries the entity |
| `entity_id` | `i64` | `entities` key, equal to `entityID` on all 30,610 [V]. **Not a type id** |
| `npc_grouping_id` | `i64?` | `npcGroupingID` (out of corpus, written verbatim) |
| `behavior_name` | `str?` | `behaviorName` |
| `group_behavior_name` | `str?` | `groupBehaviorName` |
| `trigger_spawn` | `bool` | `triggerSpawn` |
| `additional_loot_id` | `i64?` | `additionalLootId` (5 set [V]) |
| `entity_min_respawn` | `i64?` | the object's `guardCommand.entityMinRespawnDelay` |
| `entity_max_respawn` | `i64?` | `guardCommand.entityMaxRespawnDelay` |
| `spawn_once` | `bool` | `guardCommand.entitySpawnOnce` |
| `delay_spawn` | `bool` | `guardCommand.entityDelaySpawn` (C6 R5) |
| `aggression_range` | `f64?` | `guardCommand.aggressionRange` |
| `orbit_range` | `f64?` | `guardCommand.orbitRange` |
| `jitter_radius` | `f64?` | `guardCommand.jitterRadius` (C6 R5) |
| `entity_name_id` | `i64?` | `entityNameID` |

Invented example row:

```
999101	1	8001	70001	9901			0		60	120	0	0	30000.0	5000.0		
```

- X1 adds `delay_spawn`, `jitter_radius` and `entity_name_id`, which C6 R5 reads, to C1 §2.5's columns.
- **Checked: `entityID` is not a type.** Only 5,777 of the 30,586 distinct entity ids coincide with any type id, and those types are spread over 30 categories (91 SKINs most often). That is chance overlap, not a reference [V]. So **no file joins a spawn to a bounty**. The plan's CP26b base ("the median bounty sum of that level's kill-mission spawns, from `npc/bounty.tsv`, `dungeons/spawns.tsv`, `agents/missions.tsv`") has to go through C6 R5's squad fill (`npc_gen::dungeon_squads`) to get types first. This is for the coordinator: it adds CP22 to CP26b's "after" column.

### `dungeons/archetypes.tsv`

- **Writer:** CP10.
- **Readers:** CP16 (C3 §4.2 filters).
- **Key:** `archetype_id`.
- **Stable client ids and delta keys (ruling 4):** `archetype_id` is part of the `site` delta key (with `system_id`, the `population.tsv` `key` and `k`).

```
#archetype_id	title_id	content_tags	location
```

| column | type | source and meaning |
|---|---|---|
| `archetype_id` | `i64` | `archetypes` key (71 rows [V]) |
| `title_id` | `i64?` | `titleID` |
| `content_tags` | `list<str>` | `contentTags` |
| `location` | `text` | `location`, free text such as `Asteroid belt` [V]; empty when absent |

Invented example row:

```
98		activity_combat	Dungeon Specific
```

- X1 orders C1's columns names-last: `location` is free text [V: values include `Asteroid belt` and `Dungeon Specific`].

### `dungeons/resources.tsv`

- **Writer:** CP10.
- **Readers:** CP16 (the ore band filter, C3 §4.1 `*`); CP28a (ore rocks).
- **Key:** `(dungeon_id, kind)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#dungeon_id	kind	type_ids	ore_band
```

| column | type | source and meaning |
|---|---|---|
| `dungeon_id` | `i64` | key of `ore`/`gas`/`icetypesbydungeonid.static` |
| `kind` | `enum(ore\|gas\|ice)` | which of the three tables |
| `type_ids` | `list<i64>` | the table's type list |
| `ore_band` | `enum(high\|low\|null\|special)?` | C3 §7.2 rule R4, `ore` rows only; empty for gas and ice |

Invented example row:

```
999102	ore	990003,990006	low
```

### `dungeons/room-rocks.tsv`

- **Writer:** CP10.
- **Readers:** CP16 (and CP10's own check: it equals the room objects).
- **Key:** `(dungeon_id, room_id, type_id)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#dungeon_id	room_id	type_id	count
```

| column | type | source and meaning |
|---|---|---|
| `dungeon_id` | `i64` | `asteroidcountbytypeidroomiddungeonid` key |
| `room_id` | `i64` | room |
| `type_id` | `i64` | ore type |
| `count` | `i64` | rocks of that type in the room |

Invented example row:

```
999102	1	990003	14
```

### `dungeons/agents-in-space.tsv`

- **Writer:** CP10.
- **Readers:** no v2 packet reads it yet (C1 P2 content).
- **Key:** `key`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#key	dungeon_id	system_id	spawn_point_id	type_id
```

| column | type | source and meaning |
|---|---|---|
| `key` | `i64` | `agentsinspace` key (360 rows [V]) |
| `dungeon_id` | `i64` | `dungeonID` |
| `system_id` | `i64` | `solarSystemID` |
| `spawn_point_id` | `i64` | `spawnPointID` |
| `type_id` | `i64` | `typeID` |

Invented example row:

```
3999901	999103	39000001	1	990070
```

### `npc/factions.tsv`

- **Writer:** CP11a (`content_npc.py`).
- **Readers:** CP20 (`npc_data`); CP18 at load (faction ids for warzones).
- **Key:** `faction_id`.
- **Stable client ids and delta keys (ruling 4):** `faction_id` is stable and is the `occupier` value of the `fwvp` delta.

```
#faction_id	corporation_id	militia_corporation_id	system_id	member_races	owner_corp_id	groups	wrecks	damage_profile	ammo_groups	tank	tags	base_prefix	elite_prefix	commander_prefix	deadspace_stems	officers	sof_faction	name
```

| column | type | source and meaning |
|---|---|---|
| `faction_id` | `i64` | `factions` key (27 rows [V]) |
| `corporation_id` | `i64` | `corporationID` (executor) |
| `militia_corporation_id` | `i64?` | `militiaCorporationID` |
| `system_id` | `i64` | `solarSystemID` (HQ) |
| `member_races` | `list<i64>` | `memberRaces` |
| `owner_corp_id` | `i64?` | C6 R1: owner of `Zarzakh <F> Roaming Spawner`, else `corporation_id` [I]. Pirate rows only |
| `groups` | `list<i64>` | R1: category-11 groups the §2.1 grammar parses to this faction; their context and class are in `npc/types.tsv` |
| `wrecks` | `map<str,i64>` | R1: `class=wreck_type_id`, the modal `types.wreckTypeID` per class |
| `damage_profile` | `list<f64>` | R1: em,ex,kin,th, four values |
| `ammo_groups` | `map<i64,i64>` | R1: `size=charge_group_id` (size 1, 2, 3) |
| `tank` | `enum(shield\|armor)?` | R1 |
| `tags` | `list<i64>` | R1: tag type ids, ascending by price |
| `base_prefix` | `str?` | R1 (C6 §2.2) |
| `elite_prefix` | `str?` | R1 |
| `commander_prefix` | `str?` | R1 |
| `deadspace_stems` | `list<str>` | R1: five stems, frigate to battleship |
| `officers` | `list<str>` | R1: officer first names |
| `sof_faction` | `str?` | R1: the modal `sofFactionName` (cross-check only) |
| `name` | `text` | `nameID` through the en-us table |

Invented example row:

```
599001	1999001		39000001	1	1999002	99101,99102	Frigate=990080;Cruiser=990081	0.1,0.4,0.4,0.1	1=99201;2=99202	shield	990090,990091	Example	Arch Example	Dread Example	Exi,Exior,Exum,Exatis,Ex	Exa	examplecorp	Example Cartel
```

- One row for **every** faction. C1's six columns come from the `factions` table, which K1 decodes (checked: `corporationID memberRaces militiaCorporationID nameID solarSystemID` [V]). C6 R1's columns are filled for the factions the §2.1 grammar parses and are empty on every other row.
- `home_pirate` is **not** a column. It is `npc/home-pirate.tsv` (plan §1.2, conflict 4).

### `npc/corporations.tsv`

- **Writer:** CP11a.
- **Readers:** CP20; CP37a and CP11b at extract time (`lp_offer_tables`, station majority); CP26a (agent faction, through `agents.tsv` instead).
- **Key:** `corp_id`.
- **Stable client ids and delta keys (ruling 4):** `corp_id` is stable and is the key of the `lp` delta (`char corp`).

```
#corp_id	faction_id	station_id	system_id	friend_id	enemy_id	tax_rate	min_security	main_activity_id	size	extent	lp_offer_tables	ticker	name
```

| column | type | source and meaning |
|---|---|---|
| `corp_id` | `i64` | `npccorporations` key (283 rows [V]) |
| `faction_id` | `i64?` | `factionID` |
| `station_id` | `i64?` | `stationID` |
| `system_id` | `i64?` | `solarSystemID` |
| `friend_id` | `i64?` | `friendID` |
| `enemy_id` | `i64?` | `enemyID` |
| `tax_rate` | `f64` | `taxRate` |
| `min_security` | `f64?` | `minSecurity` |
| `main_activity_id` | `i64?` | `mainActivityID` (C4 §5.2 uses 5 = military to classify navy tables) |
| `size` | `str` | `size`, a one-letter code (`T S M L H` [V]) |
| `extent` | `str` | `extent`, a one-letter code (`R L N C G` [V]) |
| `lp_offer_tables` | `list<i64>` | `lpOfferTables` |
| `ticker` | `str` | `tickerName` |
| `name` | `text` | `nameID` through the en-us table |

Invented example row:

```
1999001	599001	69000001	39000001			0.0	0.0	5	M	R	9901,9902	EXC	Example Corporation
```

- X1 adds `main_activity_id` to C1 §2.6, for C4 §5.2. `size` and `extent` are single-letter strings, not numbers [V].

### `npc/types.tsv`

- **Writer:** CP11a.
- **Readers:** CP20 (`npc_data`); CP46 (`eve-bot/src/profile.rs`: `role`, `class`).
- **Key:** `type_id`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#type_id	group_id	faction_id	faction_src	context	class	tier	alias_of	needs	role
```

| column | type | source and meaning |
|---|---|---|
| `type_id` | `i64` | a category-11 type in a parsed group |
| `group_id` | `i64` | `types.groupID` |
| `faction_id` | `i64` | C6 R2 faction: `types.factionID` where present (K1), else the group grammar |
| `faction_src` | `enum(type\|group)` | which rule gave `faction_id`. CP11a's check lists the `sofFactionName` disagreements |
| `context` | `enum(Asteroid\|Deadspace\|Mission\|Ghost Sites\|Roaming\|Incursion\|Insurgency)` | C6 §2.1 |
| `class` | `str` | C6 §2.1 class, verbatim (it may hold a space, for example `Commander Frigate`) |
| `tier` | `enum(base\|elite\|commander\|officer\|deadspace)` | C6 R2 prefix rung |
| `alias_of` | `i64?` | the lowest type id with the same `(bounty, hp, attackRange, effects)`, C6 §2.2; empty when the type is its own canonical row |
| `needs` | `list<str>` | C6 R2 capabilities: any of `TURRET MISSILE SCRAM WEB EWAR NEUT SELF_REP REMOTE_REP` |
| `role` | `enum(LOGISTIC\|TACKLER\|EWAR\|DPS)` | C6 R8, first match |

Invented example row:

```
990100	99101	599001	group	Asteroid	Frigate	base		TURRET,SCRAM	TACKLER
```

- The numeric fields of C6 R2 (missile, scram, web, self-rep, timers) are **not** columns here. `npc_data` (CP20) derives them from the type's `typedata.tsv` `attrs` and `effectIDs`, which is what CP20's tests check. This file carries only what needs names, groups or client-only tables. `needs` and `role` are here as well, because the bot (CP46) reads them and must not re-derive server logic.

### `npc/components.tsv`

- **Writer:** CP11a.
- **Readers:** CP20 (a presence index).
- **Key:** `(type_id, component)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#type_id	component
```

| column | type | source and meaning |
|---|---|---|
| `type_id` | `i64` | `spacecomponentsbytype` key |
| `component` | `str` | a component name present on the type, such as `entityBounty` or `reactiveSpawnComponent` [V names] |

Invented example row:

```
990100	entityStandings
```

### `npc/reactive-spawn.tsv`

- **Writer:** CP11a.
- **Readers:** CP20 loads it as optional. No v2 packet acts on it, because belt rats are not reactive spawns in this build (C3 §8.3).
- **Key:** `(type_id, scope, key)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#type_id	scope	key	spawn_table_id	initial_fleet_type_id	owner_id	fleet_counter_table_id	bounty_payout_multiplier	friendly_response_threshold
```

| column | type | source and meaning |
|---|---|---|
| `type_id` | `i64` | a type with `reactiveSpawnComponent` (882 [V]) |
| `scope` | `enum(type\|dungeon\|faction)` | `type`: the component's own fields. `dungeon` or `faction`: one entry of `spawnDataByDungeonID` or `spawnDataByFactionID` |
| `key` | `i64?` | the dungeon or faction id; empty for `type` |
| `spawn_table_id` | `i64?` | `spawnTableID` (out of corpus, verbatim) |
| `initial_fleet_type_id` | `i64?` | `initialSpawnNpcFleetTypeID` |
| `owner_id` | `i64?` | `ownerID` |
| `fleet_counter_table_id` | `i64?` | `fleetCounterTableID` |
| `bounty_payout_multiplier` | `f64?` | `bountyPayoutMultiplier` (`type` rows) |
| `friendly_response_threshold` | `f64?` | `friendlyResponseThreshold` (`type` rows) |

Invented example row:

```
990110	type		9001		1999001	901	1.0	0.0
```

- X1 adds the scope `type`. C1 had only `dungeon|faction`, but 789 components carry a top-level `spawnTableID`, 110 a `spawnDataByDungeonID` and 38 a `spawnDataByFactionID` [V]. Each entry has `fleetCounterTableID initialSpawnNpcFleetTypeID ownerID spawnTableID` [V].

### `npc/bounty.tsv`

- **Writer:** CP11a.
- **Readers:** CP20 (`npc_data`), and through it CP22 (squad budget), CP38 and CP26b (the reward base).
- **Key:** `type_id`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#type_id	bounty	multipliers
```

| column | type | source and meaning |
|---|---|---|
| `type_id` | `i64` | a category-11 type with a bounty or a multiplier |
| `bounty` | `f64` | `typedogma` attribute 481 `entityKillBounty`; 0 when absent (CP11a's "the bounty row comes from attribute 481") |
| `multipliers` | `map<i64,f64>` | `spacecomponentsbytype.entityBounty.bountyMultipliersByOwner`, `owner=mult` (175 types [V]) |

Invented example row:

```
990100	12500.0	1999001=0.5
```

- C1's `type_id owner_key multiplier` and CP11a's attribute-481 row are one record kind here, because both are keyed by type. Checked: `entityBounty` holds exactly one field, `bountyMultipliersByOwner`, a `dict<int,float>` [V].

### `npc/regions.tsv`

- **Writer:** CP11b (`content_npc_regions.py`), with `content/npc/region-pirates.tsv` applied.
- **Readers:** CP20 (`pirate(system)`), and through it CP21, CP26a and CP30.
- **Key:** `(region_id, faction_id)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#region_id	faction_id	weight	rule
```

| column | type | source and meaning |
|---|---|---|
| `region_id` | `i64` | region |
| `faction_id` | `i64` | a pirate faction for it |
| `weight` | `f64` | relative weight |
| `rule` | `enum(0\|1\|2\|3\|4\|secondary\|override)` | the C6 R3 step that produced the row, or the override file |

Invented example row:

```
19000001	599001	1.0	2
```

### `npc/police.tsv`

- **Writer:** NW-7, `tools/content_police.py` (section `police`).
- **Readers:** `npc_data::Table::police`, `concord::police_of` and `is_law_owner`.
- **Key:** `faction_id`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#faction_id	warning_key	type_ids
```

| column | type | source and meaning |
|---|---|---|
| `faction_id` | `i64` | the faction whose systems this force polices; 500006 (CONCORD Assembly) is the default force, owned by CONCORD itself |
| `warning_key` | text | the chat warning the pilot gets; the faction's own key where the client has one, else `chatDefaultPoliceWarning` [A] |
| `type_ids` | `i64` list | the group-182 types of the force, chosen by the name prefix in the tool; the lead is found at run time by its effects |

The owner of a faction's squad is that faction's `corporation_id` in `npc/factions.tsv`.

Invented example row:

```
599002	chatFixturePoliceWarning	990330011,990330012
```

### `npc/concord.tsv`

- **Writer:** `tools/content_police.py` (section `police`), beside `npc/police.tsv`.
- **Readers:** `npc_data::Table::concord_roles`, `concord::kind_for`.
- **Key:** `role`.

```
#role	type_id
```

| column | type | source and meaning |
|---|---|---|
| `role` | text | `executioner`, `captain`, `officer`, `special_frigate`, `special_battleship`, `swat_frigate` or `swat_battleship`: the roles `concord.rs` names |
| `type_id` | `i64` | the type whose name is exactly the role's in the tool's table; a role with no type has no row, and the server flies an invented fixture |

Invented example row:

```
officer	990330003
```

### `npc/home-pirate.tsv`

- **Writer:** CP11b.
- **Readers:** CP20 (`npc_data::home_pirate`), and through it CP19 and CP48.
- **Key:** `faction_id`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#faction_id	pirate_faction_id
```

| column | type | source and meaning |
|---|---|---|
| `faction_id` | `i64` | an offering (empire or minor) faction |
| `pirate_faction_id` | `i64` | its home pirate: the argmax of C4 §2.4's faction-locked kill cross-tab; vassals take their parent's (C4 §3.2) |

Invented example row:

```
599002	599001
```

- **New in v2** (plan §1.2): it was a column of `factions.tsv` in v1. It is its own file so that CP11a and CP11b never write the same file.

### `npc/trades.tsv`

- **Writer:** CP11b.
- **Readers:** CP35 (`econ_exchange`, Tier A; under E5b the sign of `value` is the side, positive an NPC ask and negative an NPC bid at every station of the corporation, in a category whitelist [V-TQ, R-NPCTRADE]); CP24 at extract time.
- **Key:** `(corp_id, type_id)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#corp_id	type_id	value
```

| column | type | source and meaning |
|---|---|---|
| `corp_id` | `i64` | an NPC corporation with `corporationTrades` (184 [C5 §2.3]) |
| `type_id` | `i64` | `corporationTrades` key |
| `value` | `f64` | `corporationTrades` value in [-1, 1]. The sign is supply or demand [I]; the magnitude is unused |

Invented example row:

```
1999001	990001	0.083
```

- Checked: `corporationTrades` decodes as `dict<int,float>` with K1's generic reader [V].

### `npc/loot.tsv`

- **Writer:** CP11b.
- **Readers:** CP20, and through it CP22 (`npc/loot.rs`, C6 R7).
- **Key:** `(faction_id, size, kind, tier, type_id)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#faction_id	size	kind	tier	type_id	meta_level
```

| column | type | source and meaning |
|---|---|---|
| `faction_id` | `i64` | pirate faction |
| `size` | `i64` | 1, 2 or 3 (C6 R7) |
| `kind` | `enum(weapon\|tank\|prop\|ewar\|ammo\|faction\|officer\|deadspace\|tag\|clone-tag)` | the R7 pool; a `clone-tag` row (SEC-1c) has the Clone Soldier rat's **type** in `faction_id`, size 0, tier `base`, and the tag it leaves in `type_id` |
| `tier` | `enum(base\|elite\|commander\|officer\|deadspace)` | the R2 rung the pool serves |
| `type_id` | `i64` | a grantable type |
| `meta_level` | `i64?` | `types.metaLevel` |

Invented example row:

```
599001	1	weapon	base	990120	0
```

### `agents/agents.tsv`

- **Writer:** CP13 (`content_agents.py`).
- **Readers:** CP19 (`mission_pool`); CP26a; CP26b (`GetAgents` rowset in `service/agents.rs`); CP41; CP48.
- **Key:** `agent_id`.
- **Stable client ids and delta keys (ruling 4):** **`agent_id`** is part of the `mission` delta key (`char agent`).

```
#agent_id	agent_type_id	division_id	level	corp_id	faction_id	station_id	system_id	is_locator	storyline_missions
```

| column | type | source and meaning |
|---|---|---|
| `agent_id` | `i64` | `npccharacters` key, rows with an `agent` struct (10,972 [V]) |
| `agent_type_id` | `i64` | `agent.agentType` |
| `division_id` | `i64` | `agent.agentDivisionID` |
| `level` | `i64` | `agent.agentLevel` |
| `corp_id` | `i64` | `corporationID` |
| `faction_id` | `i64` | the corporation's `npccorporations.factionID` (all resolve [C4 §2.1]) |
| `station_id` | `i64?` | `stationID` (10,970 have one) |
| `system_id` | `i64?` | the station's system, through `solarsystemcontent` |
| `is_locator` | `bool` | `agent.agentLocateCharacter` |
| `storyline_missions` | `list<i64>` | `agent.storyLineMissions`, in the client's stored order (hash order, C4 §8) |

Invented example row:

```
3999001	2	24	1	1999001	599002	69000001	39000001	0	
```

- **C4 §3.1 wins over C1 §2.7.** C1's `locate gender bloodline_id name` are dropped. C4's `factionID`, `solarSystemID` and `isLocator` (`GetAgents` columns, `service.rs` `agent_rowset`) are in. `quality` is not a column: it is always 0 (C4 §2.1).
- C4's names are written in the house snake_case (`agentID` is `agent_id`). `corporationID` and `solarSystemID` become `corp_id` and `system_id`, to match every other file. The order is C4's.
- Checked with `fsd.rows(c,'npccharacters')`: the agent struct has exactly `agentDivisionID agentLevel agentType agentLocateCharacter storyLineMissions`. The row has no `factionID` and no `solarSystemID`, so both are joins [V].

### `agents/missions.tsv`

- **Writer:** CP13.
- **Readers:** CP19 (the pool and chains); CP26a (`Spec` per mission, pocket enemy); CP26b (rewards); CP41 (arc missions); CP48 (storyline pool).
- **Key:** `mission_id`.
- **Stable client ids and delta keys (ruling 4):** **`mission_id`** is the stored value of the `mission` delta and the inner key of the `arc` delta.

```
#mission_id	name_id	template	family	level	level_src	offer_faction_id	enemy_faction_id	dungeon_id	obj_type_id	obj_qty	obj_volume_total	obj_group_id	drop_item_type_id	expire_minutes	agent_type_id	corp_id	chain_key	chain_index	chain_len	has_next_slot	gift_type_id	gift_qty	reward_type_id	reward_qty	bonus_type_id	bonus_qty	bonus_minutes	fixed_lp	has_standing_rewards	extra_standings	content_tags	messages
```

| column | type | source and meaning |
|---|---|---|
| `mission_id` | `i64` | `missions` key (2,892 [V]) |
| `name_id` | `i64` | `nameID` |
| `template` | `str` | `contentTemplate` minus the `agent.missionTemplatizedContent_` prefix and the `Mission` suffix, for example `BasicKill` (18 values [V]) |
| `family` | `enum(kill\|courier\|mining\|trade\|talk\|other)` | from `template`: `…Kill`, `…Courier`, `…Mining`, `…Trade`; `EpicArcTalkToAgent` and `EpicArcAgentInteraction` are `talk`; `Heraldry` is `other` |
| `level` | `i64?` | C4 §2.3 |
| `level_src` | `enum(difficulty\|volume_band\|none)` | C4 §3.1 |
| `offer_faction_id` | `i64?` | `factionID` |
| `enemy_faction_id` | `i64?` | `dungeons.factionID` of `dungeon_id` |
| `dungeon_id` | `i64?` | `killMission.dungeonID` |
| `obj_type_id` | `i64?` | `courierMission.objectiveTypeID`, else `killMission.objectiveTypeID` |
| `obj_qty` | `i64?` | the same struct's `objectiveQuantity` |
| `obj_volume_total` | `f64?` | `obj_qty × types.volume` (C4 §2.3) |
| `obj_group_id` | `i64?` | `killMission.objectiveGroupID` (4 set [V]) |
| `drop_item_type_id` | `i64?` | `killMission.dropItemInMissionContainer` (3 set [V]) |
| `expire_minutes` | `i64?` | `expirationTime`, verbatim. 1,020 set, 188 of them 0 [V]; a reader treats a value ≤ 0 as absent [I] |
| `agent_type_id` | `i64?` | `agentTypeID` |
| `corp_id` | `i64?` | `corporationID` |
| `chain_key` | `str?` | C4 §2.5: the series key, lowercased |
| `chain_index` | `i64?` | k in "(k of n)" |
| `chain_len` | `i64?` | n |
| `has_next_slot` | `bool` | `messages` has `…completed.nextmission.agentsays` |
| `gift_type_id` | `i64?` | `initialAgentGiftTypeID` |
| `gift_qty` | `i64?` | `initialAgentGiftQuantity` |
| `reward_type_id` | `i64?` | `missionRewards.reward.rewardTypeID` |
| `reward_qty` | `i64?` | `missionRewards.reward.rewardQuantity` |
| `bonus_type_id` | `i64?` | `missionRewards.bonusReward.rewardTypeID` |
| `bonus_qty` | `i64?` | `missionRewards.bonusReward.rewardQuantity` |
| `bonus_minutes` | `i64?` | `missionRewards.bonusTimeInterval` (values 1-45+ [V]; minutes [I]) |
| `fixed_lp` | `i64` | `fixedLpRewardOmega` (see the note) |
| `has_standing_rewards` | `bool` | `hasStandingRewards` |
| `extra_standings` | `map<i64,f64>` | `extraStandings`, `faction=value` (41 set [V]) |
| `content_tags` | `list<str>` | `contentTags` |
| `messages` | `map<str,i64>` | `messages`, `slot=message_id`, where the slot is the full client key such as `messages.mission.briefing` |

Invented example row:

```
999001	999901	BasicKill	kill	1	difficulty		599001	999101												0								0	0			messages.mission.briefing=999902
```

- **C4 §3.1 wins over C1 §2.7.** C1's `kind`, `division_id`, `system_id`, `station_id` and `lp_alpha`/`lp_omega` are replaced by C4's columns, in C4's order and in snake_case.
- **Checked: `fixedLpRewardAlpha` and `fixedLpRewardOmega` are two fields.** They are non-zero on the same 78 missions and differ on all 78 [V]. C4 has one `fixedLP`, so `fixed_lp` is the Omega value [A]: eve-offline has no clone states, and Omega is the full reward. Falsified by a capture showing an Omega pilot paid the Alpha number.
- Checked: `missionRewards` is `{reward, bonusReward, bonusTimeInterval}`, both rewards are `{rewardQuantity, rewardTypeID}`, and `killMission` is `{dungeonID, objectiveTypeID, objectiveQuantity, objectiveGroupID, dropItemInMissionContainer}` [V]. 73 missions have neither a kill nor a courier struct; they are the `talk` and `trade` families.
- X1 widens C4's `family` set with `trade` and `other`, because the client ships the `BasicTrade`, `…TradeMission` and `HeraldryMission` templates [V].

### `agents/mission-messages.tsv`

- **Writer:** CP13.
- **Readers:** CP41 (`GetMessagesForEpicArcMissions` is `{slot: {missionID: messageID}}`, C4 §7.2).
- **Key:** `(mission_id, slot)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#mission_id	slot	message_id
```

| column | type | source and meaning |
|---|---|---|
| `mission_id` | `i64` | mission |
| `slot` | `str` | the full `messages` key, for example `messages.epicMission.journalText.chapterTitle` |
| `message_id` | `i64` | the localisation message id |

Invented example row:

```
999001	messages.mission.briefing	999902
```

- The same pairs as `missions.tsv`'s `messages` column, in long form, because C4 §3.1 (which wins) keeps the column and plan §1.2 keeps this file. CP13's `check` asserts that the two agree pair for pair. There is one writer, so there is one truth.

### `agents/agent-types.tsv`

- **Writer:** CP13.
- **Readers:** no v2 packet reads it (agent type ids are protocol vocabulary in Rust, plan §2.1 class A); kept for operator joins.
- **Key:** `agent_type_id`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#agent_type_id	name
```

| column | type | source and meaning |
|---|---|---|
| `agent_type_id` | `i64` | `agenttypes` key (13 rows [V]) |
| `name` | `text` | the scalar row value: `agenttypes` rows are bare `str`s [V] |

Invented example row:

```
99	ExampleAgent
```

- C1 said "scalar row". Checked: each row is a `str` [V].

### `agents/divisions.tsv`

- **Writer:** CP13.
- **Readers:** no v2 packet reads it (division → kind is a Rust const, C4 §3.2); kept for operator joins.
- **Key:** `division_id`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#division_id	name_id	leader_type_name_id	internal_name
```

| column | type | source and meaning |
|---|---|---|
| `division_id` | `i64` | `npccorporationdivisions` key (10 rows [V]) |
| `name_id` | `i64` | `nameID` |
| `leader_type_name_id` | `i64?` | `leaderTypeNameID` |
| `internal_name` | `text` | `internalName` |

Invented example row:

```
99	999910	999911	Example
```

- Checked: the fields are `description descriptionID internalName leaderTypeNameID nameID` [V]. `description` is dropped (free text that no packet reads).

### `agents/epic-arcs.tsv`

- **Writer:** CP13.
- **Readers:** CP41 (`epic_arc.rs`).
- **Key:** `(arc_id, mission_id)`.
- **Stable client ids and delta keys (ruling 4):** **`arc_id`** is part of the `arc` delta key (`char arc_id`).

```
#arc_id	mission_id	agent_id	next_missions	fail_mission_id	restart_minutes	faction_id	name_id
```

| column | type | source and meaning |
|---|---|---|
| `arc_id` | `i64` | `epicarcs` key (21 rows [V]) |
| `mission_id` | `i64` | a `missions` dict key (u32, C4 §7.1) |
| `agent_id` | `i64` | `missions[m].agentID` |
| `next_missions` | `list<i64>` | `missions[m].nextMissions` |
| `fail_mission_id` | `i64?` | `missions[m].failMissionID` |
| `restart_minutes` | `i64` | the arc's `arcRestartInterval` (minutes [I], C4 §7.1), repeated per row |
| `faction_id` | `i64?` | the arc's `factionID` |
| `name_id` | `i64` | the arc's `epicArcNameID` |

Invented example row:

```
9901	999001	3999001	999002,999003		1000	599001	999920
```

- C1 §2.7 had the five per-mission columns. X1 adds C4 §7.1's arc fields, repeated on every row (one record kind per file), because CP41 needs the restart interval and `IsValidArc` hides arcs whose interval is ≤ 1. Checked: the arc fields are `arcRestartInterval comments epicArcNameID factionID iconID missions`, and a mission entry is `{agentID, failMissionID, nextMissions}` [V].

### `agents/schools.tsv`

- **Writer:** CP13 (header frozen), written by P-START-X, which added `starter_space`.
- **Readers:** P-START-1 (`start.rs`: a new character starts at its school's first `starting_stations` entry, `characterdata/schools.py` `get_school_starting_station` [V]); E5 and E5b (`econ_exchange`: the school corporation's skillbooks are also NPC-sold at each `starting_stations` entry of a non-starter-space school, while `SCHOOL_BOOKS`; every listing corporation sells them at all its stations).
- **Key:** `school_id`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#school_id	corp_id	race_id	career_id	system_id	starting_stations	career_agents	name_id	starter_space
```

| column | type | source and meaning |
|---|---|---|
| `school_id` | `i64` | `schools` key (23 rows [V]) |
| `corp_id` | `i64` | `corporationID` |
| `race_id` | `i64` | `raceID` |
| `career_id` | `i64?` | `careerID` |
| `system_id` | `i64?` | `schoolmap.solarSystemID` for this school (12 rows [V]) |
| `starting_stations` | `list<i64>` | `startingStations` |
| `career_agents` | `list<i64>` | `careerAgents` |
| `name_id` | `i64` | `nameID` |
| `starter_space` | `bool` | `is_starter_space_school` (absent is `0`). The client offers a race only its non-starter-space schools (`get_school_ids_by_race_id`) [V]; these rows have no `schoolmap` row either, but the flag is the client's own signal, the empty `system_id` only an inference |

Invented example row:

```
99	1999001	1		39000001	69000001	3999001	999001	0
```

- Checked: `schools` and `schoolmap` are cFSD tables, not `.static`. `schools` has `careerAgents careerID corporationID … nameID raceID startingStations titleID`, and `schoolmap` has `schoolID solarSystemID` [V]. C1 §2.7 joined only `system_id`. X1 adds `starting_stations` and `career_agents`, because they are what "agree on the start station" needs. P-START-X: `schoolmap`'s keys are row indices, not school ids; the join is on its `schoolID` field [V].

### `agents/races.tsv`

- **Writer:** P-START-X.
- **Readers:** E7 (`starter_ship::TABLE`: `ship_type_id`, the corvette `dogmaIM.CreateNewbieShip` mints); lane start-skills (`start::TABLE`: `skills`, the skill sheet `Registry::create_character` gives a new character of that race, R-START Q5). A level-0 entry is a skill injected untrained.
- **Key:** `race_id`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#race_id	ship_type_id	skills	name_id
```

| column | type | source and meaning |
|---|---|---|
| `race_id` | `i64` | `races` key |
| `ship_type_id` | `i64?` | `shipTypeID`: the race's rookie ship; absent for a race with none |
| `skills` | `map<i64,i64>` | `skills`: starter skill type id = level, sorted by type id; empty for a race with none |
| `name_id` | `i64` | `nameID` |

Invented example row:

```
99	990001	990101=3;990102=1	999001
```

- Checked: `races` is a cFSD table with `skills` (a `dict` whose keys read as ints) and an optional `shipTypeID` [V, `findings/pyd-schemas.tsv`].

### `agents/bloodlines.tsv`

- **Writer:** lane start-skills (`tools/content_agents.py`).
- **Readers:** lane start-skills (`start::TABLE`: `CreateCharacterWithDoll` refuses a bloodline that is not of the chosen race, R-START §2.3).
- **Key:** `bloodline_id`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#bloodline_id	race_id	corp_id	name_id
```

| column | type | source and meaning |
|---|---|---|
| `bloodline_id` | `i64` | `bloodlines` key |
| `race_id` | `i64` | `raceID` |
| `corp_id` | `i64` | `corporationID` |
| `name_id` | `i64` | `nameID` |

Invented example row:

```
901	99	1999001	999011
```

- Checked: `bloodlines` is a cFSD table with `charisma corporationID descriptionID iconID intelligence memory nameID perception raceID willpower` [V, `findings/pyd-schemas.tsv`]. Every row is written, the NPC-only bloodlines included; the client offers a race its player bloodlines only, so the server checks the race and nothing else.

### `agents/npc-characters.tsv`

- **Writer:** CP13.
- **Readers:** no v2 packet reads it (CEOs and employment rows, for a later owner-lookup packet).
- **Key:** `character_id`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#character_id	corp_id	station_id	race_id	bloodline_id	ancestry_id	gender	ceo	school_id	career_id	name_id
```

| column | type | source and meaning |
|---|---|---|
| `character_id` | `i64` | an `npccharacters` key **without** an `agent` struct (452 [V: 11,424 rows minus 10,972 agents]) |
| `corp_id` | `i64` | `corporationID` |
| `station_id` | `i64?` | `stationID` |
| `race_id` | `i64?` | `raceID` |
| `bloodline_id` | `i64?` | `bloodlineID` |
| `ancestry_id` | `i64?` | `ancestryID` |
| `gender` | `i64` | `gender` |
| `ceo` | `bool` | `ceo` |
| `school_id` | `i64?` | `schoolID` |
| `career_id` | `i64?` | `careerID` |
| `name_id` | `i64` | `nameID` |

Invented example row:

```
3999101	1999001		1	1	1	1	1			999930
```

- C1 §2.7 named the file but gave no columns. These are the `npccharacters` fields a server could need [V key set].

### `agents/lp-offers.tsv`

- **Writer:** CP37a (`content_lp.py`), with `content/agents/lp-overrides.tsv` applied.
- **Readers:** CP37b (`loyalty.rs`, `GetAvailableOffersFromCorp`).
- **Key:** `offer_id`.
- **Stable client ids and delta keys (ruling 4):** none. `offer_id` reaches the wire (`TakeOfferForCharacter`) but is never persisted.

```
#table_id	offer_id	type_id	qty	lp_cost	isk_cost	req_items
```

| column | type | source and meaning |
|---|---|---|
| `table_id` | `i64` | an `npccorporations.lpOfferTables` id |
| `offer_id` | `i64` | `table_id × 10,000 + n`, where `n` counts the table's offers **in ascending `type_id`**, so an id stays put while the type set does (C4 §5.2) |
| `type_id` | `i64` | the product |
| `qty` | `i64` | product quantity |
| `lp_cost` | `i64` | C4 §5.2 / C5 §5.6 rule |
| `isk_cost` | `i64` | the same rule |
| `req_items` | `map<i64,i64>` | `type_id=qty` required items |

Invented example row:

```
9901	99010000	990001	1	2500	2500000	990002=1
```

- The plan's name is `lp-offers.tsv` (C4 wrote `lp_offers.tsv`). The columns are C4 §5.2's, in snake_case.
- **CAP-LOOT adds the capital-commodity condensers** (`content_lp.py`, `condenser_offers`): one `add` row per militia corporation, found by corporation and type *name* never by id, in the lowest-numbered `fw` table used only by that corporation: 600 LP and 500,000 ISK (`FW_CONDENSER_LP`, `FW_CONDENSER_ISK` [I]), qty 1, no required items. Authored `lp-overrides.tsv` rows still win.

### `industry/blueprints.tsv`

- **Writer:** CP24 (`content_econ.py`).
- **Readers:** CP33b (`industry::Table`); CP24 itself at extract time (the recipe rule, C5 §3.2 step 1); E5b (`econ_exchange`: no NPC bid on a `product` type, C5 §6 invariant 2).
- **Key:** `(bp_type_id, activity_id, kind, type_id)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#bp_type_id	activity_id	kind	type_id	quantity	probability	time	max_runs
```

| column | type | source and meaning |
|---|---|---|
| `bp_type_id` | `i64` | `blueprintTypeID` (5,100 [industry.md §2.1]) |
| `activity_id` | `i64` | the activity id for the `activities` key, through `industry_activities.static` (1, 3, 4, 5, 8, 9 [V, industry.md §2.2]) |
| `kind` | `enum(activity\|material\|product\|skill)` | `activity` is one row per activity, with `type_id` 0 and `quantity` 0, so an activity with no lists still exists |
| `type_id` | `i64` | material, product or skill type; 0 on `activity` rows |
| `quantity` | `i64` | the quantity; for `skill`, the level |
| `probability` | `f64?` | `products[].probability` (invention); empty otherwise |
| `time` | `i64` | the activity's `time`, in seconds |
| `max_runs` | `i64` | `maxProductionLimit` |

Invented example row:

```
990200	1	material	990030	2000		6000	300
```

- X1 adds `kind = activity` to C1 §2.8's set, so that `copying`, which may have no materials, keeps a row.

### `stations/stations.tsv`

- **Writer:** CP24.
- **Readers:** CP35 (station index, owner, Tier A); CP32b (`reproc_eff`, `reproc_take`); CP31 at extract time.
- **Key:** `station_id`.
- **Stable client ids and delta keys (ruling 4):** **`station_id`** is part of the `npcorder` and `npcmult` delta keys. The NPC order id uses the row's index in this file, which is sorted by `station_id` (C5 §4.1). That index is computed and never persisted.

```
#station_id	system_id	region_id	owner_id	operation_id	services	reproc_eff	reproc_take
```

| column | type | source and meaning |
|---|---|---|
| `station_id` | `i64` | `mapObjects.db npcStations` (5,210 [V]); rows sorted ascending |
| `system_id` | `i64` | its system |
| `region_id` | `i64` | its region |
| `owner_id` | `i64` | the owning NPC corporation |
| `operation_id` | `i64` | `stationoperations` key |
| `services` | `list<i64>` | the operation's `stationservices` ids |
| `reproc_eff` | `f64` | reprocessing efficiency |
| `reproc_take` | `f64` | reprocessing station take |

Invented example row:

```
69000001	39000001	19000001	1999001	99	7,21,25	0.5	0.05
```

- C5 §3.3 wrote `services` as a bitmask. X1 makes it a list of service ids: it is the same information, and a list needs no bit convention.

### `stations/operations.tsv`

- **Writer:** CP24.
- **Readers:** CP35 and CP32b (through `stations.tsv`'s `operation_id`).
- **Key:** `operation_id`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#operation_id	activity_id	services	station_types	manufacturing_factor	research_factor
```

| column | type | source and meaning |
|---|---|---|
| `operation_id` | `i64` | `stationoperations` key (69 rows [V]) |
| `activity_id` | `i64` | `activityID` |
| `services` | `list<i64>` | `services` |
| `station_types` | `map<i64,i64>` | `stationTypes`, `race=station_type_id` |
| `manufacturing_factor` | `f64` | `manufacturingFactor` |
| `research_factor` | `f64` | `researchFactor` |

Invented example row:

```
99	1	7,21,25	1=990002	1.0	1.0
```

- Checked: `stationTypes` is `dict<int,int>` and `services` is a list [V]. `border corridor fringe hub ratio` are not written, because their meaning is unknown (C5 §2.4).

### `econ/prices.tsv`

- **Writer:** CP24.
- **Readers:** CP31 (`econ_prices`: `P_ref`), and through it CP32b, CP33b, CP35 and CP44.
- **Key:** `type_id`.
- **Stable client ids and delta keys (ruling 4):** `type_id` is the stable id of the `mktidx` delta.

```
#type_id	ref_price	source	floor_price	category_id	group_id	meta_group_id	portion_size
```

| column | type | source and meaning |
|---|---|---|
| `type_id` | `i64` | a priced market type |
| `ref_price` | `f64` | `P_ref`, C5 §3.2 |
| `source` | `enum(recipe\|reaction\|pi\|base\|parent\|override)` | the rule that priced it |
| `floor_price` | `f64` | the step-6 floor |
| `category_id` | `i64` | category |
| `group_id` | `i64` | group |
| `meta_group_id` | `i64?` | `metaGroupID` |
| `portion_size` | `i64` | `portionSize` |

Invented example row:

```
990001	256000.0	recipe	120000.0	6	99001	1	1
```

- C5 §3.3's columns in snake_case. The plan's path is `econ/prices.tsv`, not C5's top-level `prices.tsv`.

### `explore/loot.tsv`

- **Writer:** HK3 (`content_explore.py`), from `types/typedata.tsv`, `types/market.tsv`, `industry/blueprints.tsv` and `econ/prices.tsv` at extract time.
- **Readers:** HK3 (`hacking_loot.rs`: what a hacked data or relic can holds).
- **Key:** `(kind, class, type_id)`.
- **Stable client ids and delta keys (ruling 4):** none.
- **The whole table is [A]** (R-HACK §3): the client ships no loot table for hackable cans. The classes are chosen by group and category [V]; their weights and quantities are the extractor's `CLASSES` [A].

```
#kind	class	type_id	weight	qty_min	qty_max	copy	value
```

| column | type | source and meaning |
|---|---|---|
| `kind` | `enum(data\|relic\|ancient)` | the can kind the row belongs to |
| `class` | `str` | a loot class (`datacore`, `decryptor`, `data_interface`, `bpc`, `filler`, `salvage`, `rig_bpc`, `relic_component`, `ancient_salvage`, `t3_relic`, `t3_datacore`), or `can`: then `type_id` is a hackable can type (`hackable` 1927 = 1) and the row only says which kind it rolls; its other columns are 0 |
| `type_id` | `i64` | the loot type, or the can type on a `can` row |
| `weight` | `i64` | the class's weight, the same on every row of the class: a roll picks a class by it, then a type in it |
| `qty_min` | `i64` | the stack's least quantity; a copy's least runs |
| `qty_max` | `i64` | the most |
| `copy` | `bool` | `1`: one blueprint copy whose runs are `qty_min..qty_max` |
| `value` | `f64` | `P_ref` from `econ/prices.tsv`, else `basePrice`, else 0: the type's weight in its class grows with it as the can's tier rises |

Invented example rows:

```
data	datacore	990001	40	1	3	0	0.0
data	bpc	990002	20	1	10	1	5000.0
data	can	990003	0	0	0	0	0.0
```

- Rows are sorted by kind (`data`, `relic`, `ancient`), class and type; the `can` rows come last.
- A loot type the server's typedata does not hold cannot be minted and is skipped at roll time. HK3 adds categories 34 (ancient relics) and 35 (decryptors) to `tools/typedata.py`'s `CATEGORIES` for that reason.
- **T3-A: `t3_datacore`** is a new `ancient` class: the published group-333 datacores whose name contains `T3_DATACORE_NAMES` ("Subsystems Engineering"): the four cores (Defensive, Offensive, Propulsion, Core) that Tech 3 relic invention consumes and nothing else yields. Weight `T3_DATACORE_WEIGHT` 20 [A], qty 1..3, not a copy. `DATACORE_EXCLUDED` still keeps them out of ordinary data cans. Falsifier: retail Sleeper data sites that drop other things, or an NPC seller of these cores. No Rust change: `hacking_loot::roll` takes any class name.

### `explore/site-loot.tsv`

- **Writer:** CAP-LOOT (`content_explore.py`), from `types/typedata.tsv`, `dungeons/dungeons.tsv`, `dungeons/objects.tsv` and `npc/factions.tsv`.
- **Readers:** CAP-LOOT (`hacking_loot.rs`: what a hacked can of a given dungeon adds to its roll).
- **Key:** `(dungeon_id, can_type_id, type_id)`.
- **Stable client ids and delta keys (ruling 4):** none.
- **The whole table is [A]**: CCP never published drop rates for the eleven capital commodities. Sites are found **by the names** of their hackable cans and their faction's name, never by id; the rates are `content_explore.SITE_*_PPM`.

```
#dungeon_id	can_type_id	type_id	chance_ppm	qty_min	qty_max	bands
```

| column | type | source and meaning |
|---|---|---|
| `dungeon_id` | `i64` | `dungeons/dungeons.tsv` |
| `can_type_id` | `i64` | the hackable can type the row applies to; `0` = any hackable can of the dungeon |
| `type_id` | `i64` | the item dropped |
| `chance_ppm` | `i64` | one independent draw per hacked can, parts per million (`1000000` = always) |
| `qty_min`, `qty_max` | `i64` | the stack's quantity |
| `bands` | `str` | the security bands it applies in: letters of `h` (high), `l` (low), `n` (null), `w` (wormhole); empty = all |

Invented example rows:

```
998001	0	991001	5000	1	1
998002	991201	991002	1000000	1	1	hl
```

- The rows are drawn after the class roll of `explore/loot.tsv`, on the same stream, so a bundle with no site file rolls as before. An absent file is no rows.

### `store/catalogue.tsv` and `store/types.tsv` (L1 NES-BACKEND)

- **Writer:** `tools/content_store.py`, the `store` section of `content.py` (so `up.sh` extracts it into whichever bundle it selects and re-extracts when an authored input changes). Inputs: the authored `content/store/{offers,categories}.tsv`, and the client's `skinlicenses.static`, `skins.static`, `types` and `groups`.
- **Readers:** L1 (`nes.rs`: the catalogue behind `storeManager`, and the item types `item::find_in` falls through to). Both files are optional; with none the store answers its placeholder.
- **Key:** `catalogue.tsv` has one row per **product**; rows of one `key` are one offer (the offer columns repeat). `types.tsv`: `type_id`.
- **Stable client ids and delta keys (ruling 4):** `offer_id` is FNV-1a of `key`, 31 bits, refused on a collision [A]. Delta kinds `nes` (the vault) and `nestok` (the redeem queue) key by account and token, never by offer id.
- **The prices and the SKIN cap are [A]** (owner decision, issue #35: SKINs 50/75/125/175/250/500 PLEX by hull class, extractor 100, bonus remap 150; no hulls, no MCT, no injectors). Falsifier: the owner's price list.

```
#key	offer_id	category	name	description	price_plx	base_plx	single	type_id	qty	service
#type_id	group_id	category_id	singleton	volume	name
```

| column | type | meaning |
|---|---|---|
| `key` | `str` | the authored offer key; a SKIN offer is `<key>:<skinID>` |
| `offer_id` | `i64` | `fnv31(key)` |
| `category` | `str` | a path of `content/store/categories.tsv` |
| `name`, `description` | `str` | the client's own type name and text (a service: our words); one line |
| `price_plx`, `base_plx` | `i64` | whole PLEX |
| `single` | `0/1` | bought once per account (SKINs) |
| `type_id`, `qty` | `i64` | the item delivered and how many; `type_id` 0 is a service |
| `service` | `str` | empty for an item; `bonus_remap` |

Invented example rows:

```
tool	5001	services/tools	Tool	A tool.	100	100	0	990901	1
remap	5003	services/char	Remap	A remap.	150	150	0	0	1	bonus_remap
990901	90003	90004	0	0.01	Tool
```

### `content/store/*.tsv` (authored)

```
key	category	selector	qty	price_plx	base_plx	single	why
path	name	why
```

- `offers.tsv`: `selector` is `const:<name>` (`content_store.CONSTS`), `service:<name>` (`content_store.SERVICES`) or `skin:hullclass=<class>:permanent[:cap=N]` (one offer per permanent SKIN licence; `cap` keeps the first N by SKIN id [A]).
- `categories.tsv`: the tree. A root with children is a category; a childless root is a tag on the client.
- The ship groups of each SKIN price class are `content_store.HULL_CLASSES` (class A vocabulary, in code). A SKIN whose hulls straddle two classes is not sold.

### `skins/licenses.tsv` and `skins/skins.tsv` (L2 SKINS)

- **Writer:** `tools/content_skins.py`, the `skins` section of `content.py` (extracted by `up.sh` into whichever bundle it selects, so each client build has its own: SKIN ids swapped meaning between builds). Inputs: the client's `skinlicenses.static`, `skins.static`, `types` and `groups`. `skinmaterials.static` is not extracted (the client resolves materials itself).
- **Readers:** L2 (`skins.rs`: the licence item types `item::find_in` falls through to, the licence-to-SKIN map, the hulls a SKIN fits). Both files are optional; with none no SKIN exists.
- **Persisted state is keyed by licence type, never by SKIN id** (port review r-port-diff §4): delta kind `skin`, `lic char lic_type -> expires single_use` (FILETIME, `0` permanent) and `app ship -> lic_type kept`.

```
#type_id	group_id	skin_id	duration	single_use	volume	name
#skin_id	visible	structure	hulls
```

| column | type | meaning |
|---|---|---|
| `type_id` | `i64` | the licence item type (category 91) |
| `group_id`, `volume`, `name` | | the type's own group, volume and en-us name (`typedata.tsv` has no SKIN licences) |
| `skin_id` | `i64` | the SKIN it unlocks; a row is refused if `skins.tsv` lacks it |
| `duration` | `i64` | days; `-1` is permanent |
| `single_use` | `0/1` | consumed on apply [A] |
| `visible`, `structure` | `0/1` | `visibleTranquility`, `isStructureSkin` (an optional key on the client's rows; absent is 0) |
| `hulls` | `str` | the hull type ids it fits, comma separated |

Invented example rows:

```
990801	990821	100	-1	0	0.1	Fixture SKIN A
100	1	0	990898
```

### `mining/geodes.tsv` (L6A GEODES)

- **Writer:** `tools/content_geodes.py`, the `geodes` section of `content.py`. Inputs: the client's `crates`, `types` and the localisation, and the authored `content/econ/geode-picks.tsv` (`name`, `tier`, `why`; ships empty, an exact en-us name per row).
- **Readers:** `eve-mod-econ` `geode::TABLE`. Optional: with none, or with the header only, no geode exists and mining is unchanged. `mining/geode-candidates.tsv` (`type_id`, `name`, `group_id`, `volume`) lists every crate the client opens with random loot and no fixed contents, to pick from.

```
#type_id	tier
```

| column | type | meaning |
|---|---|---|
| `type_id` | `i64` | a crate item type |
| `tier` | `1..3` | a higher tier holds more prizes and bigger ones (`tunables::PRIZES_BY_TIER`) |

The weights of what a geode holds are the authored `content/econ/geode-loot.tsv` (`role`, `weight`, `why`), by role and never by type id.

### `deploy/units.tsv` (hand-data, #109)

- **Writer:** `tools/content_deploy.py`, the `deploy` section of `content.py`. Input: the client's `spacecomponentsbytype`, `types`. One row per deployable group (Mobile Depot 1246, Mobile Tractor Unit 1250), from the group's **base type** (**[A]** the lowest type id of the group that has `cargoBay`, `activate`, `decay` and `deploy`; falsifier: a later type of the group whose figures the client shows differently).
- **Readers:** `eve-server` `mtu::TABLE` (`mtu::Specs`). Optional: without the file no deployable can be launched.

```
#group_id	activate_s	decay_s	access_range	allow_user_add	deploy_at	own_group_gap	tractor_max	tractor_min	cycle_s
```

| column | type | meaning |
|---|---|---|
| `group_id` | `i64` | the item group |
| `activate_s`, `decay_s` | seconds | `activate.durationSeconds`, `decay.durationSeconds` |
| `access_range` | metres | `cargoBay.accessRange` |
| `allow_user_add` | `0/1` | `cargoBay.allowUserAdd` |
| `deploy_at` | metres | `deploy.deployAtRange` |
| `own_group_gap` | metres | `deploy.minDistanceFromOwnGroup` |
| `tractor_max`, `tractor_min`, `cycle_s` | metres, metres, seconds | `autoTractorBeam` (with an `autoLooter`); all three empty for a unit with no beam |

### `deploy/injectors.tsv` (hand-data, #109)

- **Writer:** `tools/content_deploy.py`. Input: the client's `types`, `groups` and the localisation, for type ids 40519, 40520 and 45635 (the Skill Extractor, the Large and the Small Skill Injector, constants of the client's code).
- **Readers:** `eve-server` `skill_trading::TABLE` (`Rows`), which `item::find_in` falls through to. Optional: without the file those three types are unknown to the item store.

```
#type_id	group_id	category_id	singleton	volume	name
```

Columns as `store/types.tsv`.

### `salvage/wrecks.tsv`

- **Writer:** X-SALV (`content_salvage.py`), from `types/market.tsv` at extract time.
- **Readers:** X-SALV (`salvage.rs`: which size and race of yield a wreck holds).
- **Key:** `wreck_type_id`.
- **Stable client ids and delta keys (ruling 4):** none.
- **The classification is [A]**: the client ships no table of what a wreck holds for a salvager. The wreck types and the hull groups are [V]; which group is which size is the extractor's `SIZE_OF_GROUP`.

```
#wreck_type_id	size	race_id
```

| column | type | source and meaning |
|---|---|---|
| `wreck_type_id` | `i64` | a `wreckTypeID` some ship type in `types/market.tsv` carries |
| `size` | `i64` | 1 (frigate and destroyer lines), 2 (cruisers, battlecruisers, industrials) or 3 (battleships and capitals); the largest of the hulls that share the wreck type; hulls in no listed group count as 2 |
| `race_id` | `i64` | the hulls' `raceID`, else their `factionID`, else 0; 0 also when the hulls of the wreck type disagree |

Invented example rows:

```
990331	1	990341
990333	3	0
```

### `salvage/yields.tsv`

- **Writer:** X-SALV (`content_salvage.py`), from `types/market.tsv` and `econ/prices.tsv` at extract time.
- **Readers:** X-SALV (`salvage.rs`: what a successful salvage cycle yields).
- **Key:** `(size, race_id, type_id)`.
- **Stable client ids and delta keys (ruling 4):** none.
- **The whole table is [A]**: the members are the published salvage (group 754 [V]); the bands, weights, quantities and the race favouring are the extractor's `BAND_WEIGHTS`, `BAND_QTY`, `FAVOUR_MOD` and `FAVOUR_BOOST`.

```
#size	race_id	type_id	weight	qty_min	qty_max	value
```

| column | type | source and meaning |
|---|---|---|
| `size` | `i64` | 1, 2 or 3, as in `salvage/wrecks.tsv` |
| `race_id` | `i64` | a race or faction named in `salvage/wrecks.tsv`, or 0: the rows for any race and the fallback for a race with none of its own |
| `type_id` | `i64` | a salvage type |
| `weight` | `i64` | the draw's weight: the size's weight for the type's value band (cheapest, middle and dearest third), times the boost on the race's favourite types |
| `qty_min` | `i64` | one draw's least quantity |
| `qty_max` | `i64` | the most |
| `value` | `f64` | `P_ref` from `econ/prices.tsv`, else `basePrice`, else 0 |

Invented example rows:

```
1	0	990301	70	1	4	1.0
3	990341	990306	195	1	2	100.0
```

- Rows are sorted by `(size, race_id, type_id)`.
- `--check` also reads `industry/blueprints.tsv`: every published rig blueprint takes salvage, and every salvage type a rig recipe takes appears here, so what is salvaged can all be built into rigs.

### `fighters/abilities.tsv`

- **Writer:** FT-A (`content_fighters.py`), from `fighterabilities.static`.
- **Readers:** FT-A (`fighters_data.rs`, `Fighters::ability`, `effect_of`).
- **Key:** `ability_id`.
- **Stable client ids and delta keys (ruling 4):** none.
- A bundle made before format 1.3 has no such file and loads; `Fighters` is then empty.

```
#ability_id	target_mode	low_sec_banned	high_sec_banned	effect_id	icon_id	turret_graphic_id	name
```

| column | type | source and meaning |
|---|---|---|
| `ability_id` | `i64` | the client's fighter ability id (36 rows in build 3619879) |
| `target_mode` | `str` | `untargeted`, `itemTargeted` or `pointTargeted` |
| `low_sec_banned` | `bool` | `disallowInLowSec` |
| `high_sec_banned` | `bool` | `disallowInHighSec` |
| `effect_id` | `i64` | the dogma effect that carries the ability's numbers. **Not in the static table**: the client's `dogmaEffectByAbilityID` [V `fighters/abilityAttributes.py`], copied as the extractor's `ABILITY_EFFECT`. Three abilities (1, 3, 6) are used by no shipped type, so their effects need not exist in `types/typedata.tsv`; `--check` requires every used one to |
| `icon_id` | `i64` | `iconID` |
| `turret_graphic_id` | `i64?` | `turretGraphicID`; empty when absent |
| `name` | `str` | the localized `displayNameID` text |

Invented example rows:

```
990601	itemTargeted	0	0	990621	990631	990641	Example Attack
990602	untargeted	0	1	990622	990632		Example Drive
```

### `fighters/slots.tsv`

- **Writer:** FT-A (`content_fighters.py`), from `fighterabilitiesbytype.static`.
- **Readers:** FT-A (`fighters_data.rs`, `Fighters::slots_of`, `slot`).
- **Key:** `(type_id, slot)`.
- **Stable client ids and delta keys (ruling 4):** none.
- A fighter's class (light, support, heavy), squadron size, orbit range, role and refuel time are **attributes of its type row** (2212-2214 and 2740-2742, 2215, 2223, 2270, 2426 in `types/typedata.tsv`, category 87), not columns here. A carrier hull's tubes, per-class slots and bay are its attributes 2216-2219, 2737-2739 and 2055.

```
#type_id	slot	ability_id	cooldown_s	charges	rearm_s
```

| column | type | source and meaning |
|---|---|---|
| `type_id` | `i64` | a category-87 type (94 published in build 3619879; the five placeholders have no row) |
| `slot` | `i64` | 0, 1 or 2 (`abilitySlot0..2`); a slot the type does not populate has no row |
| `ability_id` | `i64` | a row of `fighters/abilities.tsv` |
| `cooldown_s` | `f64?` | `cooldownSeconds`; empty when absent (only mobility abilities have one) |
| `charges` | `i64?` | `charges.chargeCount`; empty when absent |
| `rearm_s` | `f64?` | `charges.rearmTimeSeconds`; empty when absent |

Invented example rows:

```
990611	0	990601			
990611	1	990602	60.0		
990612	2	990603		3	20.0
```

- Rows are sorted by `(type_id, slot)`.
- `--check` also reads `types/typedata.tsv`: every `type_id` is a published category-87 type, every published fighter has a row, and every published fighter carries exactly one class flag and a squadron size.

### `dbuffs/collections.tsv`

- **Writer:** K-BOOST (`content_dbuffs.py`), from `dbuffcollections.static` (an fsdlite sqlite cache, 277 rows in build 3619879).
- **Readers:** K-BOOST (`dbuff_data.rs`, `Dbuffs::collection`; `dbuff.rs` applies them).
- **Key:** `collection_id`.
- **Stable client ids and delta keys (ruling 4):** none. The id is the client's `dbuffCollectionID` (the constants of `dbuff/const.py`) and is what a burst module's `warfareBuffNID` attribute (2468, 2470, 2472, 2536) holds.
- A bundle made before this file loads; `Dbuffs` is then empty and no command burst does anything.

```
#collection_id	operation	aggregate	show	name	description
```

| column | type | source and meaning |
|---|---|---|
| `collection_id` | `i64` | the client's dbuff collection id |
| `operation` | `str` | `operationName`: one of the nine of `dbuff/storage.py` `OPERATION_IDS_BY_NAME` (`PreAssignment`, `PreMul`, `PreDiv`, `ModAdd`, `ModSub`, `PostMul`, `PostDiv`, `PostPercent`, `PostAssignment`) |
| `aggregate` | `str` | `aggregateMode`: `Minimum` or `Maximum` (`dbuff/common/aggregators.py`): how two live buffs of the collection on one ship combine |
| `show` | `str` | `showOutputValueInUI`: `ShowNormal`, `ShowInverted` (the buff bar negates the value) or `Hide` (`dbuff/client/uiFormatters.py`) |
| `name` | `str` | the localized `displayNameID` text; empty when the collection has none |
| `description` | `str` | `developerDescription`, as the client ships it |

Invented example rows:

```
990701	PostPercent	Minimum	ShowInverted	Example Resist Burst	Example
990702	ModAdd	Maximum	Hide		Example flag
```

- Rows are sorted by `collection_id`.
- `--check` also reads `types/typedata.tsv`: the enums are the known ones, every modifier names a collection of the first file and an attribute (and, for a skill row, a skill type) typedata holds, and every collection a warfare charge names (category 8 types carrying effect 6737, attributes 2468, 2470, 2472, 2536) is a row with at least one modifier.

### `dbuffs/modifiers.tsv`

- **Writer:** K-BOOST (`content_dbuffs.py`), from `dbuffcollections.static`.
- **Readers:** K-BOOST (`dbuff_data.rs`, `Collection::{item, location, group, skill}`).
- **Key:** `(collection_id, kind, attribute_id, group_id, skill_id)`.
- **Stable client ids and delta keys (ruling 4):** none.

```
#collection_id	kind	attribute_id	group_id	skill_id
```

| column | type | source and meaning |
|---|---|---|
| `collection_id` | `i64` | a row of `dbuffs/collections.tsv` |
| `kind` | `str` | `item` (`itemModifiers`: the ship itself), `location` (`locationModifiers`: everything fitted), `group` (`locationGroupModifiers`) or `skill` (`locationRequiredSkillModifiers`); `dbuff/common/registry.py` `ModifierApplier` |
| `attribute_id` | `i64` | `dogmaAttributeID`: the attribute the buff's output value moves |
| `group_id` | `i64?` | `groupID` of a `group` row; empty otherwise |
| `skill_id` | `i64?` | `skillID` of a `skill` row; empty otherwise |

Invented example rows:

```
990701	item	990711		
990703	group	990712	990721	
990704	skill	990711		990731
```

- Rows are sorted by `(collection_id, kind order item, location, group, skill, attribute, group, skill)`.

---

## 3. The three kind-tagged files (no header line)

Phase 1 keeps these byte-identical to today's extractors (plan §1.1), so their layouts are frozen as they stand. The field names are the tools' own docstring names. Each line is the record kind, then the fields, TAB-separated. Lines starting with `#` are comments.

### `types/typedata.tsv`

- **Writer:** CP1 (imports `typedata.py`); T3-A adds category 32 (*Subsystem*, 48 types: `subSystemSlot` 1366, `fitsToShipType` 1380, slot modifiers 1374-1376, hardpoints 1368/1369) to `typedata.CATEGORIES`, no layout change; `typedata.py --check` verifies them (`problems`); CP12 adds categories 17 (commodity) and 25 (asteroid) to `typedata.CATEGORIES` and changes no layout. Both install as stackable items, grantable when published (`typedata.rs` `asteroid_and_commodity_rows_install_as_stackable_grantable_items`).
- **Readers:** `typedata.rs` (installed by CP3; `Content` from CP58a); CP20 reads the category-11 rows' `attrs` and `effectIDs` for derived NPC fields.
- **Key:** `(kind, id)`. **Delta keys:** `type_id` is stable, but no delta keys on this file.

| kind | fields after the kind |
|---|---|
| `type` | `typeID groupID categoryID published portionSize volume capacity mass radius defaultEffectID effectIDs attrs name` |
| `attribute` | `attributeID defaultValue name` |
| `effect` | `effectID category isOffensive durationAttributeID dischargeAttributeID rangeAttributeID falloffAttributeID trackingSpeedAttributeID name guid` |
| `attrmeta` | `attributeID stackable highIsGood maxAttributeID minAttributeID dataType` |
| `modifier` | `effectID domain func modifiedAttributeID modifyingAttributeID operation skillTypeID groupID` |

Invented example line:

```
type	990001	99001	6	1	1	2500.0	130.0	1000000.0	35.0	0	11,12	9=350.0;37=300.0	Example Hull
```

- **Unchanged from `tools/typedata.py`** (docstring `:25-33`). There is no header line: each line starts with its record kind, and lines starting with `#` are comments (the file opens with a two-line `#` banner). `effectIDs` is a comma list, `attrs` is `id=value;…`, and `name` is last.

### `types/skills.tsv`

- **Writer:** CP1 (imports `skills.py`).
- **Readers:** `skill.rs` (installed by CP3; `Content` from CP59a).
- **Key:** `typeID`. **Delta keys:** none.

| kind | fields after the kind |
|---|---|
| `skill` | `typeID groupID rank primary secondary volume required name` |

Invented example line:

```
skill	990010	99010	3	165	166	0.01	990011=1	Example Skill
```

- **Unchanged from `tools/skills.py`** (docstring `:39`). No header line. `required` is `typeID=level;…`.
- `rank`, `primary` and `secondary` are integers (`skill.rs` parses them as `i64`, CP59a); `volume` is a float.

### `planets/pi-statics.tsv`

- **Writer:** CP1 (imports `pi_statics.py`).
- **Readers:** `planet.rs` (installed by CP3; `Content` from CP60).
- **Key:** `(kind, id)`. **Delta keys:** none.

| kind | fields after the kind |
|---|---|
| `pintype` | `typeID groupID capacity basePrice metaLevel attrs` |
| `schematic` | `schematicID cycleTimeSeconds pinTypeIDs inputs outputs` |
| `resource` | `planetTypeID distribution` |
| `itemtype` | `typeID groupID categoryID volume capacity name` |
| `taxmult` | `typeID importTaxMultiplier` |
| `customsoffice` | `typeID capacity radius taxRate` |

Invented example line:

```
schematic	9901	3600	990040,990041	990050=3000	990051=20
```

- **Unchanged from `tools/pi_statics.py`** (docstring `:35-40`). No header line.

---

## 4. Authored files: `content/` (plan §2.5)

Rules: §1.1's authored form. Every row that carries an `[A]` value states its falsifier in `why`, the last column (plan §2.4). Tests may parse a shipped file but never assert a value in it (plan §1.4). `content/README.md` (CP8) is prose, not a table, and has no header.

| file | owner | read by |
|---|---|---|
| `content/population.tsv` | CP21, CP54a, CP53b | CP21, CP8 |
| `content/npc/region-pirates.tsv` | CP11b | CP11b |
| `content/facwar/warzones.tsv` | CP18 | CP18, CP53a, CP53b |
| `content/agents/rewards.tsv` | CP26b | CP26b |
| `content/econ/price-overrides.tsv` | CP24 | CP24 |
| `content/econ/meta-mult.tsv` | CP24 | CP24 |
| `content/agents/lp-overrides.tsv` | CP37a | CP37a |
| `content/econ/hubs.tsv` | CP35 | CP35 |
| `content/econ/starter-fit.tsv` | E7 | E7 |
| `content/wormholes/statics.tsv` | CP54a | CP54a |
| `content/bots/hostile.tsv` | CP56 | CP56 |
| `content/store/offers.tsv` | L1 | `content_store.py` |
| `content/store/categories.tsv` | L1 | L1 (`nes`), `content_store.py` |
| `content/bots/difficulty.tsv` | DOC-8 | DOC-8 |
| `content/econ/geode-picks.tsv` | L6A | `content_geodes.py` |
| `content/econ/geode-loot.tsv` | L6A | L6A (`eve-mod-econ` `geode::TABLE`) |
| `content/pve/filament-drops.tsv` | #61 FIL-DROP | `filament_drops::TABLE` (the roll is called from NPC and site loot by #121) |

### `content/population.tsv`

- **Owner (writer):** CP21 (W13); rows added by CP54a (W16) and CP53b (W17).
- **Readers:** CP21 (`population::AUTHORED`, through CP8's `authored.rs`).
- **Key:** `key`.
- **Stable ids and delta keys (ruling 4):** **`key`** is the stable, author-chosen slot key of the `site` delta (`system_id archetype key k`) and the `wh` delta (`system_id key k`). **Never rename a key**: a rename drops that row's live deltas. Retire a row by deleting it, and add a new key.

```
key	band	archetype	entry	count	difficulties	respawn_s	ttl_s	why
```

| column | type | meaning |
|---|---|---|
| `key` | `str` | unique, `[a-z0-9_]+` |
| `band` | `enum(high\|low\|null)` | security band (C3 §3); CP54a adds wormhole bands |
| `archetype` | `list<i64>` | archetype ids, for example `44,45`; C3's `44\|45` is written with commas |
| `entry` | `enum(anomaly\|signature)` | entry type 28356 or 19728 |
| `count` | `f64` | live at once; a fraction below 1 is a per-system probability [A] |
| `difficulties` | `list<i64>\|*` | a list, or `*` for C3 §4.1's ore-band rule |
| `respawn_s` | `i64` | seconds after completion [A] |
| `ttl_s` | `i64` | seconds an unvisited site lives before rollover expiry [A] |
| `why` | `text` | the falsifier |

Invented example row:

```
example_combat_hi	high	98	anomaly	2	1,2	1200	86400	[A] invented example row
```

### `content/npc/region-pirates.tsv`

- **Owner (writer):** CP11b.
- **Readers:** CP11b's extractor at extract time (it is applied into `out/content/npc/regions.tsv`); the server never reads it.
- **Key:** `(region_id, faction_id)`.
- **Stable ids and delta keys (ruling 4):** none.

```
region_id	faction_id	weight	why
```

| column | type | meaning |
|---|---|---|
| `region_id` | `i64` | region |
| `faction_id` | `i64` | pirate faction |
| `weight` | `f64` | replaces the derived weight; 0 removes the pair |
| `why` | `text` | falsifier |

Invented example row:

```
19000001	599001	0.5	[A] invented example row
```

### `content/facwar/warzones.tsv`

- **Owner (writer):** CP18.
- **Readers:** CP18 (`facwar::WARZONES`); CP53a and CP53b.
- **Key:** `warzone_id`.
- **Stable ids and delta keys (ruling 4):** `fwvp` is keyed by `system_id`, not by warzone.

```
warzone_id	faction_a	faction_b	hq_a	hq_b	why
```

| column | type | meaning |
|---|---|---|
| `warzone_id` | `i64` | the client's warzone id (C2 §5) |
| `faction_a` | `i64` | one side |
| `faction_b` | `i64` | the other side |
| `hq_a` | `i64` | side A's HQ system |
| `hq_b` | `i64` | side B's HQ system |
| `why` | `text` | falsifier |

Invented example row:

```
99	599001	599002	39000001	39000002	[A] invented example row
```

### `content/pve/filament-drops.tsv`

- **Owner (writer):** #61 FIL-DROP.
- **Readers:** `filament_drops::TABLE` (`filament_drops::Table`). A row `off` in `source` turns drops off (the world switch).
- **Key:** none: rows roll in file order, the first hit wins.
- **Stable ids and delta keys (ruling 4):** none.

```
source	name	kind	band	chance_ppm	weather	why
```

| column | type | meaning |
|---|---|---|
| `source` | `enum(npc\|site\|off\|on)` | a killed NPC, a hacked relic or data can, or the world switch |
| `name` | `str` | the NPC class (as `npc/types.tsv` `class`) or the site archetype (`relic`, `data`); `*` any |
| `kind` | `enum(abyss\|jump)` | an abyssal key (tier by band) or a random-jump filament |
| `band` | `str` | letters `h` `l` `n` `w`, or `*` |
| `chance_ppm` | `i64` | chance per kill or can, parts per million [A] |
| `weather` | `str` | `any`, or a word in the abyssal key's name |
| `why` | `str` | the reason and the [A]'s falsifier |

### `content/agents/rewards.tsv`

- **Owner (writer):** CP26b.
- **Readers:** CP26b (`mission_rewards::TABLE`).
- **Key:** `(scale, level)`.
- **Stable ids and delta keys (ruling 4):** none.

```
scale	level	k_isk	k_bonus	k_lp	bonus_minutes	agent_gain	corp_gain	why
```

| column | type | meaning |
|---|---|---|
| `scale` | `enum(security\|distribution)` | C4 §4 scale |
| `level` | `i64` | 1-5 |
| `k_isk` | `f64` | × the level's ISK base (C5 §5.4) [A] |
| `k_bonus` | `f64` | bonus as a fraction of the reward [A] |
| `k_lp` | `f64` | LP as a fraction of `reward / R` [A] |
| `bonus_minutes` | `i64` | bonus window [A] |
| `agent_gain` | `f64` | agent standing gain [A] |
| `corp_gain` | `f64` | corp standing gain [A] |
| `why` | `text` | falsifier |

Invented example row:

```
security	1	1.0	0.5	0.5	60	0.01	0.005	[A] invented example row
```

- Ruling 9: our own multipliers, with no EvEJS row.

### `content/econ/price-overrides.tsv`

- **Owner (writer):** CP24.
- **Readers:** CP24's extractor (applied before the floor, C5 §3.5); the server sees the result in `econ/prices.tsv` as `source = override`.
- **Key:** `type_id`.
- **Stable ids and delta keys (ruling 4):** none.

```
type_id	price	why
```

| column | type | meaning |
|---|---|---|
| `type_id` | `i64` | type |
| `price` | `f64` | `P_ref` before the floor [A] |
| `why` | `text` | falsifier |

Invented example row:

```
990300	18000.0	[A] invented example row
```

### `content/econ/meta-mult.tsv`

- **Owner (writer):** CP24.
- **Readers:** CP24's extractor (C5 §3.2 step 4).
- **Key:** `meta_group_id`.
- **Stable ids and delta keys (ruling 4):** none.

```
meta_group_id	mult	why
```

| column | type | meaning |
|---|---|---|
| `meta_group_id` | `i64` | `metaGroupID` |
| `mult` | `f64` | × the parent's `P_ref` [A] |
| `why` | `text` | falsifier |

Invented example row:

```
99	2.0	[A] invented example row
```

### `content/agents/lp-overrides.tsv`

- **Owner (writer):** CP37a.
- **Readers:** CP37a's extractor (applied into `agents/lp-offers.tsv`).
- **Key:** `(table_id, type_id)`.
- **Stable ids and delta keys (ruling 4):** none.

```
table_id	type_id	action	lp_cost	isk_cost	why
```

| column | type | meaning |
|---|---|---|
| `table_id` | `i64` | LP offer table |
| `type_id` | `i64` | product |
| `action` | `enum(add\|remove\|price)` | add an offer, remove a derived one, or re-price it |
| `lp_cost` | `i64?` | for `add` and `price` |
| `isk_cost` | `i64?` | for `add` and `price` |
| `why` | `text` | falsifier |

Invented example row:

```
9901	990001	price	3000	3000000	[A] invented example row
```

### `content/econ/hubs.tsv`

- **Owner (writer):** CP35.
- **Readers:** CP35 (`econ_exchange`, Tier B; under O5-d, E5, loot-only and tag floor bids, no ask; E5b: only while `econ_exchange::LOOT_BIDS`, an [A] stopgap with no TQ basis).
- **Key:** `station_id`.
- **Stable ids and delta keys (ruling 4):** none.

```
station_id	why
```

| column | type | meaning |
|---|---|---|
| `station_id` | `i64` | a Tier B hub station |
| `why` | `text` | why it is a hub |

Invented example row:

```
69000001	[A] invented example row
```

### `content/econ/starter-fit.tsv`

- **Owner (writer):** E7.
- **Readers:** E7 (`starter_ship::TABLE`, the rookie ship's fit).
- **Key:** none; rows are kept in file order (one row per module, fitted in that order, each into the lowest free slot of its own rack).
- **Stable ids and delta keys (ruling 4):** none.

```
ship_type_id	type_id	why
```

| column | type | meaning |
|---|---|---|
| `ship_type_id` | `i64` | the corvette (`agents/races.tsv` `ship_type_id`) |
| `type_id` | `i64` | one module fitted into it |
| `why` | `text` | the falsifier |

Invented example row:

```
990870001	990000002	[A] invented example row
```

### `content/wormholes/statics.tsv`

- **Owner (writer):** CP54a.
- **Readers:** CP54a (`wormhole::TABLE`).
- **Key:** `(class, static_type_id)`.
- **Stable ids and delta keys (ruling 4):** the `wh` delta is keyed through `population.tsv`'s `key`, not by this file.

```
class	static_type_id	dest_class	why
```

| column | type | meaning |
|---|---|---|
| `class` | `i64` | the origin system's `wormhole_class_id` |
| `static_type_id` | `i64` | the wormhole type of the static |
| `dest_class` | `i64` | the destination class |
| `why` | `text` | falsifier (R-WH) |

Invented example row:

```
1	990400	7	[A] invented example row
```

### `content/bots/hostile.tsv`

- **Owner (writer):** CP56.
- **Readers:** CP56 (`eve-bot/src/hostile.rs`).
- **Key:** `(ship_class, min_sec, max_sec)`.
- **Stable ids and delta keys (ruling 4):** none.

```
ship_class	min_sec	max_sec	engage	why
```

| column | type | meaning |
|---|---|---|
| `ship_class` | `str` | the target's class, one of `capsule`, `small`, `medium`, `large`, `capital` (the hull's `rigSize`, 1547) or `*` for any hull |
| `min_sec` | `f64` | lowest system security where the rule applies |
| `max_sec` | `f64` | highest |
| `engage` | `bool` | engage or not |
| `why` | `text` | falsifier |

Invented example row:

```
small	-1.0	0.0	1	[A] invented example row
```

### `content/bots/difficulty.tsv`

- **Owner (writer):** DOC-8.
- **Readers:** DOC-8 (`eve-bot/src/difficulty.rs`, `eve-bots --difficulty`).
- **Key:** `(level, constant)`; a later row wins.
- **Stable ids and delta keys (ruling 4):** none.

```
level	constant	value	why
```

| column | type | meaning |
|---|---|---|
| `level` | `u8` | 0 peaceful, 1 easy, 2 normal, 3 hard, 4 brutal |
| `constant` | `str` | a name from `difficulty::KNOBS` (`OBS_NOISE`, `COMMIT_STEP`, `GRUDGE_RATE`, `TRADE_MARGIN`...); an unknown name skips the row |
| `value` | `f64` | the value at that level (`inf` allowed); whole-number constants are rounded; `FROZEN` is `0` or `1` |
| `why` | `text` | `[A]` and its falsifier |

The shipped file lists every constant at every level, generated from the built-in columns
(`cargo test -p eve-bot dump_the_table -- --ignored --nocapture`); a missing row keeps the built-in value.

Invented example row:

```
3	OBS_NOISE	0.07	[A] invented example row
```

---

## 5. `out/live/status.tsv` (an operator file, not a bundle file)

- **Writer:** CP15 (`live_status.rs`, with `--status FILE`).
- **Readers:** the operator (`watch column -t`); CP15's test pins this header.
- **Key:** `system`. **Delta keys:** none (a disposable file, rewritten every 10 s).

It uses the bundle form (§1.1): line 1 is the `#` header.

```
#system	tier	humans	bots	npcs	sites_live	sites_cleared	rocks	rocks_depleted_pct	last_touch_utc	reasons
```

| column | type | meaning |
|---|---|---|
| `system` | `i64` | a warm or hot system |
| `tier` | `enum(warm\|hot)` | the liveness tier |
| `humans` | `i64` | humans present |
| `bots` | `i64` | bots present |
| `npcs` | `i64` | live NPC balls |
| `sites_live` | `i64` | live sites |
| `sites_cleared` | `i64` | cleared sites awaiting respawn |
| `rocks` | `i64` | materialised rocks |
| `rocks_depleted_pct` | `f64` | percent of the static rock count gone |
| `last_touch_utc` | `str` | ISO 8601 UTC |
| `reasons` | `list<str>` | why it is warm or hot, for example `docked,jump` |

Invented example row:

```
39000001	hot	1	2	4	3	1	120	12.5	2026-01-01T11:00:00Z	inspace
```

- Not in plan §1.2 or §2.5. It is frozen here because CP15's row says "the status TSV header equals `schema.md`'s". C7 §7.2's `name?` column is dropped, as C7 itself advises: the server writes ids only.

---

## 6. The delta keys, file by file (ruling 4)

Plan §4.2's delta kinds, and the file each key's ids come from. **Nothing
below is a row position.** Every derived number (rock `ordinal`,
`sys_rank`, `station_index`, site and instance ids, `offer_id`) is
recomputed on load and never persisted.

| delta | key fields | ids from | stored value checked on load |
|---|---|---|---|
| `rock` | `belt_item_id n` | `belts/rocks.tsv` `belt_id`; `n` is the in-belt expansion index in the file's own order | `type_id`, position |
| `site` | `system_id archetype key k` | `universe/systems.tsv`, `dungeons/archetypes.tsv`, `content/population.tsv` `key` | `dungeon_id` (`dungeons/dungeons.tsv`) |
| `beltwait` | `belt_item_id` | `belts/rocks.tsv` `belt_id`, `universe/celestials.tsv` belt `id` | — |
| `mission` | `char agent` | `agents/agents.tsv` `agent_id` | `mission_id` (`agents/missions.tsv`) |
| `npcorder`, `npcmult` | `station_id type_id side` | `stations/stations.tsv`, `types/market.tsv` | — |
| `mktidx` | `type_id` | `econ/prices.tsv` | — |
| `dbs` | `system_id` | `universe/systems.tsv` | — |
| `lp` | `char corp` | `npc/corporations.tsv` `corp_id` | — |
| `arc` | `char arc_id` | `agents/epic-arcs.tsv` | — |
| `fwvp` | `system_id` | `universe/systems.tsv` | — (value `score occupier taken_at credited`, plan §4.2; its `occupier` is a `npc/factions.tsv` id) |
| `wh` | `system_id key k` | `universe/systems.tsv`, `content/population.tsv` `key` | — (value `dest_system expires_at mass_left generation`, plan §4.2) |

---

## 7. What the checks found that a later packet must know

1. **`dungeons.entities` are not types** (`dungeons/spawns.tsv` notes). So
   CP26b's reward base cannot join spawns to bounties directly. It needs
   CP22's squad fill, so CP22 should join CP26b's "after" column.
2. **`objectiveTargetGroup` is empty in this build** on every object and
   entity. CP28a's slim extra of that name has no data source, so it is
   either constant or omitted, as CP28a decides from the client's reader.
3. **Missions carry two fixed-LP values** (Alpha and Omega, differing on
   all 78 rows that set them). `fixed_lp` is the Omega value [A].
4. **`disallowScanning` and `dscanRange` are set on one system each**, not
   per system as C1 §2.1 implied. Most `wormholeClassID`s are on
   `systems.static`.
5. **`schools` and `schoolmap` are cFSD** (`fsd.rows`), not `.static`
   resources. `regions`, `constellations` and `systems` are `.static`
   (`fsd.Client.load`).
6. **The manifest gains `input` records**, so that editing an authored
   override re-extracts the bundle.

### `facwar/systems.tsv` (FW-3)

- **Writer:** `tools/content_facwar.py`, the `facwar` section of `content.py`. Input: the client's `warzonespositiondata` (`{warzoneID: {systems: {ssid: row}}}`).
- **Readers:** `facwar.rs` (`Table::listed_systems`, read through the `facwar` table's `boxed_load`). Optional: with no file `facwar::warzone_systems` falls back to its [A] rule (low-sec systems of a militia faction plus the HQs).
- **Key:** `system_id`; sorted ascending.

```
#system_id	warzone_id
```

| column | type | source and meaning |
|---|---|---|
| `system_id` | `i64` | a system the client draws as territory: its row is visible and has a `territorySprite` (`mapData.GetWarzoneSystems` [V]); 163 on build 3662168 [V-data] |
| `warzone_id` | `i64` | the key of the `warzonespositiondata` row it sits under; must be a `warzones.tsv` id |

A listed system whose own `faction_id` is not a militia of its warzone is logged and skipped by the server (it has no seed occupier). The server derives the seed occupier from the system, not from this file.

### `facwar/max_lp.tsv` (#133, FW-KILLS)

- **Writer:** `tools/content_facwar.py`, the `facwar` section of `content.py`. Input: the bytecode of `out/pyc/eve/common/lib/appConst.pyc` (`facwarMaxLPPayout`, a dict `group constant -> LP`) with the group ids of `out/pyc/inventorycommon/const.pyc`. With no `out/pyc` the file is not written. **Adding it needs a full bundle regeneration** (the manifest gains a file).
- **Readers:** `facwar.rs` (`Table::max_lp_of`, read through the `facwar` table's `boxed_load`), `fw_lp::max_kill_lp`. Optional: with no file, or a hull group with no row, a kill is capped at `fw_lp::MAX_LP_UNKNOWN_GROUP` (`appConst.facwarMaxLPUnknownGroup`, a Rust const). `facwarMaxLPPayoutFactionOverwrites` is not applied [A].
- **Key:** `group_id`; sorted ascending. 45 rows on build 3662168 [V-data].

```
#group_id	max_lp
```

| column | type | source and meaning |
|---|---|---|
| `group_id` | `i64` | the victim hull's inventory group (`inventorycommon/const.group*`) |
| `max_lp` | `i64` | the most one kill of that group pays, in LP, before the line multiplier |
