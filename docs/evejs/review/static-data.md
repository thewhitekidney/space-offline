# Lane: static / datamined game data in the local EvEJS checkout

Repo: `$EVEJS` (AGPL-3.0). **Facts only** below — field names,
sizes, counts, numeric constants, and the rules the data encodes. No source code copied.
All paths relative to that repo root unless absolute.

---

## 0. Headline finding: the celestial universe is NOT in the repo

**There is no committed solar-system / celestial / station / stargate table anywhere in the
repo.** Confirmed absence:

- `git ls-files | grep -iE "solarsystem|celestial|stargate|mapdenorm|invnames"` returns only
  *code* files plus two small static tables (`asteroidTypesBySolarSystemID`,
  `stargateVisualOverrides`). No `mapSolarSystems`, no `celestials/data.json`, no SDE dump.
- `.gitignore:40-41` explicitly ignores the generated output:
  `/server/src/gameStore/data/*/data.json`.
- The universe is **downloaded and converted at install time** from CCP's public static data.

### The pipeline

`tools/DatabaseCreator/CreateDatabase.bat:6-7` pins the source exactly:

- `SDE_BUILD=3396210`
- `SDE_URL=https://developers.eveonline.com/static-data/tranquility/eve-online-static-data-3396210-jsonl.zip`
- extract to `_local/sde/eve-online-static-data-3396210-jsonl`
- output to `_local/gameStore/data`, manifest `_local/gameStore/manifest.json`
- invoked as `node --max-old-space-size=8192 tools/DatabaseCreator/database-creator.js --sde-dir ... --out ... --build 3396210` (`CreateDatabase.bat:52`)

`tools/DatabaseCreator/EnsureSde.ps1:13-17` enforces the archive name
`eve-online-static-data-<build>-jsonl.zip`; `:68-86` reads `_sde.jsonl`, finds the record with
`_key == "sde"` and asserts `record.buildNumber == 3396210`; `:23` writes a completion marker
`.evejs-sde-complete` containing the build number.

So: **a converted FSD/JSONL dump from CCP, fetched at runtime — not an SDE sqlite, not the
classic YAML SDE, and not hand-built.** It is the newer "static data JSONL" product: one
`.jsonl` file per table, one JSON object per line, primary key in the field `_key`.

### The 35 required SDE JSONL files (`EnsureSde.ps1:24-60`)

```
_sde.jsonl  agentTypes  bloodlines  blueprints  categories  dbuffCollections
dogmaAttributes  dogmaEffects  dynamicItemAttributes  factions  groups  icons
mapAsteroidBelts  mapConstellations  mapMoons  mapPlanets  mapRegions
mapSolarSystems  mapStargates  mapStars  npcCharacters  npcCorporations
npcStations  planetResources  planetSchematics  races  skinLicenses
skinMaterials  skins  sovereigntyUpgrades  stationOperations  typeDogma
typeLists  typeMaterials  types
```
(each asserted present and non-zero-length; `certificates.jsonl` is referenced separately by
`server/src/services/skills/certificates/certificateRuntime.js:36`).

Note build drift in the repo: `3396210` is the server target; some authorities were imported
from donor client builds `3487903`, `3475087`, `3503375`, and two tables still cite
`3294658` (`staticTables/npcStandingsAuthority/data.json:5`, agentAuthority source).

---

## 1. Where data lives, in what format

### A. `tools/DatabaseCreator/staticTables/` — 166 MB, 43 tracked JSON files

42 directories, one `data.json` each (plus `fighterAbilities/authority.json`). All plain JSON,
all committed as-is (`git ls-files` confirms all 43 tracked). These are the **hand-curated /
reverse-engineered tables that the SDE does not contain**; `database-creator.js` copies them
verbatim into the generated DB (`database-creator.js:3263-3279` — `PRESERVED_STATIC_AUTHORITY_TABLES`
and `readStaticTableOverride`).

| size | table | top-level keys | notable counts |
|---|---|---|---|
| 68M | `dungeonAuthority` | version, source, counts, meta, sourcePriorities, sourceConfidence, spawnProfiles, coverage, clientData, templatesByID, indexes | archetypeCount 71, clientDungeonCount 5393, nodeGraphCount 546, oreResourceDungeonCount 1148, gasResourceDungeonCount 83, iceResourceDungeonCount 21, environmentTemplateCount 74, objectiveTypeCount 77 |
| 27M | `moonMiningPoints` | schemaVersion, datasetVersion, sourceBuild, source, count, contentHash, points | **count 157905** |
| 15M | `missionAuthority` | missionsByID, indexes | missionCount 2878, epicArcMissionCount 276, localizedMissionMessageCount 17164 |
| 12M | `shipSkinStoreAuthority` | designsByHex, catalogueOrder | 511 observed designs, 260 new |
| 11M | `agentAuthority` | missionPoolsByKindAndLevel, agentsByID, indexes | agentCount 10941, locatorAgentCount 1757, researchAgentCount 651 |
| 6.9M | `npcBehaviorProfiles` | behaviorProfiles | — |
| 6.5M | `explorationAuthority` | scanContracts, probeDefinitionsByTypeID, probeFormations, wormholeAuthority, … | wormholeSystemCount 8490, wormholeCodeTypeCount 99, probeTypeCount 6, probeLauncherTypeCount 11 |
| 6.1M | `explorationWormholeStatic` | k162Type, codeTypes, wanderingProfiles, systems, systemsByClass | systemCount 8490, codeTypeCount 99, staticSlotCount 3015, systemClassCount 22, wanderingProfileCount 42 |
| 5.4M | `npcProfiles` | profiles | — |
| 4.7M | `dungeonClientContent` | auditsByTemplateID, templatesByID | templateCount 26, sourceObjectCount 4097, roomCount 103, miningRockCount 1094, environmentPropCount 2826, authoredQuantity 3313731.32481 |
| 1.7M | `clientEntityStandings` | typeIDs, hostileThresholds, friendlyThresholds, types | typeCount 7343 |
| 1.1M | `stationDockingPlacements` | source, columns, count, placements | **count 5210** |
| 836K | `npcSpawnPools` | spawnPools | 83 pools |
| 553K | `shipSkinComponentAuthority` | categoriesByID, componentsByID, itemsByTypeID, slotsByID | — |
| 432K | `shipInsurancePrices` | byTypeID | — |
| **429K** | **`asteroidTypesBySolarSystemID`** | source, counts, systems | **systemCount 3974, distinctTypeCount 36, totalAssignmentCount 24071** |
| 395K | `evermarksCatalog` | licensesByTypeID, offersByOfferID | — |
| 391K | `npcStandingsAuthority` | relationsByOwnerID, entries | ownerCount 303, relationCount 978, corporationCount 283, factionCount 27 |
| 346K | `capitalNpcAuthority` | entries, manifestsByProfileID | 18 entries: dreadnought 10, titan 4, supercarrier 4; 6 factions |
| 178K | `expertSystems` | constants, expertSystems | — |
| 142K | `fighterAbilities` | abilities, types, abilitiesByID, fighterTypesByID | abilityCount 36, fighterTypeCount 94 |
| 122K | `clientPvpFilaments` | matchTypes, eventSchedules | — |
| 90K | `newEdenStore` | config, stores, publicOffers, fastCheckout | — |
| 66K | `npcLootTables` | lootTables | 61 tables |
| 55K | `structureGraphicLocators` | locators, _meta | — |
| 52K | `stationGraphicLocators` | locators, _meta | **44 locators** |
| 41K | `npcLoadouts` | loadouts | — |
| 37K | `trigDrifterSpawnAuthority` | exclusions, systemLists | knownSpaceJoveObservatorySystemCount 1026, knownSpaceUnidentifiedWormholeSystemCount 1022, pochvenSystemCount 27, drifterSpaceSystemCount 5 |
| 29K | `shipSkinSequencingAuthority` | thresholdsByGroupID, tierPricing, tierDurations | maxTier |
| 24K | `researchFieldAuthority` | fieldNamesBySkillTypeID, datacoreTypeIDBySkillTypeID, researchFieldsByAgentID | researchAgents 244, researchFields 19, datacoreMappedFields 17 |
| 13K | `reprocessingClientRandomizedMaterials` | types | randomizedTypeCount 10, randomizedEntryCount 24 |
| 12K | `npcHostileUtilities` | templates | — |
| 8K | `npcSpawnGroups` | spawnGroups | — |
| 5.2K | **`stargateVisualOverrides`** | byItemID | **87 gates**, value = `{skinMaterialSetID}` (e.g. 3636) |
| 3.0K | `skillTrainingAlphaCaps` | sourceUrl, capsByTypeID | — |
| 2.2K | `npcStartupRules` | startupRules | 2 rules |
| 1.0K | `stationStandingsRestrictions` | restrictionsByOwnerID | ownerCount 1, ruleCount 7 |
| <1K | `clientMicroJumpComponents`, `clientShipLinkComponents`, `clientTowComponents`, `shipSkinMarketAuthority`, `shipSkinComponentListingAuthority` | | 2 / 3 / 2 types |

