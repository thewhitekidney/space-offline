<!-- Provenance: the `evejs-data-static` research lane, 2026-10-22. Read-only:
     nothing outside this file and `docs/lanes/evejs-data-static.md` was
     written, no cargo was run, and **nothing in the archive was executed** --
     every claim below comes from `cat`/`grep`/`sed` and from `python3 -I`
     scripts that live in the session scratchpad and take paths as arguments.
     The operating constraints of `docs/archive/evejs-261018-reference.md` §0 bind this
     document in full: no third-party or extracted client data is reproduced
     here beyond schemas, table and column names, types, row counts and one
     sample shape. Marks are the house convention: **[V]** read out of the
     artefact, **[I]** inferred, **[A]** assumed (with its falsifier). -->

# What data EvEJS ships, where it came from, and which of it we lack

**The question, in the owner's words:** *"evejs datamining — they ship data
streamed by the server that is only datamineable, lets figure out what and how
it complements."*

**The question, stated precisely.** In real EVE a large amount of data reaches
the client only as a stream from the server: market orders, agent dialogue,
corporation and alliance rows, sovereignty, station services and hangar
contents, NPC populations, mission offers, standings, the universe's dynamic
state. A server has to *author* it. EvEJS is a server that already authored a
plausible version, and it is sitting on this disk. Which of that do we lack,
and where would we get it?

Companion reading: `docs/archive/evejs-261018-reference.md` (§0 first, it binds),
`docs/re/evejs-crosscheck.md` (eight of its claims did not survive),
`docs/evejs/evejs-subsystem-gap.md` (how much game they built),
`docs/re/static-corpus.md` + `findings/corpus-inventory.tsv` +
`findings/pyd-schemas.tsv` (our own static corpus — the other half of every
row in the table below).

Contents: 1 headline · 2 what their `init` actually builds · 3 the artefact
table · 4 the gap, ranked, with how to get each one ourselves · 5 their schema
as a design document · 6 the build delta · 7 the do-not-bother list · 8 open
questions by cost · 9 what `evejs-probe` must settle.

---

## 1. Headline

**Almost none of their data is data we need, and the exceptions are a short
list.** Three findings carry the document.

**1. Their "authority" tables are mostly a re-export of the client's own
`ResFiles`, from an older build than ours.** **[V]** 24 of the 42 static
tables they ship name a `res:/staticdata/*` file or a CCP client build in
their own `source` block — `missions.fsdbinary`, `dungeons.fsdbinary`,
`nodegraphs.fsdbinary`, `typematerials.fsdbinary`,
`spacecomponentsbytype.fsdbinary`, `graphiclocations.fsdbinary`,
`fighterabilities.static`, `asteroidtypesbysolarsystemid.pickle`,
`pvpfilament*.fsdbinary`, `expertsystems.fsdbinary`,
`stationstandingsrestrictions.fsdbinary` and the rest. We ship a reader for
all of those (`tools/fsd.py`, `tools/pyd_schema.py`) and our client is
*newer*. Porting those rows would be importing a stale copy of a file we can
decode ourselves.

**2. The one thing a server genuinely has to invent — which agent offers which
mission — is empty in this release.** **[V]**
`agentAuthority.counts.missionPoolCount = 0`,
`missionTemplateCount = 0`, `missionPoolsByKindAndLevel = {}`, and **0 of
10,941 agents carry a non-empty `missionTemplateIDs`**. **[V]** Every one of
the 10,941 agents carries `conversationMetadata = {placeholder: true}` — they
did not author agent dialogue either. `tools/DatabaseCreator/production-mission-policy.js`
is the sanitiser that strips it, and `buildAgentAuthority`'s own
`publicSdeLimitations` says so: *"Mission template pools are local EvEJS
authority data and are not present in public SDE JSONL."* The interesting
column is blank in the shipped artefact.

**3. What they invented that is actually missing from our client is narrower
than it looks, and §4.1 measures exactly where the seam is.** This lane's one
new piece of primary evidence, read out of our own client for this document:
**`spacecomponentsbytype.fsdbinary` carries CCP's own NPC spawn and aggression
parameters, per type, and `tools/fsd.py` reads them today.** **[V]** Of its
11,238 rows, **7,427** carry
`entityStandings{friendlyResponseThreshold, hostileResponseThreshold}` and
**882** carry a 27-field `reactiveSpawnComponent` whose values are real game
design — `threatRangeMeters` takes 9 distinct values with **250 km on 681 of
882**, and `(min,max)SpawnIntervalSeconds` takes 81 distinct pairs. **So
respawn timing and NPC threat range, which `docs/archive/roadmap.md` lists among the
things "not shipped", are shipped.** What is *not* shipped is what that
component's foreign keys point at: **[V]** `spawnTableID` takes **504 distinct
values** and `initialSpawnNpcFleetTypeID` **71**, and no file under
`res:/staticdata/` is a spawn table, an NPC fleet type table or a fleet
counter table (checked against all 203 names). **[V]** `behavior` is present
on 2,419 types but blank — `groupBehaviorID` is `0` on 2,411 of them. **The
gap is three tables and a behaviour tree, not 5,639 rows of design.**

