# C1: the corpus-to-feature map

Lane C1 of `docs/content/README.md` §3, a read-only research lane, 2026-10-25.
The full per-file map is in **`findings/content-C1-map.tsv`**: 203 rows, one
per `res:/staticdata` file, with 12 columns:

| column | meaning |
|---|---|
| `resource`, `format`, `rows` | from `findings/corpus-inventory.tsv` |
| `layout` | the cFSD layout state: `pyd stride=N delta=M` (from `findings/pyd-schemas.tsv`), `scalar-row` (no getattro, so no fields to name), or `self-describing` (classic, sqlite or pickle) |
| `subsystem` | the server subsystem the file feeds |
| `consumed_today` | `runtime`: an extractor's output is loaded by a server flag. `partial`: an extractor reads it, but the field the server needs is dropped. `baked`: rows or ids are hand-copied or generated into Rust. `ref`: cited in comments or tests only. `none`: not used. |
| `consumer` | the file(s) that do the consuming |
| `value` | P0 = the co-op/PvP loop needs it now; P1 = it makes the universe feel inhabited (sites, rats, agents, economy); P2 = Agency, ESS, FW, arcs, loot flavour; P3 = cosmetic, UI-only, or client-only |
| `owner_extractor` | the single section of `tools/content.py` that owns it, `none (client-only)`, or `tools/fsd.py (reader)` |
| `out_content_file` | the file(s) the owner writes under `out/content/`. `-` means the section owns the file but v1 writes nothing for it. |
| `mark`, `note` | the evidence mark, and the one fact that decides the row |

Marks follow §2 of the brief: **[V]** this build's bytecode or data,
**[V2]** EvEJS, **[I]** inferred (the inference is stated), **[A]** an
assumption we would ship, with what would falsify it.

---

## 1. Headline

1. **The server uses 13 of the 203 files at runtime, through 5 extractors and
   5 flags.** **[V]** `tools/celestials.py`, `asteroids.py`, `pi_statics.py`,
   `typedata.py` and `skills.py` feed `--celestials`, `--belt-rocks`,
   `--planets`, `--typedata` and `--skills` (`bin/eve-server.rs:118-122`).
   Another 17 files reach the server only as **baked** Rust rows, 11 only as
   citations, and **161 are not used at all.**
2. **Of those 161, 115 are P3 and need nothing from the server.** **[I]**
   They are presentation tables (graphics, sound, UI, paperdolls, skins,
   brackets) that the client loads from its own `res:`. The server's only
   duty is to send ids that close into them.
3. **The live-universe tables are almost all unread.** **[V]** `dungeons`,
   `archetypes`, `spacecomponentsbytype`, `missions`, `npccharacters`,
   `npccorporations`, `marketgroups`, `asteroidtypesbysolarsystemid` and the
   `ore`/`gas`/`ice` `typesbydungeonid` tables have **no extractor**. Today
   the server derives rats from category-11 `typedata` rows
   (`npc.rs:520 derive_all`), but only for the factions in
   `npc.rs:381 FACTIONS = [ANGEL_CARTEL]`. It knows one agent
   (`mission.rs:70`) and 23 blueprints (`industry/tables.rs`).
4. **What blocks content is a missing decoder, not missing layouts.** **[V]**
   `findings/pyd-schemas.tsv` has a layout for 139 of the 149 cFSD tables.
   The 10 without one have scalar rows, so there is nothing to lay out
   (`native-loaders.md` §1). But `tools/fsd.py` decodes only the 7 tables in
   `CFSD_COLUMNS` (`accountingentrytypes`, `categories`, `dungeons`,
   `graphiclocations`, `groups`, `missions`, `types`), and `tools/dogma.py`
   decodes only the 11 in `SCHEMAS`. **The generic `PydSchema` loader that
   `native-loaders.md` §7 proposed was never applied**: `grep -c PydSchema
   tools/*.py` returns 0 for every file. Every other P1 cFSD table is
   blocked on that one patch. The true layout gaps are in §4.