Every table carries a provenance block. Provenance **providers** seen, verbatim keys:
`provider`, `authority`, `loader`, `builder`, `exportScript`, `generatedAt`, `sha256`,
`clientFile.resourcePath` / `.hashedPath` / `.resourceHash` / `.sizeBytes`. Provider strings
include `"CCP public static-data JSONL"`, `"EVE Client Static Cache"`, `"ElysianSDE"`,
`"CCP Client Code Snapshot"`, `"Repo DataSync"`, `"EvEJS community moon-mining-point contribution"`,
`"EvEJS reverse engineered client station placement data"`, `"Tranquility client trace"`.

**Important:** many loaders/builders named in provenance are NOT in the repo —
`scripts/DataSync/*`, `scripts/ContentPacks/*`, `tools/ClientDungeonCompiler/*`,
`tools/SignalAtlas/*`, `tools/DataSync/source_json` (`.gitignore:22`), `tools/ClientCodeGrabber/Latest`
(`.gitignore:13`). Those tables are **committed outputs with no reproducible build in-tree**.

### B. `content-packs/` — 339 MB, 18 tracked JSON files

Schema string `"evejs.content-pack.v1"`; every file is `{schema, manifest, payload}`.
`manifest` = `{id, version, title, description, contentApi: 1, engine:{minVersion,maxVersionExclusive},
capabilities[], builtAgainst:{serverSdeBuild, clientDungeonBuild}, counts:{dungeonTemplates, spawnProfiles},
provenance:{builder, clientOverlaySha256, clientOverlayCounts, …}, payloadSha256}`.
`payload.dungeon` = `{templatesByID, spawnProfilesByFamily}`.

| pack | bytes | dungeonTemplates | spawnProfiles | clientDungeonBuild |
|---|---|---|---|---|
| `evejs-ded-rated-sites-1.0.2.json` | 89,764,899 | 50 | 1 | 3475087 |
| `evejs-combat-anomalies-1.4.4.json` | 70,542,604 | 176 | 1 | 3475087 |
| `evejs-pirate-exploration-1.0.0.json` | ~64M | **204** | 2 | 3475087 |
| `evejs-wormhole-sites-1.2.0.json` | ~30M | **48** | 4 | 3475087 |
| `evejs-ore-anomalies-1.1.0.json` | ~30M | **199** | 1 | 3475087 |
| `evejs-booster-production-1.1.0.json` | 21,764,958 | 32 | 2 | 3475087 |
| `evejs-gas-sites-1.0.0.json` | 15,367,573 | 74 | 2 | 3475087 |
| `evejs-homefront-1.0.5.json` | 13,309,352 | 24 | 24 | 3503375 |
| `evejs-drone-anomalies-1.4.0.json` | 8,162,640 | 22 | 1 | 3475087 |
| `evejs-mining-missions-1.0.0.json` | 3,996,293 | 40 | 0 | 3475087 |
| `evejs-ghost-sites-1.1.0.json` | 3,791,046 | 20 | 1 | 3475087 |
| `evejs-air-tutorial-1.0.{0..3}.json` | 109K–129K | 4 | 0 | donorGraph 3487903 |
| `evejs-gallente-depots-1.0.0.json` | 10,549 | 2 | 1 | — |
| `evejs-gallente-fabricators-1.0.1.json` | 5,136 | 1 | 1 | — |
| `evejs-fw-open-1.0.0.json` | 7,891 | 2 | 2 | — |

All built against `serverSdeBuild: 3396210`. Gas site template split
`templateCountByGasKind: {mykoserocin: 16, cytoserocin: …}`.

### C. `server/assets/` — 112 KB total, ONE file

`server/assets/overview/iridium_overview_20260410-v381_carbon.yaml` — 105,383 bytes, 7149 lines.
A client **overview preset pack** (YAML), not universe data. Top-level keys begin
`backgroundOrder: [11,13,52,12,14,15,16,45,44,68,9,51,50,19,18,53,10,49,17,48,66,21,20]`,
`backgroundStates: [9,11,12,…]` — these are state/flag IDs for overview coloring, plus
preset/column definitions further down. **No celestial data in `server/assets/`.**

### D. Other `tools/` data

- `tools/market-seederv3/data/price-manifest.json` — 530,538 B.
  `{formatVersion: 1, sdeBuild: 3396210, costModel: "recursive-v1",
  pricingRules: ["capture-first","reject-adjusted-price-captures","derive-compressed-from-source","floor-cost-at-reprocessing-value"],
  sources: {esi: "markets/prices @ 2026-08-15T20:03:58Z", snapshot: "market-orders-latest.v3.csv.<md5>.bz2 jita-split system 30000142 @ …"},
  prices: {<typeID>: {price, source, capturedAt}}}` — **4781 priced typeIDs**.
  Rows: `18 → 27.39`, `19 → 11882.89`, `20 → 507.53` (source `ccp-esi-average`).