The rest of the genuine gap: the market order book and its cost model, LP
store offers (9.4 MB, 178 corporations, straight from CCP's own ESI), wormhole
statics (3,015 slots across 2,490 of 8,490 systems), moon ore composition
(seeded PRNG — they do not have real TQ data either), moon mining anchor
points (157,905 positions), NES store offers, insurance prices, and per-station
dock geometry. Everything else is either in our `ResFiles`, fetchable from
CCP, or build-wrong.

A number worth keeping: **their mission table is 2,878 rows at build 3396210;
ours is 2,892 at 3619879. Their client dungeon table is 5,393; ours is 5,516.
Their agent table is 10,941 (from an SDE snapshot stamped 3294658); our
`npccharacters.fsdbinary` is 11,424.** **[V]** Every build-keyed table they
ship is short of ours by a measurable margin.

---

## 2. What their `init` actually builds, from the migration and schema files alone

**[V]** There are no `.sql` migrations and no column-typed schema. The store
is `better-sqlite3` with **one table per logical table, each `(key TEXT PRIMARY
KEY, json TEXT NOT NULL)`** (`server/src/gameStore/sqliteStore.js:853`), plus
`_persistence_owners` and `_migrations`. The "schema" is therefore the *table
name list* and `ROW_GROUPS`, which names which sub-collections of each JSON
blob are sharded into rows.

**[V]** `tools/DatabaseCreator/database-creator.js` declares exactly three
populations, and set arithmetic over them is clean:

| population | count | what it is |
|---|---:|---|
| `REQUIRED_TABLES` | **153** | every table the server expects to exist |
| `GENERATED_TABLES` | **36** | built at `init` from the SDE JSONL |
| `staticTables/` | **42** | shipped in the archive as `data.json` |
| runtime/empty | **77** | `REQUIRED − GENERATED − static`: created empty, filled by play |

The three sets overlap in exactly one place: **`agentAuthority` is both
shipped and generated** — `init` rebuilds it from the SDE and
`PRESERVED_STATIC_AUTHORITY_TABLES` keeps the shipped copy's hand-patched rows
(`agentAuthority`, `dungeonAuthority`, `missionAuthority` are the three
preserved). `stationDockingPlacements` is the one shipped table that is not in
`REQUIRED_TABLES`: **[V]** its own `source.note` says it is *"Merged into
SDE-generated stations by database-creator.js"*.

**The SDE fetch.** **[V]** `DEFAULT_SDE_URL` is
`https://developers.eveonline.com/static-data/tranquility/eve-online-static-data-3396210-jsonl.zip`,
`DEFAULT_BUILD` is `3396210`, and `loadSdeAuthority` reads 33 named `.jsonl`
files from it: `_sde`, `categories`, `groups`, `types`, `dogmaAttributes`,
`dogmaEffects`, `typeDogma`, `mapConstellations`, `mapRegions`, `mapStars`,
`mapSolarSystems`, `mapStargates`, `mapPlanets`, `mapMoons`,
`mapAsteroidBelts`, `npcCorporations`, `npcCharacters`, `agentTypes`,
`stationOperations`, `npcStations`, `blueprints`, `races`, `bloodlines`,
`factions`, `typeMaterials`, `dbuffCollections`, `dynamicItemAttributes`,
`icons`, `typeLists`, `planetResources`, `planetSchematics`,
`sovereigntyUpgrades`, `skins`, `skinMaterials`, `skinLicenses`. **This is
CCP's official static data and anyone may download it.**

**[V] That URL is live, and the equivalent URL for our build is not.** A
range-GET of their 3396210 archive returns `206 Partial Content`; the same
path with `3619879` returns `403`, and so does an `-latest-` alias. So CCP
publishes per-build archives and ours is either not published or not published
under that name. See §8 Q2.

**Three of the 36 "generated" tables are not generated at all.** **[V]**
`npcCargo`, `npcWrecks` and `npcWreckItems` are emitted as an empty collection
plus a single id allocator (`nextCargoID: 980200000000`,
`nextWreckID: 980300000000`, `nextWreckItemID: 980400000000`). They are
runtime tables with a reserved id range, mislabelled. **[V]**
`mapTagsAuthority` is emitted empty with the reason *"No public SDE map-tag
source data identified; emit empty authority shape"* — an honest admission
that map tags are server-authored and they did not author them.
`shipSkinComponentListingAuthority` ships with `listings: []` by design.

---

## 3. The artefact table

One row per data artefact they ship or generate. Paths are relative to the
archive root. `provenance` is one of **SDE** (CCP official static data, freely
downloadable), **client** (decoded out of CCP client `ResFiles`, i.e. our own
corpus), **authored** (they invented or reconstructed it), **runtime** (fetched
from a third party at build/run time), **derived** (computed from one of the
above).

### 3.1 The 42 shipped static tables — `tools/DatabaseCreator/staticTables/<name>/data.json`

| artefact | what it is | provenance | our client has it | do we need it, for what |
|---|---|---|---|---|
| `missionAuthority` (2,878 missions, 17,164 localised messages) | re-export of the client mission table + localisation | **client** [V] — `source.exportSource.resourceEntries` names `missions.fsdbinary`, `epicarcs.fsdbinary`, `contenttags(types).fsdbinary`, `localization_fsd_en-us.pickle` | **yes, and more** — `missions.fsdbinary` 2,892 rows, 24 named columns incl. `missionRewards`, `killMission`, `courierMission`, `clientObjectives`, `fixedLpReward{Alpha,Omega}` (`findings/pyd-schemas.tsv`) | missions. **Do not port** — §7 |
| `dungeonAuthority` (5,009 templates over 5,393 client dungeons, 68 MB) | re-export + spawn-profile supplements | **client** + **authored** [V] — `source.client.resourceEntries` names `nodegraphs`, `objectives`, `objectivechains`, `environmenttemplate*`; `sourcePriorities` grades `client` 100 down to `fallback` 20 | **yes** — `dungeons.fsdbinary` 5,516 rows with rooms and per-room object lists (`static-corpus.md` §1) | NPCs, sites, missions. Port **only** the `spawnProfiles`/`sourceConfidence` *shape*, not the rows |
| `agentAuthority` (10,941 agents) | agent roster + a mission-pool key | **SDE** [V] — `npcCharacters.jsonl` from snapshot **3294658** | **yes, and more** — `npccharacters.fsdbinary` 11,424 rows, full field layout incl. `agent.{agentType,agentLevel,agentDivisionID,agentLocateCharacter,storyLineMissions}` and `skills:list<{typeID,skillLevel}>` | agents. **Do not port the rows; port the pool-key rule** — §4.1 |
| `dungeonClientContent` (26 templates, 4,097 objects) | exact client dungeon geometry | **client** [V] — `source.provider: ElysianSDE`, `clientBuild: 3475087`, `dungeons.jsonl` 55 MB | **yes** | dungeons in space. Do not port |
| `moonMiningPoints` (157,905 points) | `{moonID, solarSystemID, beaconID, position{x,y,z}, distanceFromMoon}` | **runtime/authored** [V] — *"EveJS community moon-mining-point contribution"*, validated against their own celestial table | **no** | moon mining, structures. **§4.5** — derivable |
| `explorationWormholeStatic` (8,490 systems, 2,490 with statics, 3,015 static slots, 99 code types, 42 wandering profiles) | which wormhole each system statically connects to | **authored** [V] — `staticSpec: scripts/DataSync/spec/explorationWormholeStatics.json`, `wanderingSpec: …WanderingProfiles.json` | **partly** — the 99 wormhole *types* and their class/lifetime/mass attributes are dogma on `types.fsdbinary`; the per-system *assignment* is not | wormholes, multi-system. **§4.3** |
| `npcProfiles` (5,620) | `{profileID, name, shipTypeID, presentationTypeID, corporationID, allianceID, factionID, behaviorProfileID, loadoutID, lootTableID, shipNameTemplate, securityStatus, bounty, spawnDistanceMeters, preferredTargetMode}` | **authored** (names/typeIDs derived from `itemTypes`) [I] — no `source` block at all | **no** (the typeIDs yes, the profile no) | NPCs and bots. **§4.1** |
| `npcBehaviorProfiles` (5,639) | `{thinkIntervalMs, autoAggro, autoAggroTargetClasses, targetPreference, movementMode, orbitDistanceMeters, followRangeMeters, aggressionRangeMeters, leashRangeMeters, returnToHomeWhenIdle, homeArrivalMeters, autoActivateWeapons}` | **authored** [V] — no source block, pure design | **no** | NPCs and bots. **§4.1 — the single most valuable row in this document** |
| `npcLoadouts` (112) | `{loadoutID, modules:[{typeID,quantity}], charges:[{typeID,quantityPerModule,singleton}]}` | **authored** [I] | **no** | NPCs, module activation. §4.1 |
| `npcLootTables` (61) | `{lootTableID, minEntries, maxEntries, stackableMin/MaxQuantity}` (+ `entries`/`guaranteedEntries` in the capital variant) | **authored** [V] | **no** | damage and death, loot. §4.1 |
| `npcSpawnPools` (83) | weighted `{profileID|spawnPoolID, weight}` lists | **authored** [V] | **no** | NPCs. §4.1 |
| `npcSpawnGroups` (11) | named groups of `{profileID|spawnPoolID, count}` | **authored** [V] | **no** | NPCs. §4.1 |
| `npcStartupRules` (2, **both `enabled: false`**) | per-system spawn-at-scene-create rules with an `anchorSelector` (`kind: stargate`, `mode: each`, distances) | **authored** [V] | **no** | NPCs, stargates. §4.1 — the *shape* is the value |
| `npcHostileUtilities` (12 templates) | NPC EWAR/utility templates | **authored** [I] | **no** | NPCs, targeting. §4.1 |
| `capitalNpcAuthority` (18 entries, 6 factions, 10 dreads / 4 titans / 4 supercarriers) | fully-specified capital NPCs with inline loadout, behaviour and loot | **authored** [V] — `authorityFile: scripts/npcCapitals/spec/capitalNpcAuthority.json` | **no** | NPCs. §4.1 (the inline shape is the best worked example) |
| `npcStandingsAuthority` (303 owners, 978 relations) | NPC↔NPC standings | **SDE/derived** [V] — `npcCorporations.jsonl` + `factions.jsonl`, keyed on `enemyID`/`friendID` | **yes** — `npccorporations.fsdbinary` carries `enemyID`, `friendID`, `factionID`; `factions.fsdbinary` 27 rows | standings. **Do not port — derive** (§7) |
| `researchFieldAuthority` (244 research agents, 19 fields) | which R&D agent researches what | **SDE/derived** [V] — its own note: *"Research fields are `npcCharacters[].skills`, present on `agentTypeID 4` only. Datacore mapping is dogma attribute 182 on group 333."* | **yes** — both halves | agents, R&D. **Do not port — the note is the whole recipe** |
| `asteroidTypesBySolarSystemID` (3,974 systems, 36 ore types, 24,071 assignments) | which ores a system's belts may hold | **client** [V] — `asteroidtypesbysolarsystemid.pickle` | **yes** (pickle, fully readable, 3,974 rows) | mining. Do not port |
| `clientEntityStandings` (7,343 types) | `{typeID, friendlyResponseThreshold, hostileResponseThreshold}` | **client** [V] — `spacecomponentsbytype.fsdbinary` | **yes** (11,238 rows, stride 1,832) | NPC aggression. Do not port — decode |
| `clientTowComponents` (2 types) | tow game-objective components | **client** [V] — same file, md5 quoted | **yes** | — |
| `clientMicroJumpComponents` (2) / `clientShipLinkComponents` (3) | space-component subsets | **client** [V] — from build **3487903**, retargeted to 3396210; `scope` admits *"target-build raw component extraction remains unverified"* | **yes** | module activation. Do not port |
| `clientPvpFilaments` (86 match types, 118 event schedules) | filament catalogue | **client** [V] — `pvpfilamentmatchtypes.fsdbinary` + `pvpfilamenteventdates.fsdbinary`, md5+sha256 quoted | **yes** (both files present) | — |
| `expertSystems` (51) | Expert System definitions | **client** [V] — `expertsystems.jsonl` mirror | **yes** — `expertsystems.fsdbinary`, 59 rows | — |
| `fighterAbilities` (36 abilities, 94 fighter types) | fighter ability table | **client** [V] — `fighterabilities.static` + `fighterabilitiesbytype.static` + `fighterConst.py` | **yes** (both `.static`, classic FSD, fully readable) | carriers. Do not port |
| `reprocessingClientRandomizedMaterials` (10 types, 24 entries) | randomised refine outputs | **client** [V] — `typematerials.fsdbinary` | **yes** (9,866 rows) | reprocessing. Do not port |
| `stationGraphicLocators` (44) / `structureGraphicLocators` (19) | dock/undock locator geometry per station type | **client** [V] — `graphicLocations.fsdbinary` | **yes** (68 rows) | docking, undock. Do not port |
| `stationStandingsRestrictions` (1 owner, 7 rules) | standing gates on docking | **client** [V] — resource path quoted | **yes** (2 rows) | docking. Do not port |
| `stationDockingPlacements` (5,210 rows; columns `stationID, stationTypeID, dockEntry, dockPosition, dockOrientation, undockDirection, undockPosition`) | per-station absolute dock/undock geometry | **derived from client** [V] — *"EvEJS reverse engineered client station placement data"* | **derivable, not shipped as such** — station position is in `solarsystemcontent.static`, the type-local locator is in `graphiclocations.fsdbinary` | **undock and docking.** §4.4 |
| `shipSkinComponentAuthority` (597 components, 901 items, 8 slots) | SKINR component catalogue | **client** [V] — ClientSDE **3487903**, with 124 `excludedPhysicalTypeIDs` because the donor build has types 3396210 does not | **yes** — `ship_skin_design_component*.fsdbinary`, `ship_cosmetic_slot*.fsdbinary` | cosmetics (not on the backlog). Do not port |
| `shipSkinSequencingAuthority` (49 group thresholds, 366 type→group, 20 tiers) | SKINR sequencing economy | **client** thresholds + **authored** economy [V] — `economyAuthority: "Tranquility client trace"`, `economyTrace: client/TQLogs/SHIPSEQ.txt:6674` | thresholds **yes** (`ship_skin_design_tier_thresholds.fsdbinary`); pricing **no** | not on the backlog |
| `shipSkinMarketAuthority` (fee schedules) | broker/buyer fee tables | **authored** [V] — self-labelled *"versioned best-evidence reconstructions"* from patch notes | **no** | not on the backlog |
| `shipSkinStoreAuthority` (1,674 designs) | an observed TQ design catalogue | **runtime/observed** [V] — harvested from `client/TQLogs/SHIPSEQLOADS.txt` and `TQSkinsAndStoreAugust22.txt`, `captureReachedEnd: false` | **no** | not on the backlog. **Observed player content — do not port** |
| `shipSkinComponentListingAuthority` | empty by design | **authored** [V] — `authority: "server-authored"`, *"Empty means the client receives a valid empty component listing catalogue"* | n/a | the **shape** matters: an empty-but-valid catalogue is a real answer |
| `evermarksCatalog` (818 licences/offers) | EverMarks heraldry offers | **runtime** [V] — `lpStoreUrl: https://www.fuzzwork.co.uk/lpstore/sell/10000002/1000419/withblueprints` (third-party) + SDE 3396210 | **no** | not on the backlog |
| `newEdenStore` (46 public offers, 1 store) | NES catalogue, `{storeOfferID, plexPriceInCents, fulfillment, preview, source{kind,observedAt,url}}` | **authored/observed** [V] — `seedProfile: "official-observed-2026-03-26"` | **no** | PLEX/NES (not on the backlog). §4.7 |
| `shipInsurancePrices` (473 hulls) | `{fullInsurancePriceCents, levels{Basic..Platinum}{fraction,costCents,payoutCents}}` | **runtime** [V] — `source: https://esi.evetech.net/latest/insurance/prices/`, captured 2026-04-21, with `formulas{baseInsuranceFraction: 0.4, insuranceFractionIncrement: 0.5}` | **no** (but `basePrice` is in `types.fsdbinary`) | damage and death. **§4.6 — the formula is the answer** |
| `skillTrainingAlphaCaps` (174 skills) | alpha-clone skill level caps | **runtime** [V] — `sourceUrl: https://wiki.eveuniversity.org/Clone_states?action=raw` | **yes** — `clonegrades.static` (SQLite fsdlite, 4 rows, columns `internalDescription;skills`) | skills. **Do not port — read `clonegrades.static`** (§7, and this is the clearest case) |
| `trigDrifterSpawnAuthority` (10 lists; 1,026 Jove Observatory systems, 1,022 unidentified-wormhole systems, 27 Pochven, 5 Drifter) | which systems host Trig/Drifter content | **runtime/authored** [V] — `joveObservatorySystemNames: https://jambeeno.com/jo.txt`, plus *"repo-owned static names"* | **unknown** — `landmarks.static` (45) and `factionsowningsolarsystems.static` (22) do not cover it | NPCs, multi-system. §4.8 |
| `explorationAuthority` (6 probe types, 11 launchers, 8 scan contracts) | probe/scan definitions | **derived** [V] — `itemTypes` + `typeDogma` + the client's own `probescanning/const.py` and `formations.py` | **yes** — both the dogma and the client module | exploration, scanning. Do not port — read the client module |
| `stargateVisualOverrides` (87 itemIDs → `skinMaterialSetID`) | per-gate visual override | **authored** [I] — no source block | **partly** — `graphicmaterialsets.fsdbinary` holds the sets; the per-gate assignment is not in `solarsystemcontent.static`'s stargate dict | stargates (cosmetic only). Low value |

### 3.2 Content packs — `content-packs/*.json` (19 files, 339 MB)

**[V]** All 19 share one envelope: `{schema: "evejs.content-pack.v1",
manifest{id, version, title, description, contentApi, engine{minVersion,
maxVersionExclusive}, capabilities[], builtAgainst{serverSdeBuild,
clientDungeonBuild}, counts{dungeonTemplates, spawnProfiles},
provenance{builder, …}, payloadSha256}, payload}`.

| artefact | templates / profiles | provenance | our client has it | need |
|---|---:|---|---|---|
| `evejs-ded-rated-sites-1.0.2` (89 MB) | 50 / 1 | **client** [V] — `clientDungeonBuild: 3475087` | yes | sites. Do not port the payload |
| `evejs-combat-anomalies-1.4.4` (70 MB) | 176 / 1 | **client** 3475087 | yes | " |
| `evejs-pirate-exploration-1.0.0` (66 MB) | 204 / 2 | **client** 3475087 | yes | " |
| `evejs-ore-anomalies-1.1.0` (31 MB) | 199 / 1 | **client** 3475087 | yes | " |
| `evejs-wormhole-sites-1.2.0` (31 MB) | 48 / 4 | **client** 3475087 | yes | " |
| `evejs-booster-production-1.1.0` (21 MB) | 32 / 2 | **client** 3475087 | yes | " |
| `evejs-gas-sites-1.0.0` (15 MB) | 74 / 2 | **client** 3475087 | yes | " |
| `evejs-homefront-1.0.5` (13 MB) | 24 / 24 | **client** `3503375` + **authored** [V] — `assumptions: ["Server spawn tables unavailable; encounter estimates are explicitly authored."]` | layouts yes; encounters no | NPCs |
| `evejs-drone-anomalies-1.4.0` (8 MB) | 22 / 1 | **client** 3475087 | yes | sites |
| `evejs-mining-missions-1.0.0` (4 MB) | 40 / **0** | **client** 3475087 | yes | mining missions |
| `evejs-ghost-sites-1.1.0` (4 MB) | 20 / 1 | **client** 3475087 | yes | sites |
| `evejs-air-tutorial-1.0.{0..3}` (4 versions, ~120 KB each) | 4 / 0 | **authored** [V] — `nativeClientVerified: false`, donor graph build 3487903 | no | NPE (not on the backlog) |
| `evejs-fw-open`, `-gallente-depots`, `-gallente-fabricators` (3 files, ~24 KB) | 2/2, 2/1, 1/1 | **authored** [V] — `sourceRefs` are eveuniversity wiki and CCP patch-note URLs | no | FW (not on the backlog) |

**The finding about this folder:** 331 of its 339 MB is CCP dungeon geometry
re-exported from a client build *older than ours*. The **`spawnProfiles`** —
8 kB of security bands, wormhole classes, per-region density multipliers, site
tier bands and system spawn chances — is the only authored part, and it is the
part worth reading. §4.2.

### 3.3 Other shipped data

| artefact | what it is | provenance | our client has it | need |
|---|---|---|---|---|
| `server/src/services/loyalty/lpStoreStaticOffers.json` (9.4 MB, 178 corps) | every NPC corp's LP store offers | **runtime** [V] — `sourceAuthority: "CCP ESI tranquility"`, per-corp etags and expiries recorded | **no** — `npccorporations.fsdbinary` has `lpOfferTables:list<i32>` but **no table in `res:/staticdata/` those ids point into** | **LP store, missions, agents. §4.2** |
| `server/src/_secondary/image/generated/agents/manifest.json` (18 MB, 65,274 portraits) | agent portrait manifest, 4 sizes | **runtime** [V] — `baseUrl: https://images.evetech.net/`, with a *"local client conversation-agent fallback"* at `agentPortraitAuthority/client-conversation-agents.json` | **partly** — `character_portraitresources.fsdbinary` and `conversationagents.fsdbinary` (44) exist; the 65k NPC portraits are an online service | agents. §4.9 |
| `server/src/abyssal/catalog/content/abyssalRewardTables.generated.json` (6.6 MB) + `qsnaAbyssalSpawnTables.generated.json` (752 KB) | abyssal loot and spawn tables | **runtime** [V] — *"historical Abyssal.Space complete-run loot telemetry"*, *"QSNA pages derived from Abyssal.Space telemetry"* | no | abyssals (not on the backlog) |
| `server/src/abyssal/catalog/content/retailAuthoredContent.generated.json` (4.4 MB) | abyssal room content | **authored/observed** [V] | no | " |
| `server/src/services/mining/iceSystemSyntheticNullsecAuthority.json` (1.2 MB, 596 rows) | null-sec/Pochven ice belts | **runtime/authored** [V] — *"DOTLAN supplies system and slot evidence … missing Pochven composition uses documented original-quarter best-fit templates"*, self-labelled *"Emulator-owned synthetic"* | no | mining. §4.5 |
| `server/src/services/achievement/data/definitions.json` (188 KB, 38 achievements, 32 contribution methods) | achievement tree | **client/derived** [V] — `achievementSourceMode: "client-sde-derived"` | **yes** — `achievements.static`, `achievementevents.fsdbinary`, `challenges*.fsdbinary`, `contributionmethods.fsdbinary`, `grouprewards.fsdbinary` | not on the backlog. Do not port |
| `server/src/gameStore/data/authoredSpaceProps/*.json` (16 systems: Jita, Amarr, Perimeter, Yulai, Colelie, …) | static environment props anchored to stations: `{itemID, typeID, groupID, categoryID, anchorItemID, ownerID, mode, radius, flagsByte, position, dunPosition, dunRotation, dungeonObjectID, forceDamageState, staticVisibilityScope}` | **authored/observed** [V] — descriptions read *"TQ ENV_Oceanic_01a at …"* | no | **the `AddBalls` row shape for a non-ship object — §5** |
| `server/src/_secondary/chat/staticChannelContracts.js` | static help-channel contracts, room names of the form `system_<msgID>_<msgID>` | **client/derived** [V] — `contractSource: "codeccpfull_v23.02"` | **derivable** — the two integers are localisation message ids, which `tools/loctable.py` resolves | **chat. §4.10** |
| `server/assets/overview/iridium_overview_*.yaml` | one community overview preset | **runtime** [I] | no | overview (nice-to-have) |
| `config/*.json` (8 files, ~10 KB) | tuning constants mirroring `server/src/config/schema/*.js` | **authored** [V] | n/a | our own config shape |
| `tools/market-seederv3/data/price-manifest.json` (4,771 entries) | one resolved price + provenance per seeded type | **derived + runtime** [V] — 2,308 `cost-recursive`, 1,942 `ccp-esi-average`, 239 `ccp-snapshot-jita-split`, 165 `derived-compressed-twin`, 117 `reprocessing-floor`, 0 `base-price` | **derivable** — the recursive cost model needs only `blueprints.static` + `typematerials.fsdbinary`, both ours | **the market. §4.2** |
| `externalservices/market-server` (Rust daemon + the only real SQL in the archive) | 11-table SQLite market: `manifest`, `regions`, `solar_systems`, `stations`, `market_types`, `seed_stock`, `seed_buy_orders`, `market_orders`, `market_order_events`, `region_summaries`, `system_seed_summaries`, `price_history` | **authored schema** [V] | n/a | **the market — this schema is the deliverable, not the rows. §5.2** |
| `tools/moon-data/generate-moon-composition-pack.js` | moon ore composition **generator** | **authored** [V] — `DEFAULT_SEED = "EveJS"`, *"An install keeps the same seed for good: a new seed gives every moon new ores"* | no | moon mining. **They did not datamine this either — §4.5** |

---

## 4. The gap: server-authored, and our client does not have it

Ranked by what it unblocks on our backlog (warp, stargates and multi-system,
targeting, module activation and fitting, damage and death, NPCs and bots,
chat, and the content behind them). For each: **how we would get it
ourselves.**

### 4.1 NPC behaviour, loadouts, loot and spawn tables — rank 1

**What is missing.** `npcBehaviorProfiles` (5,639), `npcProfiles` (5,620),
`npcLoadouts` (112), `npcLootTables` (61), `npcSpawnPools` (83),
`npcSpawnGroups` (11), `npcStartupRules` (2), `npcHostileUtilities` (12),
`capitalNpcAuthority` (18). **[V]** None of these has a `source` block at all —
the eight `npc*` files are a bare `{<collectionName>: [...]}` with no
provenance, which in a tree that stamps provenance on everything else is itself
the statement that they were written by hand.

**Why it is rank 1.** `docs/archive/roadmap.md` already says it: *"What is not shipped
is the part this page would have called the game design anyway: NPC aggression
and target selection, warp-in behaviour, respawn timing, loot rolls, anomaly
generation. That is the same engine as the player-like bots in the end goal."*
It is the row the end goal runs through, which is why it is first.

**But that roadmap sentence is partly wrong, and the correction below is this
lane's main result.** NPC aggression thresholds, respawn timing, threat range
and warp-in range *are* shipped, per type, in a file we already decode. Read
the correction before acting on the field list above: the list is still the
right vocabulary, but three of the five things it was meant to supply have a
client-shipped answer, and the part that has to be invented is much smaller
than the 5,639 rows suggest.

**How to get it ourselves — and the honest answer is: author it, but author it
against their field list.** The rows are invented and we should invent our own.
What is reusable with no contamination is **the parameter vocabulary**, because
it is the list of decisions an NPC engine has to make and it took them 5,639
rows to settle on fourteen fields:

- behaviour: `thinkIntervalMs`, `autoAggro`, `autoAggroTargetClasses`,
  `targetPreference`, `movementMode`, `orbitDistanceMeters`,
  `followRangeMeters`, `aggressionRangeMeters`, `leashRangeMeters`,
  `returnToHomeWhenIdle`, `homeArrivalMeters`, `autoActivateWeapons`;
- identity: `shipTypeID` **and a separate `presentationTypeID`** (the hull it
  flies versus the hull the client is told about — that distinction is a
  finding, not a field name);
- economy: `bounty`, `securityStatus`, `spawnDistanceMeters`,
  `shipNameTemplate`;
- composition: `spawnPool → [{profileID, weight}]` and
  `spawnGroup → [{profileID|spawnPoolID, count}]`, a two-level
  weight-then-count split;
- placement: `anchorSelector{kind: "stargate", mode: "each",
  distanceFromSurfaceMeters, spreadMeters, formationSpacingMeters}` plus
  `respawnEnabled`/`respawnDelayMs`.

**Three of their field choices are worth adopting outright** [I]: the
`presentationTypeID`/`shipTypeID` split; `behaviorOverrides` on a startup rule
(so one profile can be reused passively at a gate and aggressively in a
dungeon — their Jita CONCORD rule sets `autoAggro: false`,
`targetPreference: "none"`, `allowPodKill: false`); and the `aliases` array on
every profile, pool and group, which exists so a GM can type
`"blood pair"`. Our GM tooling will want the same.

#### The correction: most of this is in the client, per type

**[V]** `clientEntityStandings` is their 7,343-row re-export of
`spacecomponentsbytype.entityStandings` — and `findings/pyd-schemas.tsv` shows
that struct is one of *four* NPC-relevant components on that same 1,832-byte
record, which our reader resolves field by field:

**[V] Measured for this document**, by reading the presence bits of all
11,238 rows of our own `spacecomponentsbytype.fsdbinary` (script in the
session scratchpad; `fsd.CfsdTable` + the offsets from
`findings/pyd-schemas.tsv`, which are record-relative and already include
`delta`):

| component | offset | presence | rows carrying it | what it decides |
|---|---:|---|---:|---|
| `entityStandings{friendlyResponseThreshold, hostileResponseThreshold}` | 1404 | `+1820 & 0x1` | **7,427** | will it shoot me |
| `behavior{behaviorName, groupBehaviorID}` | 64 | `+1816 & 0x100` | **2,419** | which behaviour tree it runs |
| `reactiveSpawnComponent` (4 strings, 2 dicts, 18 numbers, 3 flags) | 696 | `+1824 & 0x40` | **882** | when it respawns, how far it notices you, what it pays |
| `npcpilot{characterIds}` | 536 | `+1820 & 0x2000000` | 48 | which NPC character flies it |
| `npcWarpBeacon{warpToRange}` | 1608 | `+1820 & 0x1000000` | 6 | warp-in distance |

**The offsets are cross-validated against EvEJS itself.** **[V]** Their
`clientEntityStandings.counts` reports
`uniqueFriendlyThresholdCount: 6, uniqueHostileThresholdCount: 6` over 7,343
types at build 3396210. Reading the same two `f32` fields out of our 3619879
file gives **6 distinct friendly and 6 distinct hostile values** over **7,427**
types — same cardinality, +84 rows, which is the build delta and not a decode
error. Two independent readings of two different builds of the same column
agree.

**And the values are CCP's own game design, not placeholders.** **[V]** Of the
882 spawner types: `(minSpawnIntervalSeconds, maxSpawnIntervalSeconds)` takes
81 distinct pairs (2 h flat, 900–1800 s, 7–15 s, 24 h flat, …);
`threatRangeMeters` takes **9 distinct values** and the commonest is
**250 km on 681 of 882**; `initialPointAmount` takes 84 distinct values.
**So respawn timing and NPC threat range — two of the five things
`docs/archive/roadmap.md` lists as "not shipped" — are shipped, per type, and we can
read them today.** That is a correction to our own roadmap, not to EvEJS.

**What is genuinely missing is the tables those components point into.**
**[V]** Over the 882 spawner rows, `spawnTableID` takes **504 distinct
non-zero values in 40..1252** (zero on 93) and `initialSpawnNpcFleetTypeID`
takes **71 distinct non-zero values in 11..3536** (zero on 278);
`fleetCounterTableID` takes 82 in 1..232. **[V]** No file under
`res:/staticdata/` is a spawn table, an NPC fleet type table or a fleet
counter table — checked against all 203 names. **[V]** And joined, not just
name-matched: `trackedTypeListID` closes **20 of 20** into
`typelist.fsdbinary`, which is the control that proves the method, while
`spawnTableID` closes at best 243/504 into `nodegraphs` and
`initialSpawnNpcFleetTypeID` at best 19/71 into `operations` — coincidence
over dense small-integer ranges, not a key. Full table in §8 Q9. **[I]**
`static-corpus.md` §3.1 independently lists `spawnTableID`, `npcFleetTypeID`
and `groupBehaviorTreeID` among the FSD schema's foreign-key *type names* —
names whose whole purpose is to say what an integer references — with nothing
to reference. **504 spawn tables are named by id and none is shipped. That is
the gap, stated as precisely as static reading can state it.**

**One caveat, and it cuts against the good news.** **[V]** The `behavior`
component is present on 2,419 types but **blank**: `groupBehaviorID` is `0` on
**2,411** of them and only ~8 rows carry a real id. So the client knows an NPC
*has* a behaviour tree and does not say which — the AI selection is entirely
server-side, exactly as EvEJS's 5,639 hand-written `npcBehaviorProfiles`
assume. **[A]** that the empty `behaviorName` strings are genuinely empty and
not a misread of the variable-length arena; falsified by dumping those 8
non-zero rows with `fsd_dump.py raw` and checking the string pointer — worth
doing, because if `behaviorName` *is* populated it is CCP's own AI vocabulary.

> **How we would do it, revised.** Read
> `spacecomponentsbytype.reactiveSpawnComponent` for the 882 spawner types and
> use CCP's own numbers for interval, point budget, threat range and timeout,
> sensor drift and bounty multiplier. Read `entityStandings` for all 7,427
> types for "will it shoot me". Then author **only** the three missing tables
> — a spawn table (`spawnTableID` → a weighted list of types), an NPC fleet
> type (`npcFleetTypeID` → a composition) and a fleet counter table — plus a
> behaviour tree per NPC class, against EvEJS's
> `npcSpawnPools`/`npcSpawnGroups` two-level weight-then-count split and their
> fourteen-field behaviour vocabulary, which is what those tables have to
> produce. **The 5,620 `npcProfiles` rows become a join, not a table:**
> `shipTypeID` + that type's own `entityStandings`/`reactiveSpawnComponent` +
> a behaviour tree id is the profile. Only the last term has to be invented.

### 4.2 The market, and the LP store — rank 2

Three separate artefacts, three different answers.

**(a) The order book is authored, and theirs is reproducible from our own
files.** **[V]** `market-seederv3` computes a price for every T1 type
**bottom-up through its own blueprint tree**, sells at 3× and buys at 1× at
Jita 4-4 only, from a committed manifest: 2,308 of 4,771 prices are
`cost-recursive` and need **no network at all**. The recursion needs
`blueprints.static` (ours, 5,100 rows, classic FSD, fully readable) and
`typematerials.fsdbinary` (ours, 9,866 rows) and nothing else. The four rules
that make it not-exploitable are the real content and all four are
reproducible: reject ESI `adjusted_price` as a price (it is CCP's industry
index, not a trade); give both halves of a compression pair the same per-unit
price; floor a type's ask at what the seed's own bids pay for its reprocessing
outputs, iterated to a fixpoint; and resolve a leaf's cost capture-first so
building from seed-bought inputs cannot profit. **[V]** They measured what
happens without the last rule: *"7 loops of ours-sell → TQ-buys (worst
×165.7, Element Containment Vessel) and 5 of TQ-sells → ours-buys (worst
×8.41, Plutonium)."*