5. **The type extract lacks the economy columns, and the extract on disk is
   stale.** **[V]** Since `b8a6234` (W10 belt rats), `typedata.py:87`
   `CATEGORIES = (6, 7, 8, 11, 16, 18)` includes the 7,418 *Entity* types,
   but the `out/typedata.tsv` on this machine predates that commit: it has 0
   category-11 rows. That is exactly the staleness the manifest in §5.2
   catches. Two gaps remain. The `type` record still lacks `basePrice`,
   `marketGroupID`, `metaLevel`, `factionID`, `raceID` and `wreckTypeID`
   (the server derives wrecks per faction instead, `npc.rs` `wreck_of`).
   Category 25 (*Asteroid*, 503 types) is also absent. Without these the
   server cannot price, seed a market, or roll meta-level loot.
6. **Committed client data, flagged for the coordinator.** **[V]**
   `server/crates/eve-server/src/reprocess/tables.rs` (3,916 lines) and
   `industry/tables.rs` (1,564 lines) are tracked by git, and both say
   "Generated. Do not hand-edit... read straight out of the client
   installation". They hold subsets of `typematerials`, `typedogma`,
   `compressibletypes`, `types.basePrice` and `blueprints`. **[I]** That
   conflicts with the standing rule "no extracted client data in the
   repository" (`docs/archive/plan-overnight-2026-10-23.md:402`). No tool in
   `tools/` regenerates them. The bundle in §5 makes them fallbacks, the
   same pattern `--typedata` already uses for `modules::TYPES`. Whether to
   shrink them is the coordinator's call, not C1's.

Counts from the TSV:

| value | files | | consumed_today | files |
|---|---:|---|---|---:|
| P0 | 16 | | runtime | 13 |
| P1 | 32 | | partial | 1 (`systems.static`) |
| P2 | 40 | | baked | 17 |
| P3 | 115 | | ref | 11 |
| | | | none | 161 |

Ownership is 96 `none (client-only)`, 7 `tools/fsd.py (reader)` (the six
`.schema` sidecars plus `dialogs.static`), 5 `none`, and 95 split over nine
`content.py` sections: `types` 16, `agency` 25, `industry` 12, `dungeons`
11, `agents` 10, `universe` 9, `npc` 8, `belts` 2 and `planets` 2.

---

## 2. The map, by subsystem (P0 and P1 only; the TSV has all 203)

### 2.1 Universe: `content.py:universe`

| file | today | spec |
|---|---|---|
| `solarsystemcontent.static` | runtime: `celestials.py` (planet, moon, belt, station, gate, star) | **[V]** also carries `security`, `securityClass`, `hub/border/fringe/corridor/regional/international`, `disallowScanning`, `dscanRange`, `wormholeClassID` per system, and none of them is extracted. Extractor writes `universe/systems.tsv` (§5.3). |
| `systems.static` | partial: `celestials.py` reads it, and the server never sees security (`crimewatch.rs:674`: "the system's class from `systems.static`, which the server does not load") | **[V]** the only source of `regionID` and `constellationID` per system. Joined into `universe/systems.tsv`. |
| `regions.static`, `constellations.static` | none | `universe/regions.tsv` (`region_id nebula_id faction_id wormhole_class_id neighbours name`), `universe/constellations.tsv`: `id faction_id wormhole_class_id neighbours name`. **[I]** the faction-by-region rule for rat selection (C3/C6) keys on this. |
| `factionsowningsolarsystems.static` | none | 22 rows **[V]**; folded into `systems.tsv:owner_faction`. |
| `graphiclocations.fsdbinary` | baked: `docking.rs:299 STATIONS` | `universe/dock-locators.tsv`; the docking lane's hand table becomes the fallback. |
| `jumps.static` | none | **[I]** redundant: gate destinations already come from `solarsystemcontent` `stargates[].destination` (`celestials.py:37`). The extractor uses it as a cross-check only and writes nothing. |

### 2.2 Belts and resources: `content.py:belts`