- `tools/moon-data/generate-moon-composition-pack.js` — 8,364 B. Generates a moon-composition
  authority from `moonMiningPoints` + a seeded PRNG. Default seed `"EveJS"`
  (`:13`), env override `EVEJS_MOON_WORLD_SEED` (`:45`), backups kept 3 (`:14`).
  Delegates to `server/src/services/structure/moonCompositionPackGenerator`.
  **Moon ore composition is NOT datamined — it is procedurally generated per install.**
- `tools/market-seed`, `market-seederv2`, `market-seederv3` are Rust (`Cargo.toml`); their
  caches/targets are gitignored.
- Everything else under `tools/` is scripts/installers, no bulk data.

---

## 2. Celestial / solar-system data — exact emitted schemas

Produced by `tools/DatabaseCreator/database-creator.js`. The generator writes **36** generated
tables (`:72-109 GENERATED_TABLES`) into a **153**-entry required set (`:111-265 REQUIRED_TABLES`).
Output is `<DATA_DIR>/<table>/data.json` (`AGENT.md:169`).

### `solarSystems/data.json` — wrapper `{source, count, solarSystems[]}` (`:3117`)
Record built at `:2113-2130` from `mapSolarSystems.jsonl` row (`raw`):

| emitted field | from |
|---|---|
| `regionID` | `raw.regionID` |
| `constellationID` | `raw.constellationID` |
| `solarSystemID` | `raw._key` |
| `solarSystemName` | localized `raw.name`, fallback `"System <key>"` |
| `position` | `{x,y,z}` from `raw.position` |
| `security` | `raw.securityStatus` |
| `factionID` | `raw.factionID` ?? constellation.factionID ?? region.factionID ?? 0 |
| `radius` | `raw.radius` |
| `sunTypeID` | `mapStars[raw.starID].typeID` |
| `securityClass` | `raw.securityClass` (string, e.g. `"B3"`) |
| `visualEffect` | only if present on `raw` |

So `mapSolarSystems.jsonl` rows carry: `_key, name, regionID, constellationID, position{x,y,z},
securityStatus, securityClass, radius, factionID, starID, visualEffect?`.

### `celestials/data.json` — wrapper `{source, count, celestials[]}` (`:3022`)
**One flat array holding suns, planets, moons and asteroid belts**, sorted by `itemID` (`:2843`).
Common field set (`:2653-2748`):

`itemID, typeID, groupID, categoryID, groupName, solarSystemID, constellationID, regionID,
orbitID, position{x,y,z}, radius, itemName, security, celestialIndex, orbitIndex, kind`

`kind` ∈ `"sun" | "planet" | "moon" | "asteroidBelt"`. Default groupIDs when the type lookup
misses: **sun 6, planet 7, moon 8, asteroid belt 9**; default `categoryID` **2** for all;
default groupNames `"Sun"`, `"Planet"`, `"Moon"`, `"Asteroid Belt"`.

Per kind:

- **Star / sun** (`:2650-2670`, from `mapStars.jsonl`): `orbitID: null`,
  **`position: {x:0,y:0,z:0}`** (the star is the system origin), `radius` from `row.radius`
  else type radius, `celestialIndex: null`, `orbitIndex: null`,
  `itemName = "<SystemName> - Star"`. `mapStars.jsonl` fields used: `_key, solarSystemID,
  typeID, radius`. **That is all the sun data there is** — no temperature, luminosity, spectral
  class, or age in this pipeline. Confirmed absence.
- **Planet** (`:2672-2695`, `mapPlanets.jsonl`): `orbitID` = the star's itemID (0 → null),
  `position` = `{x,y,z}` as-is, `celestialIndex`, `orbitIndex`,
  `itemName = "<SystemName> <RomanNumeral(celestialIndex)>"` — Roman numeral built at `:643-672`
  from the table `[1000 M, 900 CM, 500 D, 400 CD, 100 C, 90 XC, 50 L, 40 XL, 10 X, 9 IX, 5 V, 4 IV, 1 I]`.
  `mapPlanets.jsonl` fields: `_key, solarSystemID, typeID, orbitID, position, radius,
  celestialIndex, orbitIndex`.
- **Moon** (`:2697-2720`, `mapMoons.jsonl`): same shape; `orbitID` = parent planet itemID;
  `itemName = "<ParentPlanetName> - Moon <orbitIndex>"`.
- **Asteroid belt** — see §5.

`positions` are kept full precision for celestials (`cloneVector`, `:611-617`); only stargates
get rounded (3 dp, `:624-630`).

### `sovereigntyStatic` (`:2029-2074`) joins `planetResources.jsonl`
Per-planet: `{planetID, solarSystemID, power, workforce, reagentDefinitions:[{reagentTypeID,
amountPerCycle, cyclePeriodSeconds, securedPercentage: 50, securedCapacity, unsecuredCapacity,
securedStock: 0, unsecuredStock: 0}]}`. Capacity = `amountPerCycle * 1460`
(`SKYHOOK_REAGENT_STORAGE_CYCLES = 1460`, `:339`, documented from a TQ client log 2026-09-10).
Claimable systems = systems whose **sun itemID** appears in `planetResources` (`:2033-2037`).
`sovereigntyStatic` version `{major: 24, minor: 1, patch: 1, build_tags: ["elysian-eve","ccp-equinox-resource-data4"]}` (`:328-334`).

### `mapTagsAuthority` — **confirmed empty** (`:1308-1327`)
Emits `{version, generatedAt, source:{provider:"EvEJS local bootstrap", reason:"No public SDE
map-tag source data identified; emit empty authority shape."}, systems: [], constellations: [],
regions: []}`. No map-tag data exists.

---

## 3. Stargates

### `stargates/data.json` — wrapper `{source, count, stargates[]}` (`:3119`), sorted by itemID (`:2842`)
Record `:2636-2648`, from `mapStargates.jsonl`:

| field | value |
|---|---|
| `itemID` | `row._key` |
| `typeID` | `row.typeID` |
| `solarSystemID` | `row.solarSystemID` |
| `itemName` | `"Stargate (<destination system name>)"` (`:2632-2635`) |
| `position` | `{x,y,z}` **rounded to 3 decimal places** |
| `radius` | **hardcoded 15000** (not from the SDE) |
| `destinationID` | `row.destination.stargateID` |
| `destinationSolarSystemID` | `row.destination.solarSystemID` |
| `destinationName` | name of the destination gate, or `null` |

**So the gate→gate link is a nested object in the SDE row: `destination: {stargateID,
solarSystemID}`.** That is the whole linking mechanism — a direct peer pointer, no separate
jump table. `mapStargates.jsonl` row fields used: `_key, typeID, solarSystemID, position,
destination{stargateID, solarSystemID}`.