> **How we would do it:** derive the cost map from `blueprints.static` +
> `typematerials.fsdbinary`, seed both sides at one station, and port the four
> anti-arbitrage rules as rules. We need no snapshot, no ESI and no EVE Ref.

**(b) Their *other* two seeders are the ones not to copy.** `market-seed` (v1)
seeds from SDE `basePrice` universe-wide; `market-seederv2` imports a real
Tranquility order snapshot. **[V]** v3's own README calls the snapshot path
*"the TQ NPC catalog overlay … imported from a cached Tranquility order
snapshot"* from `https://data.everef.net/market-orders/index.json`, and
`order_filter = "npc_only"` with `npc_order_duration_threshold_days = 90` —
i.e. **an NPC order is identified by a duration over 90 days**, which is a
genuinely useful protocol fact [V] and the only thing worth taking from that
path. Importing a real TQ order book is both unnecessary and a third-party
dependency with a shelf life.

**(c) LP store offers are the one content catalogue with no client source and
a clean public one.** **[V]** `npccorporations.fsdbinary` carries
`lpOfferTables: list<i32>` — pointers — and **no `res:/staticdata/` file those
ids resolve into** (checked against all 203 names). Their 9.4 MB
`lpStoreStaticOffers.json` came from CCP's own ESI
(`/loyalty/stores/{corporation_id}/offers/`), 178 corporations, with etags.
**That endpoint is CCP's, public, and documented.** If LP rewards matter for
missions, fetch it ourselves and cite CCP.