| file | today | spec |
|---|---|---|
| `asteroidcountbytypeidbeltid.static` | runtime: `asteroids.py` → `--belt-rocks`, but only through `tools/bots.sh`, because **[V]** `up.sh` never passes `--belt-rocks` | `belts/rocks.tsv`, unchanged format `belt_id ordinal type_id count system growth unit_size max_radius`. |
| `asteroidtypesbysolarsystemid.pickle` | none | **[V]** 3,974 systems → a tuple of types. `belts/ore-by-system.tsv`: `system_id type_ids`. **[I]** the regrowth palette for depleted belts and for ore anomalies (C3). |

### 2.3 Types and dogma: `content.py:types`

| file | today | spec |
|---|---|---|
| `types`, `groups`, `categories`, `typedogma`, `dogmaattributes`, `dogmaeffects` | runtime: `typedata.py` | Phase 1 keeps `types/typedata.tsv` byte-identical (it already carries category 11, which `typedata.rs:600` keeps out of the player-item lookups **[V]**). Phase 2 adds category 25 to it, and adds `types/market.tsv`: `type_id market_group_id base_price meta_group_id meta_level faction_id race_id wreck_type_id portion_size published` for all 53,288 types (**[V]** 19,957 have a `marketGroupID`). |
| `requiredskillsfortypes` | runtime: `skills.py` | `types/skills.tsv`, the existing format. |
| `marketgroups` | none (the layout is in `dogma.py SCHEMAS`) | `types/marketgroups.tsv`: `id parent_id has_types icon_id name`. 2,114 rows **[V]**. |
| `typelist` | baked: `industry.rs` (raw read) | `types/typelists.tsv`: `list_id inc_types inc_groups inc_cats exc_types exc_groups exc_cats`. **[V]** `dungeons.connections.allowedShipsList` closes 162/162 into it (`dungeon-model.md` §2). Gate ship restrictions need it. |
| `typematerials`, `compressibletypes` | baked: `reprocess/tables.rs` (484 of 9,556, and 212) | `types/materials.tsv` `type_id material_type_id quantity`, `types/materials-random.tsv` `type_id material_type_id qmin qmax`, `types/compress.tsv` `type_id compressed_type_id`. Scrapmetal reprocessing needs all 9,556. |

### 2.4 Planets: `content.py:planets`

`schematics` and `globaltypes` go to `--planets` through `pi_statics.py`.
Phase 1 moves `planets/pi-statics.tsv` over unchanged. **[V]**

### 2.5 Dungeons and sites: `content.py:dungeons`

None of these is extracted today. `dungeon.rs` and `mission.rs` hold ids
only (`dungeon.rs:8-21`: "every number in it is `[A]`").

| file | spec |
|---|---|
| `dungeons.fsdbinary` | Five long-form files, every column from `pyd-schemas.tsv` **[V]**. `dungeons/dungeons.tsv`: `dungeon_id archetype_id faction_id difficulty entry_object_id entry_type_id objectives_id name_id content_tags`. `rooms.tsv`: `dungeon_id room_id short_name empty_reset_delay message_header_id message_text_id`. `objects.tsv`: `dungeon_id room_id object_id type_id x y z radius yaw pitch roll name_id` (260,242 rows). `gates.tsv`: `dungeon_id from_object_id allowed_ships_list disallowed_fitted_types_list allowed_ship_races`. `spawns.tsv`: `dungeon_id room_id object_id entity_id npc_grouping_id behavior_name group_behavior_name trigger_spawn additional_loot_id entity_min_respawn entity_max_respawn spawn_once aggression_range orbit_range` (30,610 rows). |
| `archetypes.fsdbinary` | `dungeons/archetypes.tsv`: `archetype_id location content_tags title_id`. 71 rows **[V]**. The site-class key C3 generates from. |
| `ore/gas/icetypesbydungeonid.static` | `dungeons/resources.tsv`: `dungeon_id kind type_ids`, 1,120 + 83 + 21 rows **[V]**. |
| `asteroidcountbytypeidroomiddungeonid.static` | `dungeons/room-rocks.tsv`: `dungeon_id room_id type_id count`, 3,054 cells. **[V]** These reproduce the `dungeons` room objects exactly, so the extractor asserts that equality as its self-test. |
| `agentsinspace.fsdbinary` | `dungeons/agents-in-space.tsv`: `key dungeon_id system_id spawn_point_id type_id`. 360 rows with full placement **[V]**. |
| `triggertypes.static`, `eventtypes.static` | Vocabulary only. The extractor validates `spawns.tsv` against it and writes nothing. |