Absent from gate rows: **no orientation/rotation, no activation radius, no jump mass, no
faction, no celestial parent (`orbitID`)**. Confirmed absence.

### `stargateTypes/data.json` (`:3120-3128`, record `:827-838`)
`{typeID, typeName, groupID, categoryID, groupName, raceID, graphicID, published}`.

### Gate geometry is reconstructed in server code, not datamined
- `server/src/space/stargateRadius.js:13` — `DEFAULT_STARGATE_PHYSICAL_RADIUS_METERS = 15_000`;
  `:14-19` resolution order for an explicit radius: `physicalRadius`, `ballRadius`,
  `destinyRadius`, `typeRadius`, then type record radius, then instance `radius`.
- `server/src/space/stargateOrientation.js`, `server/src/space/transientStargates.js`,
  `server/src/space/runtime/scene/stargateActivation.js`, `.../spaceRuntime/stargateJump.js`,
  `server/src/space/stargateJumpRange.js` — all derived/computed, no data file.
- Only cosmetic per-gate data exists: `staticTables/stargateVisualOverrides/data.json`,
  `byItemID` → `{skinMaterialSetID}`, **87 entries**, e.g. `50001248/50001249/50001250 → 3636`.

### Wormhole "gates" are far better covered: `staticTables/explorationWormholeStatic/data.json`
`{version, generatedAt, source, counts, k162Type, codeTypes[], wanderingProfiles[], systems[], systemsByClass{}}`
- `counts`: systemCount **8490**, codeTypeCount **99**, staticSlotCount **3015**,
  environmentSystemCount **1038**, systemClassCount **22**, wanderingProfileCount **42**.
- `systems[]` row: `{solarSystemID, solarSystemName, regionID, regionName, constellationID,
  securityStatus, wormholeClassID, nebulaID, environmentFamily, environmentTypeID,
  environmentName, environmentPosition, environmentEffectTypeID, environmentEffectTypeName,
  staticSlots[]}`. Real rows:
  `{30000001, "Tanoo", 10000001 "Derelik", 20000001, securityStatus 0.858324, wormholeClassID 7, nebulaID 11799, staticSlots []}`;
  `{31000007, "J105443", 11000001 "A-R00001", 21000311, securityStatus -0.99, wormholeClassID 1, nebulaID 11781, staticSlots:[{slotKey:"31000007:1:Z060", slotIndex:1, code:"Z060", typeID:30668, typeName:"Wormhole Z060", targetClassID:9, lifetimeMinutes:960, maxStableMass:1000000000, massRegeneration:0, maxJumpMass:62000000, distributionID:304, radius:3000, graphicID:3715}]}`
- `codeTypes[]` row: `{code:"A009", typeID:34439, typeName:"Wormhole A009", graphicID:3715,
  radius:3000, targetClassID:13, lifetimeMinutes:270, maxStableMass:3000000000,
  massRegeneration:3000000000, maxJumpMass:5000000, distributionID:296}`
- `k162Type`: `{typeID:30831, typeName:"Wormhole K162", graphicID:3715, radius:3000}`
- `systemsByClass` keys: `1..9, 12..23, 25` (22 classes).
- `wanderingProfiles[]` row: `{profileKey:"smallShipWspaceAndNull:c1:e004", recipeKey,
  sourceClassID:1, sourceLabel:"C1", code:"E004", typeID:34134, destinationClassID:1,
  destinationLabel:"C1", lifetimeMinutes:270, maxStableMass:3000000000,
  massRegeneration:3000000000, maxJumpMass:5000000, distributionID:296, radius:3000,
  graphicID:3715, estimatedUniverseCount:1, totalCountText}`
- Source is a local spec, not the SDE: `scripts/DataSync/spec/explorationWormholeStatics.json`
  and `...WanderingProfiles.json` — **neither present in the repo**.

---

## 4. Stations

### `stations/data.json` (`:3129-3139`); record built `:2760-2824` from `npcStations.jsonl`
Joins: `mapSolarSystems`, `mapConstellations`, `mapRegions`, `types`, `npcCorporations`,
`stationOperations`, the celestial table (for `orbitID`), and the static docking placements.

Emitted fields:
```
stationID, security, dockingCostPerVolume, maxShipVolumeDockable, officeRentalCost,
operationID, stationTypeID, corporationID, solarSystemID, solarSystemName,
constellationID, constellationName, regionID, regionName, stationName,
position{x,y,z}, reprocessingEfficiency, reprocessingStationsTake, reprocessingHangarFlag,
itemName, itemID, groupID, categoryID, orbitID, orbitName, orbitGroupID, orbitTypeID,
orbitKind, stationTypeName, stationRaceID, stationGraphicID, radius, interactionRadius,
useOperationName, dockEntry, dockPosition, dockOrientation, undockDirection, undockPosition
```
Hardcoded constants (`:2785-2787`, `:2801`): `dockingCostPerVolume: 0`,
**`maxShipVolumeDockable: 50000000`**, `officeRentalCost: 10000`,
`reprocessingHangarFlag` default **4**. `interactionRadius = radius` (type radius) (`:2814-2815`).
`orbitKind` maps `"sun"` → `"star"` (`:2810`).
Naming rule (`:2774-2780`): if `row.useOperationName === true` →
`"<orbitName> - <corporationName> <operationName>"`, else `"<orbitName> - <corporationName>"`.

`npcStations.jsonl` row fields used: `_key, typeID, solarSystemID, ownerID, operationID,
orbitID, position, reprocessingEfficiency, reprocessingStationsTake, reprocessingHangarFlag,
useOperationName`.

### Docking geometry: `staticTables/stationDockingPlacements/data.json` — 1.1 MB
- `source.provider` = `"EvEJS reverse engineered client station placement data"`;
  `source.extractedFrom` = `"server/src/gameStore/data/stations/data.json before generated data purge"`;
  `source.note` says it is merged back in by `database-creator.js`.
- `columns` = `["stationID","stationTypeID","dockEntry","dockPosition","dockOrientation","undockDirection","undockPosition"]`
- `count` = **5210** (array-of-arrays, column-oriented to save space).
- Real rows:
  - `[60000004, 1531, [2447.21484, -3841, 2988], [1723680886834.563, 256414062476.266, -60755438456.491], [-0.80788, 0.278403, 0.519444], [-0.80788, 0.278403, 0.519444], [1723680886834.563, 256414062476.266, -60755438456.491]]`
  - `[60000007, 1531, [2447.21484, -3841, 2988], [-640881259246.665, -65580443853.8, -506471422950.948], [-0.161282, 0.281128, 0.946021], [-0.161282, 0.281128, 0.946021], [-640881259246.665, -65580443853.8, -506471422950.948]]`
  - Note the semantics: `dockEntry` is **station-local** (metres, small); `dockPosition` and
    `undockPosition` are **system-absolute** (1e11-scale); `dockOrientation` and
    `undockDirection` are unit vectors and are identical in both sample rows.
  - Same `stationTypeID` 1531 shares the same `dockEntry` — the local entry point is
    per-type, the absolute positions are per-station.