**Also from §3.3, cheaply:** the `market_orders` column list in §5.2 is the
market's whole state model and costs nothing to adopt.

### 4.3 Wormhole statics — rank 3

**[V]** `explorationWormholeStatic` assigns each of 8,490 systems its static
wormhole slots: 2,490 systems carry at least one, 3,015 slots in all, keyed
`"<solarSystemID>:<slotIndex>:<code>"` and carrying
`{code, typeID, typeName, targetClassID, lifetimeMinutes, maxStableMass,
massRegeneration, maxJumpMass, distributionID, radius, graphicID}`. Their
source is `scripts/DataSync/spec/explorationWormholeStatics.json` — a spec
file **not in the archive**. So the table is shipped and the authoring input
is not.

**Split it in two.** The 99 **code types** (A009, Z060, K162, …) are
`types.fsdbinary` rows whose class/lifetime/mass/jump-mass are dogma
attributes, so that half is ours already [I] — the attribute ids are
findable in `findings/corpus-dogmaattributes.tsv`. The **per-system
assignment** is not in the client and is not in the SDE either: in real EVE a
system's statics are a property of the shard, discovered by players. The 42
`wanderingProfiles` (keyed `"<recipeKey>:c<sourceClass>:<code>"`, e.g. a
small-ship w-space-and-null recipe) are pure design.