### 2.6 NPCs: `content.py:npc`

| file | today | spec |
|---|---|---|
| `spacecomponentsbytype` | none | 11,238 types, record 1,832 bytes, 40-odd optional components **[V]**. `npc/components.tsv` is `type_id component` (a presence index). `npc/reactive-spawn.tsv` is `type_id scope(dungeon\|faction) key spawn_table_id initial_fleet_type_id owner_id fleet_counter_table_id bounty_payout_multiplier friendly_response_threshold`. `npc/bounty.tsv` is `type_id owner_key multiplier` (`entityBounty.bountyMultipliersByOwner`). |
| `npccorporations` | baked: `model.rs`, `npc.rs` (ids) | `npc/corporations.tsv`: `corp_id faction_id station_id system_id friend_id enemy_id tax_rate min_security size extent lp_offer_tables ticker name`. 283 rows **[V]**. |
| `factions` | baked: `npc.rs:381` (Angel only) | `npc/factions.tsv`: `faction_id corporation_id militia_corporation_id system_id member_races name`. 27 rows **[V]**. |
| `crates`, `grouprewards`, `contrabandtypes`, `wrecks.static` | none | P2. `crates.lootTableByRace` and the dungeon entity `additionalLootId` point at tables that are not in the corpus **[V]** (`dungeon-model.md` §6), so loot is authored (C6). **[V]** `combat.rs:313` takes wrecks from `types.wreckTypeID`, not from `wrecks.static`. |

### 2.7 Agents and missions: `content.py:agents`

| file | today | spec |
|---|---|---|
| `npccharacters` | baked: one agent (`mission.rs:70`, `service.rs:4346`) | `agents/agents.tsv`: `agent_id corp_id station_id level division_id agent_type locate gender bloodline_id name`. The struct is `agent{agentLevel, agentDivisionID, agentType, agentLocateCharacter, storyLineMissions}` **[V]**. `agents/npc-characters.tsv` holds the non-agent rows (CEOs, `employmentRecord`). |
| `missions` | baked: `mission.rs` (COURIER) | `agents/missions.tsv`: `mission_id name_id agent_type_id corp_id faction_id division_id kind dungeon_id content_template objective_type_id objective_qty reward_type_id reward_qty bonus_type_id bonus_qty bonus_interval lp_alpha lp_omega expiration system_id station_id`. Here `kind` is `kill` or `courier`, taken from which struct's presence bit is set **[V]**. `agents/mission-messages.tsv`: `mission_id key message_id`. |
| `agenttypes`, `npccorporationdivisions`, `epicarcs` | none | `agent-types.tsv` (scalar row), `divisions.tsv`, and `epic-arcs.tsv` (`arc_id mission_id agent_id next_missions fail_mission_id`). The pool design belongs to C4. |
| `schools`, `bloodlines` | baked: `service/character.rs` | The client loads these itself. The server must agree on `schoolID` and its start station only. `agents/schools.tsv` joins `schoolmap` (`school_id system_id`). |

### 2.8 Industry, stations and economy: `content.py:industry`

`blueprints.static` is baked as 23 of 4,890 rows. Its spec is
`industry/blueprints.tsv`, long form:
`bp_type_id activity kind type_id quantity probability time max_runs`, where
`kind` is `material`, `product` or `skill`. `stationoperations` is baked in
`reprocess.rs`, `industry.rs` and `world.rs`, and with `stationservices`
becomes `stations/operations.tsv`:
`operation_id activity_id services station_types manufacturing_factor
research_factor`. **[I]** Which NPC station offers market, repair and
reprocessing is decided here. The `industry_*` sqlite tables stay `ref`
until C5 says the cost model needs them.

### 2.9 What the server must never copy, only agree with