- Merge is type-guarded (`:2771-2773`): the placement is only applied if its `stationTypeID`
  is absent or matches the station's actual `stationTypeID`; otherwise the five geometry
  fields are `null`.

### `staticTables/stationGraphicLocators/data.json` — 44 locators
`_meta.source = "client graphicLocations.fsdbinary"`. Row:
`{stationTypeID: 54, typeName: "Caldari Logistics Station", graphicID: 237,
graphicLocationID: 28, hasUndockLocators: true, graphicFile: null,
directionalLocatorCategories: ["undockPoint"], hasDirectionalLocators: true,
undockLocatorCategories: ["undockPoint"],
directionalLocators: [{category:"undockPoint", name:"undockPoint",
position:{x:36, y:6854.26221, z:7948.4668}, direction:{x:0,y:0,z:1}}]}`.
A sibling `structureGraphicLocators/data.json` (55 KB) holds the same shape for structures.

### `stationTypes/data.json` (`:3140-3151`, record `:840-868`)
`{stationTypeID, typeName, groupID, categoryID, groupName, raceID, graphicID, radius,
basePrice, volume, portionSize, published, dockEntry, dockOrientation, graphicLocationID,
directionalLocatorCategories, undockLocatorCategories}` — `dockEntry`/`dockOrientation` come
from `directionalLocators[0]` of the graphic locator.

### Station rotation oracle: only TWO entries, hand-authored
`database-creator.js:465-474` `STATION_ROTATION_OVERRIDES`:
- `60003760 → [206.8031, 0, 0]` (Jita IV - Moon 4 - Caldari Navy Assembly Plant)
- `60015249 → [305, 0, 0]` (Manifest V - AIR Laboratories Trade Center)
The comment at `:462-464` states the rule explicitly: ordinary stations must **omit**
`dunRotation` so the client keeps the asset's default orientation, and docking placement
geometry is **not** authoritative evidence for that slim-item field. `applyHandAuthoredStationRotations`
(`:476+`) deletes `dunRotation` from every station first.

### `stationStandingsRestrictions/data.json`
`source.resourcePath = "res:/staticdata/stationstandingsrestrictions.fsdbinary"`,
`counts = {ownerCount: 1, ruleCount: 7}` — essentially empty.

---

## 5. Asteroid belts and asteroid fields

### `asteroidBelts/data.json` (`:3013`) is a **filtered view** of `celestials`
`:2957` — `belts = celestials.filter(kind === "asteroidBelt")`. Wrapper `{source, count, belts}`.

### Belt record (`:2722-2748`), from `mapAsteroidBelts.jsonl`
Base celestial fields plus:
- `radius`: **if `row.radius == null` → 15000**, else the SDE value (the only celestial with a
  radius fallback).