> **How we would do it:** generate. `wormholeClassID` is on every system in
> `solarsystemcontent.static` **[V]**, and a static assignment is a
> seeded draw from "systems of class N get one static to class M with
> distribution D". That is exactly the shape their `wanderingProfiles` already
> encodes, and a seeded generator gives us a stable universe without
> importing anyone's table. **Do not port the 3,015 rows** — they are one
> shard's layout, ours can be our own, and friends-with-bots does not need
> Tranquility's wormhole map.

### 4.4 Per-station dock and undock geometry — rank 4

**[V]** `stationDockingPlacements` is 5,210 rows (≈ one per NPC station) of
`[stationID, stationTypeID, dockEntry, dockPosition, dockOrientation,
undockDirection, undockPosition]`, where `dockEntry` is a small station-local
offset (metres) and `dockPosition`/`undockPosition` are absolute system
coordinates (1e11–1e12 metres). Their `source` calls it *"EvEJS reverse
engineered client station placement data"*, extracted *"before generated data
purge"* — so even they no longer have the extractor.

**This one is derivable, and the derivation is short.** **[I]** The station's
absolute position is in `solarsystemcontent.static` (per-system `planets` dict
→ NPC stations, which our reader decodes). The type-local dock entry and
orientation are in `graphiclocations.fsdbinary` (68 rows, one per station
type, and the `stationGraphicLocators`/`structureGraphicLocators` tables in
§3.1 are exactly that file re-exported). So:

```
dockPosition(station)   = position(station) + R(orientation(station)) · dockEntry(stationType)
undockDirection(station) = R(orientation(station)) · undockDirection(stationType)
```

and the rotation is the station's own, which the client already knows because
it draws the model. **This matters because `docs/lanes/undock-points.md`
exists** — the undock point is on the backlog, and the answer is a join of two
files we can already read rather than 5,210 rows of someone else's extract.
**[A]** that `graphiclocations` carries a usable dock-entry locator per
station type; falsified if its 68 rows turn out to be keyed on something other
than station type, in which case the row shape above still tells us exactly
what the server has to send.

### 4.5 Moon composition and moon mining points — rank 5

Two artefacts, and the headline is that **EvEJS did not datamine either**.

**[V]** `tools/moon-data/generate-moon-composition-pack.js` *generates* moon
ore composition from a fixed string seed (`DEFAULT_SEED = "EveJS"`), with the
comment *"An install keeps the same seed for good: a new seed gives every moon
new ores."* **[V]** `iceSystemSyntheticNullsecAuthority.json` is self-labelled
*"Emulator-owned synthetic"*, built from DOTLAN scrapes with *"best-fit
templates"* where DOTLAN had nothing.

> **How we would do it: the same way, with our own seed.** Moon composition is
> per-shard in real EVE and nobody outside CCP has Tranquility's. A seeded
> generator keyed on `moonID` with weights by region and security is the
> correct answer and costs a function.

`moonMiningPoints` is different: 157,905
`{moonID, solarSystemID, beaconID, position, distanceFromMoon}` rows — an
anchor point per moon, not a composition. **[I]** Those are derivable from the
moon positions in `solarsystemcontent.static` plus the recorded
`distanceFromMoon`, which is the only free parameter and is a constant per
moon class in their data. **[A]** that the client does not validate the anchor
position against anything; falsified if the client refuses a Skyhook/drill
anchored off-locator, which is a question for `evejs-probe` (§9).

### 4.6 Insurance prices — rank 6, and already solved

**[V]** `shipInsurancePrices._meta.formulas` is
`{baseInsuranceFraction: 0.4, insuranceFractionIncrement: 0.5}` over six
levels (Basic, Standard, Bronze, Silver, Gold, Platinum) with per-level
`fraction`, `costCents` and `payoutCents`. **[I]** Combined with
`basePrice` from `types.fsdbinary` (ours, 53,288 rows) that is a closed-form
table: payout is a fraction of the hull's base price and cost is a fraction of
the payout, stepping by level. **We need the two constants, not the 473 rows.**
Their rows came from `https://esi.evetech.net/latest/insurance/prices/`, which
is CCP's and public, so the rows are also a legitimate cross-check if the
formula does not reproduce them. **[A]** the formula reproduces CCP's table;
falsified by recomputing 473 payouts from `basePrice` and comparing — and
*that check is a 20-line script against a public endpoint*, which makes it the
cheapest open question in this document.

### 4.7 New Eden Store offers — rank 7

**[V]** 46 observed offers with `plexPriceInCents`, `fulfillment{kind,
durationDays}` and a `source{kind, observedAt, url}` on every row. Not on our
backlog, and the shape (`storeOfferID` as a *string* key, a `preview` block of
pure presentation, `fastCheckout` as a separate catalogue) is the only part
worth keeping if the NES window ever needs to not-hang. Author 2 offers, not
46.

### 4.8 Trig/Drifter system lists — rank 8

**[V]** 1,026 Jove Observatory systems and 1,022 unidentified-wormhole systems
from `https://jambeeno.com/jo.txt` and *"repo-owned static names"*. **[I]**
Not in our corpus: `landmarks.static` has 45 rows and
`factionsowningsolarsystems.static` has 22. These are player-discovered lists
of one shard's content. **Generate or skip** — a Jove Observatory is a beacon
in a system, and which systems have one is ours to decide.

### 4.9 Agent portraits — rank 9

**[V]** 65,274 portraits across 4 sizes, manifested against
`https://images.evetech.net/`. The client fetches portraits from that service
in retail; offline it cannot. **[V]** Their fallback is
`client-conversation-agents.json` — the client's own
`conversationagents.fsdbinary` (44 rows) and
`character_portraitresources.fsdbinary`, both ours. So the floor is 44 real
portraits plus procedural fallbacks for the other 11,380 agents; the ceiling
needs CCP's image service, which is public but online. **A blank portrait that
does not hang the window is the requirement**, and that is a protocol question
(§9), not a data question.

### 4.10 Static chat channels — rank 10, and nearly free

**[V]** Their help-channel room names are `system_<int>_<int>` —
`system_263238_263262` for English, `_263267` German, `_263301` Russian,
`_263302` Japanese, `_504075` French — with `contractSource:
"codeccpfull_v23.02"`. **[I]** Both integers are localisation message ids:
263238 is the shared "Help Channels" stem and the second is the per-language
name. `tools/loctable.py` resolves both out of our own localisation pickle, so
**the whole static-channel catalogue is a two-id-per-channel derivation from a
file we already read.** For the chat backlog item, this is the cheapest
correct answer available anywhere in the archive.

---

## 5. Their schema as a design document

### 5.1 The 77 empty tables are a list of state we have no place for

Their table names tell you what a working server decided it had to store. Set
against `Registry` (`service.rs:541`), which today holds **thirteen fields**
— `config`, `character_description`, `pending`, `last_destiny_stamp`,
`roster`, `store`, `journal`, `skills`, `missions`, `sessions`, `fleets`,
`planet`, `jobs`, `dirty` — the shortfall sorts into groups, and the groups are
the finding, not the names:

| their tables | the state category | do we have a place for it |
|---|---|---|
| `characters`, `accounts`, `identityState`, `accessGroups` | identity, separate from the character row | partly — `session_table` has presence, `Character` has the row; `identityState` as its own table is the one we lack |
| `items`, `npcCargo`, `npcWrecks`, `npcWreckItems` | **four id spaces for inventory**, with reserved ranges (`98020…`, `98030…`, `98040…`) | **no** — `item::Store` is one space. Transient NPC-owned items in their own range is a choice we have not made |
| `npcEntities`, `npcModules`, `npcRuntimeState`, `npcControlState`, `npcRuntimeControllers`, `npcSpawnSites`, `pendingNpcBounties` | **seven** tables for live NPCs: the entity, its fitted modules, its AI state, its control arbitration, its spawn site, and the bounty it owes | **no** — `roster` holds players in space and nothing else |
| `killmails`, `killRights`, `playerBounties`, `shipKillCounters`, `insuranceContracts` | the consequences of death | **no** |
| `marketRuntime`, `marketEscrow` | orders, and **escrow as its own table** | **no** |
| `sovereignty`, `solarSystemInterferenceState`, `mapTelemetry` | who owns what, and who has been where | **no** |
| `structures`, `structureProfiles`, `structureAssetSafety`, `structureTetherRestrictions`, `structurePaintwork` | player structures and the five separate concerns they carry | **no** |
| `bookmarks`, `bookmarkFolders`, `bookmarkSubfolders`, `bookmarkGroups`, `bookmarkKnownFolders`, `bookmarkRuntimeState`, `sharedBookmarkFolders` | **seven** tables for bookmarks | **no**, and seven is a warning about the client's bookmark surface |
| `missionRuntimeState`, `dungeonRuntimeState`, `probeRuntimeState`, `wormholeRuntimeState`, `miningRuntimeState`, `planetRuntimeState`, `industryRuntime`, `reprocessingFacilityState`, `moonExtractions` | **"runtime state" as a first-class, persisted table per subsystem** | **this is the architectural finding** — see below |
| `savedFittings`, `skillPlans`, `skillQueues`, `sharedSettings`, `overviewSharedPresets`, `moduleGroupingState`, `shipCosmetics`, `shipDirt`, `shipLogoFittings`, `characterNotes`, `calendarEvents`, `calendarResponses`, `mail`, `notifications` | per-character client-side-looking state the **server** must keep | partly — `character_description` is our only example of this class, and it exists because the client re-reads it on every character-sheet open |