`dialogs.static` (4,223 keys), `accountingentrytypes`, the `.schema`
sidecars and every presentation table. **[V]** The server emits keys
(`store.rs:310`, `fleet.rs:254`) and the client resolves them. **The
extractor's job here is validation**: it checks every dialog key and
refType the server source mentions against the client. That is
`tools/loctable.py`'s existing role.

---

## 3. The `.pyd` loaders

**[V]** One `bin64/<stem>Loader.pyd` per cFSD file (149 of them,
case-insensitive; `fsd.py:1859 _find_loader_pyd`). They feed no server
subsystem; they are the **layout source** for decoding. Owner:
`tools/content.py`, which calls `pyd_schema.schema_for(path, stem)` at
extract time and checks the result against `findings/pyd-schemas.tsv`.
The extractor refuses to run if the two disagree on stride, delta or any
field offset, so a client update fails loudly instead of decoding garbage.
**[I]** It does this rather than trust the committed TSV alone, because the
TSV is a snapshot of one build.

---

## 4. cFSD layout gaps that actually block content

| gap | blocks | severity |
|---|---|---|
| **No generic decoder.** The `PydSchema` loader proposed in `native-loaders.md` §7 is unapplied. `fsd.cfsd_rows` raises `no confirmed columns` for any stem outside `CFSD_COLUMNS` (`fsd.py:1604`) **[V]**. | every P1 cFSD table outside the 7 + 11: `spacecomponentsbytype`, `npccorporations`, `factions`, `archetypes`, `agentsinspace`, `typematerials`, `typelist`, `epicarcs`, `agenttypes`, `npccorporationdivisions`, `stationoperations`, `crates` | **blocking**: packet K1 |
| **Nested struct strides are not in the TSV.** `pyd-schemas.tsv` has `#record stride` for 139 tables but no `#struct` size row (**[V]** `awk '$2~/#/'` finds only `#record`). `list<struct:X>` and `dict<k,struct:X>` need an element stride. | `dungeons.connections`/`standingRestrictions`, `spacecomponentsbytype` `spawnDataBy*`, `epicarcs.missions`, `npccorporations.divisions`, `typematerials` | **[I]** computable as `align_up(span, align)`, the same rule `native-loaders.md` §2 uses for records. Every use must be verified by a closure test (all element `typeID`s close into `types`). |
| `dungeons.assetManipulations` (record, room and object level) is `dict<str,unknown>` **[V]** | nothing in v1 | **[I]** environment dressing. Skip the field. |
| `objectivechains` `contextParameterTrigger/value` is `unknown` **[V]** | Agency objective triggers only | P2. C2 decides whether the server needs it. |
| `seasonselection` has no stride (a header-only singleton); `effectsequences` delta is **[I]** | nothing | P3, client-only |
| Signedness of `i32` and enum names are `unknown` (`native-loaders.md` §6) | `missions.kind`-style enums would need a name map | write raw ints, and let the server own the enum constants |

**Not layout gaps, but they block content just as hard, because the
reference leaves the corpus** (**[V]**, `dungeon-model.md` §5.5 and §6,
`npc-behaviors.md` §1.4):

- `spawnTableID` (504 ids)
- `npcFleetTypeID` (71)
- `npcGroupingID`
- `fleetCounterTableID`
- `crates.lootTableByRace`
- `additionalLootId`
- `npccorporations.lpOfferTables`

All of these are **authored**, and C4, C5 and C6 own them. C1's
contribution is that the extractor writes these ids out verbatim, so an
authored table can be keyed on them and checked for coverage.

---

## 5. The unified extractor: `tools/content.py` → `out/content/`

### 5.1 Shape

```
tools/analysis.sh python3 -I /work/tools/content.py \
    --client /client --resfiles /client-root/ResFiles \
    --out /work/out/content [--sections universe,belts,types,...] [--systems 30000001,30000003]
```

- **One file, stdlib only**, and a library over the existing readers
  (`fsd`, `dogma`, `pyd_schema`, `loctable`). **It imports, it does not
  copy**: `celestials.rows_for`, `asteroids`, `pi_statics`, `typedata` and
  `skills` keep their logic, and each `main()` stays as a thin wrapper for
  one release.