- `securityClass`: copied from the system (the ore table keys off it).
- `itemName = "<ParentPlanetName> - Asteroid Belt <orbitIndex>"`.
- `fieldSeed = row._key` (the belt's own itemID).
- then `addAsteroidFieldProfile(belt)` (`:708-722`) appends:
  `fieldStyleID, fieldSeed, asteroidCount, clusterCount, fieldRadiusMeters,
  clusterRadiusMeters, verticalSpreadMeters, largeAsteroidCount`.

`mapAsteroidBelts.jsonl` row fields used: `_key, typeID, solarSystemID, orbitID, position,
radius, celestialIndex, orbitIndex`.

### Deterministic field generation — the exact algorithm
`hashInteger(text)` (`:680-682`) = first 4 bytes of `sha256(text)` read big-endian as uint32.
`seededInt(seed, salt, min, max)` (`:684-691`) = `min + hashInteger("<seed>:<salt>") % (max-min+1)`,
returning `min` when `max <= min`. Salts used: `"asteroidCount"`, `"clusterCount"`,
`"fieldRadiusMeters"`, `"clusterRadiusMeters"`, `"verticalSpreadMeters"`, `"largeAsteroidCount"`.
**Seed is the belt itemID — so the whole field envelope is reproducible from `itemID` alone.**

### Style selection rule (`:693-706`)
1. `securityClass` starts with `"W"` **or** `solarSystemID >= 31000000` → `wormhole_standard`
2. `security >= 0.45` → `empire_highsec_standard`
3. `security >= 0` → `empire_lowsec_standard`
4. else → `nullsec_standard`

### `asteroidFieldStyles/data.json` (`:3014-3021`) — exactly 4 styles (`:341-460`)

| field | highsec | lowsec | nullsec | wormhole |
|---|---|---|---|---|
| `fieldStyleID` | empire_highsec_standard | empire_lowsec_standard | nullsec_standard | wormhole_standard |
| `securityMin` / `securityMax` | 0.45 / 1.1 | 0 / 0.449999 | -1 / -0.000001 | -1 / 1.1 |
| `asteroidCountMin/Max` | 16 / 24 | 20 / 30 | 24 / 36 | 18 / 28 |
| `clusterCountMin/Max` | 3 / 4 | 4 / 5 | 5 / 6 | 4 / 5 |
| `fieldRadiusMin/MaxMeters` | 26000 / 34000 | 30000 / 38000 | 34000 / 46000 | 28000 / 40000 |
| `clusterRadiusMin/MaxMeters` | 3500 / 7000 | 4500 / 8500 | 5000 / 10000 | 4500 / 9000 |
| `verticalSpreadMin/MaxMeters` | 2000 / 5000 | 2500 / 6500 | 3000 / 8000 | 3000 / 7500 |
| `innerExclusionRadiusMeters` | 2000 | 2200 | 2600 | 2400 |
| `largeAsteroidCountMin/Max` | 0 / 1 | 1 / 2 | 2 / 3 | 1 / 3 |

`decorativeTypes` (typeID, weight, radiusMinMeters, radiusMaxMeters) use typeIDs
**60559, 60560, 60561, 60562** in all four styles, with per-style weights and radii, e.g.
highsec `[{60559,4,900,1800},{60560,4,1000,1900},{60561,3,1100,2100},{60562,2,1200,2200}]`;
nullsec `[{60559,2,1300,2600},{60560,3,1400,2800},{60561,4,1500,3100},{60562,4,1600,3400}]`.
`largeTypes` (typeID, weight): highsec `[{90042,3},{90445,2}]`, lowsec `[{90042,2},{90445,3}]`,
nullsec `[{90042,2},{90445,3},{90446,2}]`, wormhole `[{90042,1},{90445,2},{90446,2}]`.
The style source note (`:3016-3018`) states these are **server-authored**, not datamined.

### Ore types per system: `staticTables/asteroidTypesBySolarSystemID/data.json`
This is the real datamined ore table, and it is **extracted from the client, not the SDE**:
- `source.provider = "EVE Client Static Cache"`,
  `source.authority = "asteroidtypesbysolarsystemid.pickle"`,
  `source.clientFile.resourcePath = "res:/staticdata/asteroidtypesbysolarsystemid.pickle"`,
  `resourceHash = 3a649c2572f621d73ded8c7150b99060`, `sizeBytes = 96610`, `extra = ["15372"]`,
  `loader = "scripts/DataSync/build-asteroid-system-ore-table.py"` (**loader absent from repo**).
- `counts = {systemCount: 3974, distinctTypeCount: 36, totalAssignmentCount: 24071}`
  → mean 6.06 ore types per system; 3974 of ~5200 K-space systems covered.
- `systems` = `{"<solarSystemID>": [typeID, …]}`. Systems 30000001–30000008 all carry the same
  9-type list: `[1224, 1228, 1230, 17459, 17460, 17463, 17464, 17470, 17471]`
  (Pyroxeres/Scordite/Veldspar plus their +5%/+10% variants).
- Bootstrap fallback when the client table is unavailable (`:3181-3187`):
  `{counts:{systemCount:0, distinctTypeCount:0, totalAssignmentCount:0}, systems:{}}`
  with note `"Generated without client private asteroid map."`

### Runtime ore distribution rules: `server/src/space/asteroids/asteroidService.js` (1255 lines)
- `:35-36` `STATIC_ASTEROID_ITEM_ID_BASE = 5_000_000_000_000`, `STATIC_ASTEROID_ITEM_ID_STRIDE = 512`
  (so belt itemID → asteroid itemIDs is `BASE + beltIndex*512 + n`).
- `:38-109` `FIELD_SHAPE_PROFILES` — two entries, `default` and `empire_highsec_standard`, each
  ~30 numeric knobs: `axisAngleMin/MaxDegrees` (default -30/75; highsec -65/105),
  `single|multi SpanFactorMin/Max`, `WidthFactorMin/Max`, `BowFactorMin/Max`,
  `AreaPerAsteroidMin/Max` (default single 4,850,000–5,900,000 m², multi 4,350,000–5,400,000;
  highsec single 4,750,000–5,750,000, multi 4,050,000–5,050,000),
  `LaneCountMin/Max` (single 1/1, multi 1/2), `VisualWidthFactorMin/Max`,
  `SideOffsetFactorMin/Max`, `verticalSpreadScale` (0.14 / 0.12),
  `alongJitterScale` (0.03 / 0.025), `crossJitterScale` (0.38 / 0.36).
- `:113-129` `ORE_FAMILY_TYPE_IDS` — 15 families → typeID lists:
  Veldspar `[1230,17470,17471]`, Scordite `[1228,17463,17464]`, Pyroxeres `[1224,17459,17460]`,
  Plagioclase `[18,17455,17456]`, Omber `[1227,17867,17868]`, Kernite `[20,17452,17453]`,
  Jaspet `[1226]`, Hemorphite `[1231]`, Hedbergite `[21]`, Dark Ochre `[1223]`,
  Gneiss `[1229]`, Crokite `[1225,17432,17433]`, **Bistot `[]` (empty — a gap)**,
  Arkonor `[22,17425,17426]`, Mercoxit `[11396,17869,17870]`.
- `:131-147` `SECURITY_CLASS_FAMILIES` — securityClass letter → allowed families:
  `A: [Veldspar, Scordite]`; `B: +Pyroxeres`; `C/C1: +Plagioclase, Omber`;
  `C2/D/D1: [Pyroxeres, Plagioclase, Omber, Kernite, Jaspet]`;
  `E/E1: [Kernite, Jaspet, Hemorphite, Hedbergite]`;
  `F/F1: [Hemorphite, Hedbergite, Dark Ochre, Gneiss]`;
  `G/G1: [Dark Ochre, Gneiss, Crokite, Bistot]`;
  `H/H1: [Crokite, Bistot, Arkonor, Mercoxit]`.
- `:149-153` `BELT_BUCKETS` = `{common:{targetMin:2,targetMax:4,weight:6},
  uncommon:{targetMin:1,targetMax:3,weight:5}, rare:{targetMin:1,targetMax:2,weight:2.5}}`.
- `:155-171` `GENERATED_ORE_ASTEROID_SHELL_TYPE_IDS` = 15 consecutive typeIDs **64063–64077**.
- `server/src/space/asteroids/asteroidData.js:14-15` loads only two tables:
  `TABLE.ASTEROID_BELTS` and `TABLE.ASTEROID_FIELD_STYLES`, indexing belts by itemID and by
  solarSystemID (sorted by itemID, `:37-39`).

### Respawn
- **Belt asteroid respawn: no constant found.** `grep -iE "respawn|replenish|regrow"` across
  `server/src/space/asteroids/` returns nothing. Confirmed absence.
- **Ice** has one: `server/src/services/mining/iceSystemAuthority.js:269`
  `RESPAWN_DELAY_MINUTES = 360` (6 h), surfaced as `respawnDelayMinutes` (`:540`, `:602`) and
  converted at `:755` (`*60*1000`). `miningRuntimeState.js:1390-1399` computes
  `respawnDelayMs` / `respawnAtMs` from the authority row.
- **Dungeon** (content-pack) respawn is per-object, in `guardCommand.sourceFields`:
  `{entityMinRespawnDelay: 900, entityMaxRespawnDelay: 1500, objectMinRespawnDelay: 3600,
  objectMaxRespawnDelay: 3600, entitySpawnOnce: 0, entityDelaySpawn: 1, objectDelaySpawn: 0,
  objectTriggerSpawn: 0, aggressionRange: 180000, orbitRange: 5500}`.

### Ore *anomaly* rocks are exact client extracts — the strongest data in the repo
`content-packs/evejs-ore-anomalies-1.1.0.json`, 199 templates, keyed `"client-dungeon:<id>"`.
Example template `client-dungeon:10021` ("Empire Border Rare Asteroids", archetypeID 27,
factionID 500021, difficulty 5, entryObjectTypeID 28356, dungeonNameID 642247):
- `populationHints.miningRocks[]` — 20 rocks, each:
  `{key:"client-object:1159466", source:"client_static_3475087", exact:true, objectID, dunObjectID,
  roomID:28945, roomKey:"room:28945", typeID:46675, groupTag:"Asteroids", order:14,
  count:1, quantity:1036, sourcePosition:[-9008.05273, 5065.89453, 253.92139],
  positionOffset:{x:-9008.05273, y:2967.81299, z:-5632.75732}, dunRotation:null}`
  → **exact per-rock ore typeID, m³ quantity and position.**
- `exactContentCaps = {miningRocks: 20, environmentProps: 12}`
- `completion.objectiveMode = "deplete_all_mining_rocks"`
- `resourceComposition = {oreTypeIDs[5], gasTypeIDs[], iceTypeIDs[], hasAnyResources: true}`
- `audit = {valid:true, errors:[], sourceObjectCount:35, roomCount:1, miningRockCount:20,
  environmentPropCount:12, controllerCount:2, gateCount:0, entityAnchorCount:0,
  guardCommandCount:2, triggerCount:0, authoredQuantity:206218, resourceCountsByRoomAndType:{28945:{…5 types}}}`
- `provenance = {kind:"client_static", clientBuild:3475087, geometry:"exact",
  resourceObjects:"exact", quantities:"exact_dunRadius", sourceFields:"lossless"}`
  → **quantity is derived from the client's `dunRadius`.**
- `spawnProfilesByFamily.ore = {enabled:true, selectionAuthority:"strict_security_ore",
  bands:["highsec","lowsec","nullsec"], siteLifetimeMinutes:1440, siteOrigin:"universe_dungeon",
  templateFilters:{siteFamilies:[],siteKinds:[],nameIncludesAny:[],nameExcludesAny:[]}}`
- Room record: `{roomKey:"room:28945", roomID:28945, label:"Asteroid Belt", shortName:"R1",
  grid:[0,0,0], position:[0,0,0], stage:"entry", initialState:"active"}`; raw
  `sourceFields.roomName` retains a BOM (`"﻿Asteroid Belt"`).

### Ice systems: `server/src/services/mining/iceSystemSyntheticNullsecAuthority.json` — 1.17 MB
`{generatedAt:"2026-06-20", sourceReview, policy, publicSources[5], summary:{systems:596}, rows[596]}`.
`policy` states plainly: **"Emulator-owned synthetic nullsec/Pochven ice authority: DOTLAN
supplies system and slot evidence; DOTLAN system celestials supply composition where present;
missing Pochven composition uses documented original-quarter best-fit templates. Shattered
wormhole ice is deferred."** `publicSources[0] = "https://evemaps.dotlan.net/region"`.
Row: `{solarSystemID:30000021, solarSystemName:"Kuharah", regionID:10000070,
regionName:"Pochven", regionSlug, constellationID:20000788, security:-1,
securityBand:"nullsec", securityClass:"B3", slotCount:1, sourceDungeonID:5511,
templateID:"client-dungeon:5511", quarter:"amarr", iceTypeNames:["Clear Icicle IV-Grade",
"Glare Crust","Dark Glitter","Gel…"]}` — **scraped from DOTLAN, not datamined.**

### Moon mining: `staticTables/moonMiningPoints/data.json` — 27.7 MB, **157,905 points**
`{schemaVersion:1, datasetVersion:"evejs-moon-mining-points-3396210-v1", sourceBuild:3396210,
source:{provider:"EveJS community moon-mining-point contribution", sourceFile:"moonMiningPoints.json",
sha256:"cfd0da86…", validationUniverse:{celestialTableSha256:"9c6f352f…",
solarSystemTableSha256:"f31209d3…"}}, count:157905, contentHash:"6e8821d5…", points[]}`.
Point row: `{moonID:40000536, solarSystemID:30000010, beaconID:1040000536,
position:{x:96211746656.3836, y:12706037692.810184, z:-9171791430.718697},
distanceFromMoon:1660000}`. Second/third rows: `{40000539, 30000010, 1040000539,
distanceFromMoon:2100000}`, `{40000542, 30000010, 1040000542, distanceFromMoon:1640000}`.
Rule encoded: `beaconID = 1_000_000_000 + moonID`; `distanceFromMoon` is in metres,
~1.6–2.1 Mm. **Moon ore composition is NOT here** — see `tools/moon-data/generate-moon-composition-pack.js`,
which generates it from a world seed (default `"EveJS"`).

---

## 6. The sun / star — complete field list

From `mapStars.jsonl` only `_key`, `solarSystemID`, `typeID`, `radius` are consumed
(`:2621-2624`, `:2650-2670`). The emitted sun celestial is:
`{itemID, typeID, groupID (6), categoryID (2), groupName ("Sun"), solarSystemID,
constellationID, regionID, orbitID: null, position:{x:0,y:0,z:0}, radius, itemName:"<Sys> - Star",
security, celestialIndex: null, orbitIndex: null, kind:"sun"}`.
Indirectly, `solarSystems[].sunTypeID` carries the star typeID (`:2126`).
Suns also gate sovereignty claimability (`:2033-2037`) and get a `starConfigurations` entry
(`:2073-2074`, `{starID, …}`).
**Confirmed absent: temperature, luminosity, spectral class, age, colour.**

---

## 7. Oracle candidates for a clean-room reimplementation

Ranked by usefulness. Note the licence constraint: these are AGPL files, so use them as
*expected-value references* to check our own output against, not as shipped data.

1. **`staticTables/stationDockingPlacements/data.json` — 5210 rows, 7 columns.** The single
   best geometric oracle: per-station dock entry (local), dock position (absolute), dock
   orientation, undock direction, undock position. Self-describing via `columns`.
   Its own provenance says it was *extracted from a previously generated stations table*, i.e.
   it is itself an oracle snapshot taken before a data purge.
2. **`staticTables/moonMiningPoints/data.json` — 157,905 rows** with `contentHash` plus
   `validationUniverse.celestialTableSha256` / `.solarSystemTableSha256`. Those two hashes are
   a **universe-parity fingerprint**: if our celestial and solarSystem tables hash to
   `9c6f352fde8abd11294f79bee98929128dc14df19fc8547c3df1d15e857a574f` and
   `f31209d3292ffd93ce787474f9085b90c76caaf770c466995fdb20048f7acf9f` respectively, our
   universe matches theirs byte-for-byte. (Hash input format is not documented in-tree.)
3. **`server/src/abyssal/catalog/fixtures/t0TranquilRetailCapturedSceneryFixture.json` — 310 KB.**
   An explicit **captured-from-Tranquility golden file**: `{sourceArchive:"docs/tqlogs.zip",
   extractionPolicy, layouts:{electrical:{sourceFile:"TQTranquilAbyssal1.txt",
   filamentTypeID:56131, rooms:{"1":{sourceLine:5438, capturedBallCount:39,
   sceneryObjectCount:28, rows:[…]}}}}}`. Row tuple shape:
   `[itemID(string), typeID, ownerID, ?, null, null, position[3], rotation[3], groupID, categoryID]`
   e.g. `["9002291510000069033", 47945, 500021, 1059320, null, null,
   [123531.53125, 158271.078125, 836786.3125], [56.552, 20.482, 38.085], 1975, 2]`.
   This is the clearest "what the real server actually sent" artefact in the repo, with
   source file + line number citations.
4. **Content-pack `audit` blocks and `clientOverlayCounts`** (every pack). Per-template
   `{valid, errors[], sourceObjectCount, roomCount, miningRockCount, environmentPropCount,
   controllerCount, gateCount, entityAnchorCount, guardCommandCount, triggerCount,
   authoredQuantity, resourceCountsByRoomAndType}` plus pack-level `payloadSha256` and
   `clientOverlaySha256`. These are ready-made parity fixtures for a site generator.
   Aggregate example: `dungeonClientContent` authoredQuantity `3313731.32481` across
   26 templates / 1094 mining rocks.
5. **`asteroidFieldStyles` + `seededInt`/`hashInteger`** (`database-creator.js:341-460`, `:680-722`).
   Because the field envelope is `sha256("<beltItemID>:<salt>") % range`, it is a *closed-form,
   independently reproducible* oracle: given a belt itemID, any implementation can compute the
   expected `asteroidCount`, `clusterCount`, `fieldRadiusMeters`, `clusterRadiusMeters`,
   `verticalSpreadMeters`, `largeAsteroidCount` and compare. Note these are *their* numbers,
   not CCP's — useful as a cross-check of our own determinism, not as ground truth.
6. **`staticTables/*/counts` blocks** throughout — ~25 tables publish exact row counts
   (agentCount 10941, missionCount 2878, systemCount 3974/8490, typeCount 7343, etc.).
   Cheap smoke-test targets for a parallel import.
7. **`staticTables/stationGraphicLocators`** (44 types) and **`structureGraphicLocators`** —
   per-type undock locator positions and directions from `graphicLocations.fsdbinary`.
8. **`tools/market-seederv3/data/price-manifest.json`** — 4781 typeIDs with timestamped ESI
   prices and a documented `costModel`/`pricingRules`; a reference price oracle.
9. **`server/src/abyssal/catalog/content/baseRoomGeometryFactsV1.json`** — 247 KB, a 9-element
   array named "geometry facts", and `abyssalRewardTables.generated.json` (6.9 MB),
   `retailAuthoredContent.generated.json` (4.6 MB), `qsnaAbyssalSpawnTables.generated.json`
   (768 KB), `abyssalLootTables.generated.data/tier-{4,5,6}.json` — generated derived tables.
10. **`server/src/services/loyalty/lpStoreStaticOffers.json`** — 9.8 MB LP store offers
    (`evermarksCatalog` cites `https://www.fuzzwork.co.uk/lpstore/sell/10000002/1000419/withblueprints`).

### What is NOT an oracle (confirmed absences)
- **No test-vector / golden-file directory.** `find` for `fixtures|__fixtures__|golden|testdata|parity`
  returns exactly one hit: `server/src/abyssal/catalog/fixtures/`.
- **No committed celestial, solarSystem, station or stargate table** to diff against. You must
  run their `database-creator.js` against SDE build 3396210 to materialise one.
- **No `mapTagsAuthority` data** (explicitly empty by design).
- **No belt asteroid respawn rule.**
- **No star physical properties beyond typeID and radius.**
- **No stargate orientation/rotation data** (only 87 cosmetic `skinMaterialSetID` overrides).
- **No SDE sqlite or YAML** anywhere; nothing in `server/assets/` but one overview YAML.

---

## 8. Generated-vs-committed summary

| bucket | where | committed? | generated by |
|---|---|---|---|
| Universe (solarSystems, celestials, stargates, stations, stargateTypes, stationTypes, itemTypes, typeDogma, factions, corporations, sovereigntyStatic, …) — **36 generated of 153 required tables** | `_local/gameStore/data/<table>/data.json` | **No** (`.gitignore:40-41`) | `tools/DatabaseCreator/database-creator.js` ← CCP SDE JSONL build 3396210, downloaded by `EnsureSde.ps1` |
| 42 authority tables | `tools/DatabaseCreator/staticTables/*/data.json` | **Yes**, 166 MB | mostly by absent `scripts/DataSync/*` tools from client FSD/pickle/code dumps; copied verbatim into the DB |
| 18 content packs | `content-packs/*.json` | **Yes**, 339 MB | absent `scripts/ContentPacks/build-*-pack.js` from client dungeon builds 3475087 / 3487903 / 3503375 |
| Moon ore composition | runtime | No | `tools/moon-data/generate-moon-composition-pack.js`, seeded `"EveJS"` |
| Overview preset | `server/assets/overview/*.yaml` | **Yes**, 105 KB | hand-authored/third-party |
| Market prices | `tools/market-seederv3/data/price-manifest.json` | **Yes**, 530 KB | ESI capture 2026-08-15 + Rust seeder |

Placeholder behaviour worth copying: `defaultPlaceholderForTable` (`:3170-3239`) defines the
**empty-but-valid shape** for 16 tables, which is a free spec of each table's required skeleton
(e.g. `rafflesRuntime.nextRaffleId = 980000001`, `killRights.{nextKillRightID:1, nextActivationID:1}`,
`moonExtractions.{nextExtractionID:1, nextEventID:1}`, `mapTelemetry._meta.schemaVersion = 1`).

### Other useful constants spotted in `database-creator.js`
- `:267-275` `ATTRIBUTE_IDS = {mass:4, maxVelocity:37, capacity:38, radius:162, inertia:70,
  signatureRadius:552, warpSpeedMultiplier:600}`
- `:277-288` `REPROCESSING_ATTRIBUTE_IDS = {refiningYieldMultiplier:717, reprocessingSkillType:790,
  rigSize:1547, hiSecModifier:2355, lowSecModifier:2356, nullSecModifier:2357, oreBasicType:2711,
  strRefiningYieldBonus:2722, structureGasDecompressionEfficiencyBonus:3261,
  gasDecompressionBaseEfficiency:3262}`
- `:290-298` ore reprocessing skills `{12189, 12195, 60377, 60378, 60379, 60380, 60381}`;
  `:300-306` moon-ore skills `{46152..46156}`;
  `:308-322` gas groupIDs `{12,226,305,307,340,448,649,711,897,920,1975,2020,4168}`;
  `:324-326` general refinable typeID `{41139}`
- `:674-678` `fileTimeNow()` = `(unixMs + 11644473600000) * 10000` → Windows FILETIME; the
  wire format for character timestamps.
- `:2136-2143` bootstrap character defaults: stationID **60003760**, solarSystemID **30000142**
  (Jita), corporationID 1000044, raceID 2, bloodlineID 8, schoolID 33, factionID 500001,
  typeID 1380, shipTypeID **670** ("Capsule"), balance 1000000000, `shipID = characterID + 1000000000000`.
- `:3309-3320` seeded accounts `test`/`test2`, characters `140000001`–`140000004`,
  `hyperCoreTypeID: 52568`.
- `research`: `researchFieldAuthority` notes `"Research fields are npcCharacters[].skills,
  present on agentTypeID 4 only"` and `"Datacore mapping is dogma attribute 182 (requiredSkill1)
  on group 333"`.