**The architectural finding.** Their persistence splits every subsystem in two:
an **authority** table (static, built at init, read-only at runtime) and a
**`*RuntimeState`** table (mutable, persisted, one blob per subsystem with
`ROW_GROUPS` naming the shardable sub-collections). Our `Registry` has no such
split: static tables are not in it at all (they are read from `ResFiles` on
demand) and mutable state is thirteen typed fields. **That is the better
design for thirteen fields and it does not obviously survive seventy-seven.**
The specific pressure point is `docs/reference/world-state.md`'s rule that conversations
stay out of the snapshot: `missions` is already keyed per character and *not*
persisted on exactly that reasoning, and their `missionRuntimeState`
(`charactersByID`) is the same data persisted. One of us is wrong about
whether an accepted mission survives a restart, and it is worth deciding
deliberately rather than by default. **[I]**

Two smaller observations with no analogue on our side:

- **[V]** `tableOwnership.js` + `_persistence_owners` + a **lease** policy
  (`persistenceLeasePolicy.js`, `persistenceRoles.js`) means more than one
  process writes the store and they arbitrate by lease. We are one process and
  should stay that way; the row exists so that the reason is on the record.
- **[V]** `authoredSpaceProps/*.json` is the cleanest example in the archive
  of **what a non-ship object on a grid needs**:
  `{itemID, typeID, groupID, categoryID, anchorItemID, ownerID, mode, radius,
  flagsByte, position, dunPosition, dunRotation, dungeonObjectID,
  dunObjectNameID, nameID, itemName, slimName, forceDamageState,
  staticVisibilityScope}`. `mode: "STOP"`, `flagsByte`, `slimName` and
  `staticVisibilityScope: "publicgrid"` are all `AddBalls`-shaped, and
  `docs/lanes/addballs.md` is the file that should read that list. One sample
  row's worth of shape is quoted here and no more.

### 5.2 The market schema is the one piece of SQL worth reading

**[V]** `externalservices/market-server/crates/market-common/src/lib.rs`
holds the archive's only real DDL: 11 tables + indices. `market_orders` is the
column list a market needs and we have none of it:

```
order_id, owner_id, is_corp, wallet_division, station_id, solar_system_id,
constellation_id, region_id, type_id, price, vol_entered, vol_remaining,
min_volume, bid, range_value, duration_days, escrow, state, source,
issued_at, last_state_change_at, updated_at
```