- **Sections**: `universe belts types planets dungeons npc agents industry
  agency`, matching the `owner_extractor` column. Each section is a
  function `section_<name>(ctx) -> [(relpath, header_cols, rows_iter)]`, so
  a test can run one section on its own.
- **Atomic output**: everything is written to `out/content.tmp-<pid>/`,
  `MANIFEST.tsv` last, then renamed over `out/content/`. A half-written
  bundle can never be loaded.
- **`--systems`** restricts universe and belts to a sandbox, the way
  `celestials.py --systems` does today. The manifest records the
  restriction.

### 5.2 The manifest: `out/content/MANIFEST.tsv`

```
# eve-offline content bundle. CCP's data: never commit this directory.
format	1
client_build	3619879
extractor	<git sha of tools/content.py, or "dirty">
systems	all | 30000001,30000003
file	universe/systems.tsv	<rows>	<sha256>	res:/staticdata/systems.static,res:/staticdata/solarsystemcontent.static
file	...
source	res:/staticdata/systems.static	<md5 from resfileindex.txt>
```

The `source` md5 comes from `resfileindex.txt` column 3 **[V]**
(`static-corpus.md` §2). The extractor can therefore say "nothing changed"
and skip work, which replaces `up.sh`'s ad-hoc staleness probe
(`grep -q '^customsoffice'`, `up.sh:345`).

### 5.3 File format

Use the house format. **TSV, `#` comments, one record kind per file**, or a
leading `kind` column where a file already uses one (`typedata.tsv`,
`pi-statics.tsv`). Lists are `a,b,c` and maps are `k=v;k=v`, which is
`typedata.tsv`'s `attrs` convention **[V]**. Nested data is flattened into
long-form files, never a nested encoding, so `std`-only Rust parses every
file with `split('\t')`. Floats are written with `repr`, as `typedata.py`
already does for bit-exactness. Names come last, because they are the only
free text.

Every file under `out/content/`, in the order the packets build it:

- `universe/`:
  - `systems.tsv`: `system_id region_id constellation_id faction_id
    owner_faction security security_class hub border fringe corridor
    regional international wormhole_class_id disallow_scanning dscan_range
    sun_type_id name`
  - `regions.tsv`
  - `constellations.tsv`
  - `celestials.tsv`: the existing 10-column `celestials.py` format, for
    every system
  - `dock-locators.tsv`
- `belts/`:
  - `rocks.tsv`: the existing format
  - `ore-by-system.tsv`
- `types/`:
  - `typedata.tsv`: the existing format
  - `market.tsv`
  - `marketgroups.tsv`
  - `skills.tsv`: the existing format
  - `typelists.tsv`
  - `materials.tsv`
  - `materials-random.tsv`
  - `compress.tsv`
- `planets/`:
  - `pi-statics.tsv`: the existing format
- `dungeons/`:
  - `dungeons.tsv`
  - `rooms.tsv`
  - `objects.tsv`
  - `gates.tsv`
  - `spawns.tsv`
  - `archetypes.tsv`
  - `resources.tsv`
  - `room-rocks.tsv`
  - `agents-in-space.tsv`
- `npc/`:
  - `components.tsv`
  - `reactive-spawn.tsv`
  - `bounty.tsv`
  - `corporations.tsv`
  - `factions.tsv`
- `agents/`:
  - `agents.tsv`
  - `npc-characters.tsv`
  - `missions.tsv`
  - `mission-messages.tsv`
  - `agent-types.tsv`
  - `divisions.tsv`
  - `epic-arcs.tsv`
  - `schools.tsv`
- `industry/`:
  - `blueprints.tsv`
- `stations/`:
  - `operations.tsv`

**[I]** Size. `objects.tsv` is about 260k rows and `celestials.tsv` about
477k (the latter is the size of today's `celestials-all.tsv`, **[V]**
477,083 lines). Both are acceptable for a load at startup. The C7 liveness
lane decides whether loading becomes lazy, per system.

### 5.4 The server flag: `--content DIR`

The spec for a new module `server/crates/eve-server/src/content.rs`, owned
by packet K3:

- `content::Bundle::open(dir) -> Result<Bundle, String>` reads
  `MANIFEST.tsv` and refuses three cases:
  - `format` major ≠ 1;
  - `client_build` ≠ the build the handshake advertises;
  - a listed file that is missing or has the wrong row count.

  The check is on row count, not sha, for startup speed **[A]**. That is
  falsified if a corrupted file passes the count check in practice.
- `Bundle::install(&self, cfg: &mut Config)` calls the **existing**
  loaders, unchanged:
  - `load_celestials(dir/universe/celestials.tsv)`
  - `asteroid::load(dir/belts/rocks.tsv, ..)`
  - `planet::install(Statics::load(..))`
  - `typedata::install(Table::load(..))`
  - `skill::install(Table::load(..))`
- New sections install only once their consumer exists. Each subsystem
  exposes `fn install_from(bundle: &Bundle)`, and a file that is absent
  means fallback to the baked rows. That is the contract `--typedata`
  already set: "Those rows stay, as the fallback for a server started
  without an extract" (`typedata.py:7-10`).
- **Precedence**: an explicit `--celestials`, `--belt-rocks`, `--planets`,
  `--typedata` or `--skills` overrides that one file of the bundle, so
  `bots.sh` and the tests keep working. The five flags are deprecated in
  the usage text one wave later.
- A startup summary line per section, printed the same way as the
  existing `println!("types: ...")`:
  `content: <section> <file> <rows> (bundle <sha8>)`.
- `tools/up.sh`: replace the four "first run only" blocks (`up.sh:328-381`)
  with one `content.py` run whenever `MANIFEST.tsv` is absent or its
  `source` md5s differ, then pass `--content out/content`.

---

## 6. Proposals

1. **Adopt `tools/content.py` as the single owner of every server-consumed
   corpus file**, with the section ownership in the TSV's
   `owner_extractor` column. The five existing extractors become imported
   libraries, and no new standalone extractor is written after this.
2. **Land the generic cFSD decoder first (packet K1)**, before any C3-C6
   build packet. It is the one patch every P1 table waits on.
3. **Make the bundle versioned and self-describing.** `MANIFEST.tsv`
   records format, client build, extractor sha, per-file rows, sha256 and
   source md5s. The server refuses a mismatched bundle with a readable
   error, the same as `--typedata` refuses an empty one today.
4. **Add `--content DIR`, keep the five old flags as per-file overrides,
   and change no existing loader.** Phase 1 is plumbing only, and it is
   byte-identical for the five existing files.
5. **Write out-of-corpus references verbatim** (`spawnTableID`,
   `npcFleetTypeID`, `npcGroupingID`, loot ids, `lpOfferTables`), so the
   authored tables C4-C6 propose can be keyed on them and get a coverage
   test of the form "every id in `spawns.tsv` has an authored row or an
   explicit default".
6. **Add the market, meta and wreck columns as a new `types/market.tsv`**
   rather than as new `typedata.tsv` columns, so the existing parser does
   not change, and add category 25 to the type extract.
7. **Treat `reprocess/tables.rs` and `industry/tables.rs` as fallbacks.**
   Their consumers read `types/materials.tsv`, `types/compress.tsv` and
   `industry/blueprints.tsv` when the bundle has them. Flag the two
   committed generated files to the coordinator under the no-client-data
   rule.
8. **Validation is part of extraction.** `content.py --check` runs closure
   tests and fails the extract on any miss:
   - every `*_type_id` closes into `types`;
   - `room-rocks.tsv` equals the room objects;
   - every dungeon `allowed_ships_list` closes into `typelists`;
   - every dialog key in `server/` closes into `dialogs.static`.

---

## 7. Packets

Each packet is one build lane. Ownership does not collide: K1 owns only
`tools/fsd.py`, K2 owns only `tools/content.py`, and K3 owns only the
server side.

| # | packet | owns | tests |
|---|---|---|---|
| **K1** | Generic cFSD decoder: apply `native-loaders.md` §7's `PydSchema` loader to `tools/fsd.py`, add nested-struct strides (`align_up(span, align)`), and expose `fsd.rows(client, stem)` for any of the 139 tables. | `tools/fsd.py` | `tools/test_fsd_generic.py` (new, stdlib `unittest`; runs in `analysis.sh`). Assertions: (a) the generic decode equals `cfsd_rows` on all 7 `CFSD_COLUMNS` tables and `dogma.Table` on all 11 `SCHEMAS` tables, row for row; (b) for every table with a stride, the decoded stride equals the container's; (c) `spacecomponentsbytype` `reactiveSpawnComponent` decodes on more than 0 rows, and every `spawnTableID` is non-zero; (d) `npccorporations.factionID` closes 100% into `factions`. |
| **K2** | `tools/content.py`, phase 1: the manifest, atomic write, `--sections`, `--systems` and `--check`; the sections `universe` (+`systems.tsv`), `belts`, `types` (existing files only), `planets`. Old extractors are imported. | `tools/content.py`, `tools/test_content.py` | The four phase-1 files are byte-identical to `celestials.py`, `asteroids.py`, `pi_statics.py`, `typedata.py` and `skills.py` output on the same client; the manifest row counts match; a killed run leaves no `out/content/`; `systems.tsv` has 8,490 rows, and `security` equals `solarsystemcontent` for every row. |
| **K3** | `--content DIR` in the server. | `src/content.rs` (new), and the arg arm in `src/bin/eve-server.rs`; `tools/up.sh` lines 328-381 | `content.rs` unit tests on a synthetic temp bundle: wrong `format` → Err; wrong build → Err; missing file → Err naming it; an explicit `--typedata` overrides the bundle's file; an absent optional section → Ok, fallback. No client data in the tests. |
| **K4** | `content.py` phase 2: the sections `dungeons`, `npc`, `agents`, `industry` and `types/{market,marketgroups,typelists,materials*,compress}`. Depends on K1. | `tools/content.py` (after K2 lands) | `--check` closures (Proposal 8); `objects.tsv` has 260,242 rows and `spawns.tsv` 30,610 (`dungeon-model.md` §1); `room-rocks.tsv` equals the room-object counts on 3,054 of 3,054 cells; `market.tsv` has 53,288 rows; `missions.tsv` has 2,892 rows, each with exactly one `kind`. |
| **K5** | Consumers switch to the bundle, one per subsystem lane, each owning its own module: `reprocess.rs` reads `materials/compress`, `industry.rs` reads `blueprints`, `crimewatch.rs` reads `systems.security`. | that subsystem's file | Each module tests that, with the bundle absent, behaviour equals today's baked rows; and with a synthetic two-row bundle present, the bundle rows win. |

Order: K1 and K2 in parallel, then K3, then K4, then K5. C3-C6 build
packets depend on K4's files, not on raw corpus access.

---

## 8. unknown

- The key semantics of `wrecks.static` `raceIdToWreckTypeId` (294 rows)
  are unknown: whether the key is a group, a type or a size class.
- `locationcache.static` (481,068 rows of id → int): what the value is,
  and whether the client needs the server to agree with it. **[I]** It is
  a client lookup cache.
- Whether `dungeons` gate destinations are "the next room in id order".
  The same is open in `dungeon-model.md` §2. K4 writes `from_object_id`
  only.
- Whether `clientlocationlists` is what `essreservekeys.locationListID`
  points at. **[I]** from the name; C2 owns this.
- What `asteroidtypesbysolarsystemid` covers: 3,974 of 8,490 systems
  **[V]**. Whether the other 4,516 are wormhole, Pochven or beltless
  systems, or whether the table is simply sparse.
- The signedness of every cFSD `i32` (`native-loaders.md` §6). This
  matters only for ids at or above 2^31; none is known in these tables.
- Whether loading every section eagerly at startup fits the C7 tick and
  memory budget. `objects.tsv` plus `celestials.tsv` is about 740k rows
  **[I]**.
- The scale of the `industry_*` tables' role in C5's cost model.
- The source tool for the committed `reprocess/tables.rs` and
  `industry/tables.rs`: neither is in `tools/`. **[V]** by grep.