Three choices in there are worth taking [I]: **denormalised location** (all
four of station/system/constellation/region on every order, because every
client query is "best price within range" and range is a location predicate);
**`source` as a column** (`'player'` vs seeded, so the seeder's rows and a
player's rows live in one book and can be told apart); and
**`market_order_events` as an append-only mirror of every column** rather than
a diff, which makes the order book's history reconstructible without joins.
`seed_stock` and `seed_buy_orders` are separate from `market_orders` and keyed
`(station_id, type_id)` — infinite-quantity NPC seeding is *not* an order, it
is a price at a place. `region_summaries` and `system_seed_summaries`
precompute best-ask/best-bid per `(region|system, type)`, which is the shape
the client's market tree actually asks for (`docs/lanes/market-tree.md`).

---

## 6. The build delta's effect on the data

**Their target is client build 3396210. Ours is 3619879.** Same version
(V24.01), different build. The content tables are keyed by type ids, dungeon
ids, mission ids and attribute ids, and **the delta is measurable in every one
of them**:

| table | theirs | ours | delta |
|---|---:|---:|---:|
| missions | 2,878 (`missionAuthority`) | **2,892** (`missions.fsdbinary`) | **+14** |
| client dungeons | 5,393 (`dungeonAuthority.counts.clientDungeonCount`) | **5,516** (`dungeons.fsdbinary`) | **+123** |
| npc characters / agents | 10,941 (SDE snapshot **3294658**) | **11,424** (`npccharacters.fsdbinary`) | **+483** |
| space components by type | 11,046 (`clientTowComponents.source.sourceTypeCount`) | **11,238** (`spacecomponentsbytype.fsdbinary`) | **+192** |
| entity-standing types | 7,343 (`clientEntityStandings`) | 11,238 rows available to filter | — |

**What that invalidates, concretely:**

1. **Any table keyed by a dungeon or mission id is short, not wrong.** Their
   5,009 templates cover 5,393 of our 5,516 dungeons; the 123 we have extra
   would simply have no template. Porting loses content silently.
2. **Three tables were built against builds *newer* than their own target and
   they knew it.** **[V]** `shipSkinComponentAuthority` carries 124
   `excludedPhysicalTypeIDs` and 2 `ambiguousPhysicalTypeIDs` — the type ids
   build 3487903 has that 3396210 does not. `dungeonClientContent` and the
   content packs are from **3475087**; `evejs-homefront` is from **3503375**;
   the AIR tutorial's donor graph is **3487903**. Our 3619879 is *newer than
   all of them*, so a port would be a downgrade in content and an upgrade in
   exclusion lists we do not need.
3. **Attribute ids carry over and should still be re-read.** Their
   `ATTRIBUTE_IDS` (mass 4, maxVelocity 37, capacity 38, radius 162,
   inertia 70, signatureRadius 552, warpSpeedMultiplier 600) and
   `REPROCESSING_ATTRIBUTE_IDS` (717, 790, 1547, 2355–2357, 2711, 2722, 3261,
   3262) are stable CCP ids and **agree with our `findings/corpus-dogmaattributes.tsv`
   by construction** — but they are a lookup for us, not a constant. Our
   `dogmaattributes.fsdbinary` has 2,871 rows and is authoritative.
4. **Service names are the one place the delta bites hardest, and it is not
   this lane's.** `docs/re/evejs-crosscheck.md` already struck
   `machoNet.GetInitVals` as nonexistent in V24.01. Nothing in *this* document
   depends on a service name.
5. **The SDE build number is not the client build number.** **[V]** Their
   `agentAuthority` was built from SDE **3294658** while their client is
   3396210 and `evermarksCatalog` used **3396210** — three different numbers in
   one archive. Any claim of the form "their data matches their client" is
   false as stated.

---

## 7. The honest do-not-bother list

**Rows where the client already ships it (do not port, decode):** every row in
§3.1 marked provenance **client** — `missionAuthority`, `dungeonAuthority`'s
client half, `dungeonClientContent`, `asteroidTypesBySolarSystemID`,
`clientEntityStandings`, `clientTowComponents`, `clientMicroJumpComponents`,
`clientShipLinkComponents`, `clientPvpFilaments`, `expertSystems`,
`fighterAbilities`, `reprocessingClientRandomizedMaterials`,
`station`/`structureGraphicLocators`, `stationStandingsRestrictions`,
`shipSkinComponentAuthority`, `explorationAuthority`'s probe half — plus
**all 19 content packs' payloads** (331 of 339 MB) and
`achievement/definitions.json`. Our reader is `tools/fsd.py` +
`tools/pyd_schema.py` and our build is newer.

**The two sharpest cases, because they look like gaps and are not:**

- **`skillTrainingAlphaCaps`.** They scraped
  `wiki.eveuniversity.org/Clone_states?action=raw`. **[V]** The client ships
  `res:/staticdata/clonegrades.static` — a 4-row fsdlite SQLite with a
  `skills` column, fully readable by `tools/fsd.py` today. The authoritative
  answer was in `ResFiles` and they went to a wiki.
- **`missionAuthority`.** Their rows null out `missionRewards` on 1,883 of
  2,878 missions, `agentTypeID` on 2,644, `corporationID` on 2,801,
  `nodeGraphID` on 2,755 and `clientObjectives` on 2,695. **[V]**
  `findings/pyd-schemas.tsv` has all five of those as named, typed,
  offset-resolved columns on our `missions.fsdbinary`, including the nested
  `struct:missionRewards{bonusReward{rewardTypeID,rewardQuantity},
  bonusTimeInterval, reward{rewardTypeID,rewardQuantity}}` and
  `struct:killMission{dungeonID, objectiveTypeID, objectiveQuantity,
  objectiveGroupID, dropItemInMissionContainer}`. **We can read more of their
  own mission table than their shipped export contains.**

**Rows that are SDE data we can fetch ourselves:** the 36 `GENERATED_TABLES`.
Except we mostly should not even do that — `types.fsdbinary` (53,288),
`typedogma.fsdbinary` (27,160), `groups`, `categories`, `blueprints.static`
(5,100), `typematerials.fsdbinary` (9,866), `solarsystemcontent.static`
(8,490 systems with planets, moons, belts, stations, stargates and
destinations), `systems`/`regions`/`constellations`/`jumps`+`.schema` are all
in `ResFiles` and all readable. **The SDE's only unique contribution over our
own `ResFiles` is `planetResources.jsonl` and `sovereigntyUpgrades.jsonl`**
[I] — and `landmarks.static`, `factionsowningsolarsystems.static` and
`starmapcache.pickle` cover part of even that.

**Rows where their version is build-wrong:** §6.2's three donor-build tables
and the entire content-pack folder.

**Rows that are observed player or retail content and should not be in our
tree on principle:** `shipSkinStoreAuthority` (1,674 designs harvested from
TQ client logs, `captureReachedEnd: false`), the abyssal telemetry tables
(Abyssal.Space / QSNA scrapes), `evermarksCatalog` (fuzzwork), and
`iceSystemSyntheticNullsecAuthority` (DOTLAN). Each is someone else's
observation of Tranquility. Our position is a server of our own, and these are
the rows that make a tree look like a mirror of a live game.

---

## 8. Open questions, ranked by what they cost us if wrong

**Q1 — Is the insurance formula right? (cost: half a day, and it is also the
cheapest to settle.)** If `payout = basePrice × fraction(level)` with
`baseInsuranceFraction 0.4` and `insuranceFractionIncrement 0.5` reproduces
their 473 ESI-captured rows, insurance is closed-form and `shipInsurancePrices`
drops off every list. If it does not, we need the endpoint, and we will have
spent 20 lines finding out. **Falsifier:** recompute from `types.fsdbinary`
`basePrice` and diff against CCP's public `/insurance/prices/`.

**Q2 — Is there a CCP SDE for build 3619879, and do we care? (cost: a day of
someone building an SDE importer we do not need.)** **[V]** The 3396210 URL is
live and the 3619879 one 403s. The *right* answer is probably that we never
need the SDE at all, because `ResFiles` is a superset for everything except
`planetResources` and `sovereigntyUpgrades`. **Falsifier:** enumerate the 33
`.jsonl` names `loadSdeAuthority` reads and check each against
`findings/corpus-inventory.tsv`; if the residue is really two files, close the
question for good. If it is ten, the importer is worth writing.

**Q3 — Does the client validate a dungeon or anchor position against anything
it already knows? (cost: the whole §4.5 "generate it" answer.)** If the client
accepts any position the server sends, moon composition, moon anchor points,
wormhole statics and asteroid field envelopes are all ours to generate and the
gap in §4 shrinks to NPC behaviour and the market. If the client cross-checks
against `solarsystemcontent.static` or a dungeon's own room geometry, every
generated layout has to agree with a shipped table and the cost of each §4 row
goes up by an order of magnitude. **This is the question §9 exists for.**

**Q4 — Is `missionRuntimeState` right and our non-persisted `missions` wrong?
(cost: one player relaunch that silently forgets an accepted mission.)**
`docs/reference/world-state.md` §2 keeps conversations out of the snapshot and
`Registry::missions` follows it. They persist it. **Falsifier:** accept a
mission, restart, and see whether the client's journal still lists it from its
own cache — if it does, our snapshot has to agree with the cache or the player
sees a mission the server has never heard of.

**Q5 — Do the 68 `graphiclocations.fsdbinary` rows actually key on station
type? (cost: §4.4's derivation, and with it the undock point.)** 5,210
stations over 68 locator rows is a clean many-to-one if the key is
`stationTypeID` and a dead end if it is not. **Falsifier:** read the table's
key column with `tools/fsd_dump.py` and join it against
`stationTypes`/`types.fsdbinary`.

**Q6 — Where do `npccorporations.lpOfferTables` ids point? (cost: the LP
store, and an unnecessary ESI dependency.)** **[V]** No `res:/staticdata/`
file name matches, over all 203. **[A]** they are server-side table ids with no
client counterpart; falsified if an `lpOfferTables` value turns out to index
`typelist.fsdbinary` or `clientlocationlists.fsdbinary`, in which case the LP
catalogue is partly client-shipped after all and §4.2(c) is wrong.

**Q7 — Is the `presentationTypeID`/`shipTypeID` split real or theirs?
(cost: an NPC that renders as the wrong hull, found late.)** **[I]** It reads
like a protocol requirement — the hull the client is told about may differ from
the hull the server simulates — but nothing in our corpus confirms the client
needs two. **Falsifier:** `evejs-probe`, §9.

**Q8 — Is `behaviorName` on the 2,419 `behavior`-carrying types really empty?
(cost: re-inventing CCP's AI vocabulary when it is in the file.)** **[V]**
`groupBehaviorID` is `0` on 2,411 of 2,419, which is a plain `u32` read and
trustworthy. The `behaviorName` strings read as empty, which could instead be
a misread of the variable-length arena. **Falsifier:**
`tools/fsd_dump.py raw spacecomponentsbytype` on the ~8 rows with a non-zero
`groupBehaviorID` and resolve the slot at record offset 64 as a string
pointer. If the names are real, §4.1's "author a behaviour tree per NPC class"
becomes "read CCP's class names first", and that is a different and better
piece of work.

**Q9 — ~~Do the 504 `spawnTableID` values collide with anything we can already
read?~~ Asked and answered: no.** Left on the list because the *method* is
reusable and because a future reader will want to ask it again. **[V]** The
four foreign keys were joined against every plausible client table:

| foreign key | distinct | `typelist` | `nodegraphs` | `crates` | `globaltypes` | `operations` |
|---|---:|---:|---:|---:|---:|---:|
| `trackedTypeListID` | 20 | **20/20** | 12 | 14 | 0 | 0 |
| `spawnTableID` | 504 | 204 | 243 | 194 | 0 | 0 |
| `initialSpawnNpcFleetTypeID` | 71 | 4 | 7 | 9 | 3 | 19 |
| `fleetCounterTableID` | 82 | 30 | 43 | 41 | 0 | 0 |

`trackedTypeListID` closing **100% into `typelist.fsdbinary`** is the control:
it proves the offsets, the presence bits and the join method are all right. On
the same method the other three close at 40%, 27% and 52% at best, which over
dense small-integer ranges is coincidence, not a key. **So §4.1's three
missing tables are missing. The question is closed and the scope does not
shrink.**

---

## 9. What `evejs-probe` should look for that static reading cannot settle

Our two documents are meant to meet. Everything below is a question about what
goes **on the wire**, which is theirs, not mine.

1. **Does an `AddBalls`/`AddBall` row for a non-ship object carry the
   `authoredSpaceProps` fields?** §5.1's list — `mode`, `flagsByte`,
   `slimName`, `staticVisibilityScope`, `dunPosition`, `dunRotation`,
   `anchorItemID`, `forceDamageState`, `dungeonObjectID` — is a *storage*
   shape. Which of those nine actually appear in the packet, under what names,
   decides whether `docs/lanes/addballs.md` is finished or has nine holes.
2. **Does the client validate positions?** Q3 above. Anchor a prop or a rock
   at a position that disagrees with `solarsystemcontent.static` and see
   whether the client draws it, moves it, or complains. This single
   observation prices four of my §4 rows.
3. **Is `presentationTypeID` ever different from the simulated hull in a real
   `AddBalls` row?** Q7.
4. **What does the agent window do with a placeholder portrait?** All 10,941
   of their agents ship `conversationMetadata.placeholder: true` and their
   portrait path is an online image service. Does a missing portrait render
   blank, render a default, or hang the dialogue window (`§4.9`)?
5. **How does the client ask for an LP store, and does it accept an empty
   catalogue?** `shipSkinComponentListingAuthority` is *deliberately* empty
   with the note *"Empty means the client receives a valid empty component
   listing catalogue"* — so they found at least one catalogue where empty is a
   valid answer. Whether the LP store is another one decides whether §4.2(c)
   is urgent or optional.
6. **Does a market order's `range_value` default of `32767` appear on the
   wire, and what does the client do with it?** §5.2's column list is a
   storage schema; `range_value` is the one column whose meaning is a client
   behaviour (order range), and `32767` reads like a sentinel for "region".
7. **Is an NPC order distinguishable on the wire the way their importer
   assumes?** Their filter is *duration > 90 days*. If the client renders NPC
   and player orders identically, that threshold is an import heuristic and
   nothing more; if the client treats them differently, there is a flag we
   have not found.
8. **Does the client ever ask for a mission's rewards, or only render what the
   offer packet carries?** Our `missions.fsdbinary` has the full
   `missionRewards` struct, so if the client reads its own table we need only
   send the mission id; if the offer packet carries the rewards, the server has
   to read that struct and marshal it, and §7's "do not port" becomes "decode,
   and decode this struct first."
