# C3: the world population generator

Research lane C3 (`docs/content/README.md` §3), 2026-10-25. Read-only: this
file and `findings/content-C3-archetypes.tsv`, `findings/content-C3-bands.tsv`
(counts and distributions only, no client rows). Client build 3619879
(`tq-offline`). Probes ran through `tools/analysis.sh` from a scratch
directory; their intermediate dumps in `out/c3_*` are deleted.

Marks as in the shared brief: **[V]** this build's bytecode or data (file and
line, or file and field), **[V2]** EvEJS, **[I]** inferred (inference
stated), **[A]** an assumption we ship (falsifier given), **unknown**.

---

## 0. The answer, shortest form

1. **The client tells us what a site *is*; it never tells us where, how many,
   or how often.** Every dungeon carries an archetype, a difficulty, a
   faction and an entry type, so a site's kind, scan class, rat faction and
   tier are all **[V]**. There is no system placement, no per-system count and
   no site respawn rule anywhere in the 203 static files. Those are authored
   **[A]**, as small server tables.
2. **The generator draws from six archetypes.** Combat sites (24), ore
   anomalies (27), ice belts (28), gas clouds (30), relic sites (44) and data
   sites (45). Together they are **1,244 dungeons** in the corpus, and after
   the filters in §4.2 about 850 of them can actually be drawn. Missions (20, 21, 22, 26)
   belong to C4. FW, incursion, insurgency, homefront and abyssal sites are
   out of scope (`findings/content-C3-archetypes.tsv`, column `C3_role`).
3. **Anomaly or signature is a field.** `dungeon.entryTypeID` is Cosmic
   Anomaly 28356 or Deadspace Signature 19728 **[V]**. Probe class follows
   from the archetype by the client's own table: ore→gravimetric, gas→ladar,
   relic→magnetometric, data→radar, combat→"all" **[V]**
   (`probescanning/explorationSites.pyc:16-29`).
4. **Difficulty 1-5 is the client's scan difficulty, and it tracks
   security.** Combat anomalies climb Hideaway/Burrow (1), Refuge/Den (2),
   Yard/Rally Point (3), Port/Hub (4), Haven/Sanctum (5) **[V-data]**. Data
   and relic sites are Local/Crumbling (2), Regional/Decayed/Looted (3) and
   Central/Ruined (4) **[V-data]**. Mapping difficulty to a security band is
   **[I]**.
5. **Belts are fully specified by the client** apart from depletion and
   regrowth, which the client only *reads*. The client grades a belt's
   depletion by rock count against `asteroidcountbytypeidbeltid`, with
   thresholds of 0.9, 0.3 and 0 (`mining/client/mining_util.pyc:104-109`)
   **[V]**. A server regrowth rule is therefore safe: the client accepts any
   count.
6. **Belt rats have no client spawn row.** `reactiveSpawnComponent` exists
   on 882 types, and none of them is a belt spawner **[V-data]**, so W10's
   `tick_belts` is the right shape. What the corpus *does* give is the
   parameter vocabulary and CCP's typical values: threat timeout 900 s on 874
   of 882 rows, a sensor cadence of 60 s on 776, and a threat range of 250 km
   on 681. §8 proposes adopting them.

---

## 1. Inputs, all on disk, and what each one gives

| input | gives | status |
|---|---|---|
| `dungeons.fsdbinary`: `archetypeID@64`, `difficulty@72` (bit 0x10), `entryTypeID@88` (0x40), `factionID@92` (0x80), rooms dict `@40`, and per object `typeID@140`, `position@116`, `radius@128`, `guardCommand@64`, `entities@16` | what a site is, its layout, and its in-pocket respawn rules | **[V]**, `docs/re/dungeon-model.md` |
| `archetypes.fsdbinary`, 71 rows: `contentTags@8`, `location@16`, `descriptionID@24`, `titleID@28` | the archetype's UI title and tags (`feature_combat_anomalies`, `feature_ore_anomalies`, `feature_ice_belts`) | **[V]** schema from `findings/pyd-schemas.tsv`; the titles resolve for 33 of 71 |
| `appConst.pyc:550…`, `dunArchetype*` | names for the archetype ids: 24 CombatSites, 27 OreAnomaly, 28 IceBelt, 30 GasClouds, 44 RelicSites, 45 DataSites, 31 EscalatingPathDungeons, 38 Wormhole | **[V]**, all 51 constants |
| `oretypesbydungeonid` (1,120), `icetypesbydungeonid` (21), `gastypesbydungeonid` (83), `asteroidcountbytypeidroomiddungeonid` (1,221) | the resource types per site and the rock count per room | **[V]**, these close 100% into the dungeon objects |
| `systems.static` | `regionID`, `constellationID`, `securityStatus`, `factionID`, `wormholeClassID` per system | **[V]**, 8,490 systems |
| `asteroidcountbytypeidbeltid.static` | `{beltID: {oreTypeID: rockCount}}`, 40,928 belts | **[V]**, already consumed by `asteroid.rs` |
| `asteroidtypesbysolarsystemid.pickle` | `{systemID: (oreTypeID,…)}`, 3,974 systems | **[V]**. The 3,974 systems are exactly the systems that have belts, so the file is redundant with the belt counts |
| `spacecomponentsbytype` `reactiveSpawnComponent@696` (bit 0x40 @+1824) | CCP's spawner parameters | **[V]** schema; 882 rows decoded here |
| `solarsystemcontent.static`, `npcStations[].ownerID` → `npccorporations.factionID@160` | pirate-owned NPC stations | **[V]**, 5,209 stations |
| `probescanning/const.pyc` | `scanDifficulty{VeryEasy..VeryHard}` = 1..5, `probeScanGroupSignatures` 3, `probeScanGroupAnomalies` 8 | **[V]** |
| `evedungeons/client/anomalyTracker.pyc:232-331` | what the client asks for: `dungeonInstanceCacheMgr.Get{CombatAnomaly,IceBelt,OreAnomaly}Instances`, `…Count`, `GetOreAnomaliesCountInRange`, and the FW, homefront, insurgency and Triglavian variants | **[V]**. The shapes are C2's |
| `explorationscanner/common/cosmicAnomalyInfo.pyc`, `signatureInfo.pyc`, `siteInfo.pyc` | the record a site becomes. Every site has `position`, `targetID`, `difficulty`, `dungeonID` and `archetypeID`; an anomaly adds `siteID`, `instanceID`, `dungeonNameID`, `factionID`, `scanStrengthAttribute`, `allowedTypes`, `entryObjectTypeID` and `solarSystemID`; a signature adds `deviation` | **[V]** attribute names (the field order is C2's) |

**Absent, settled:** a per-system site count, a site's system placement, its
respawn after completion, which rat faction a region gets, the NPC fleet
behind `npcGroupingID` (C6), loot (C6), and wormhole statics.

---

## 2. The site catalogue: what the corpus offers per archetype

From `findings/content-C3-archetypes.tsv`; every number is **[V-data]**.

| arch | kind | dungeons | anomaly / signature entry | difficulty histogram | medians: rooms / entities | generator |
|---|---|---:|---|---|---|---|
| 24 | combat sites | 507 | 325 / 156 | 1:69 2:77 3:95 4:106 5:104 | 1 / 7 | **G1** |
| 27 | ore anomalies | 124 | 114 / 4 | 1:16 2:52 3:28 4:6 5:12 | 1 / 0 | **G2** |
| 28 | ice belts | 20 | 20 / 0 | 2:18 | 1 / 0 | **G3** |
| 30 | gas clouds | 75 | 0 / 75 | 2:42 4:10 5:17 | 1 / 0 | **G4** |
| 44 | relic sites | 285 | 0 / 285 | 2:84 3:84 4:106 | 1 / 0 | **G5** |
| 45 | data sites | 233 | 1 / 232 | 2:46 3:54 4:101 | 1 / 0 | **G5** |
| 31 | escalations | 384 | 24 / 5 (346 `Beacon`) | 3:110 4:121 5:145 | 1 / 6 | G6, later |
| 29 | combat hacking | 17 | 0 / 17 | 4:16 | 1 / 12 | later |

**Facts that shape the generator:**

- **Combat sites are single-room.** 292 of 507 have one room, and the
  median combat anomaly has 7 spawn entities and 44 objects; the 99th
  percentile is 544 objects and the largest 797. The existing
  `dungeon::Pocket` (one point plus NPCs) is the right container for G1 once
  it learns to place objects.
- **Combat anomalies are per faction.** After the §4.2 filters, 189
  combat-anomaly dungeons remain for the five pirate factions: Guristas 40,
  Blood Raiders 39, Sansha 39, Angel 36 and Serpentis 35. By difficulty that
  is 1:47, 2:35, 3:40, 4:38, 5:29. Combat *signatures* (DED-style
  Watch/Vigil/Annex/Base/Fortress) number 118, by difficulty 1:11, 2:21,
  3:16, 4:22, 5:48.
- **Data and relic sites exist for every pirate faction**, about 50 relic
  and about 30 data dungeons each, at difficulties 2, 3 and 4 only.
- **Ore anomalies split by ore tier.** Classifying each archetype-27
  dungeon by the band its ores occur in, in belts (§7.2): 7 are hisec
  ("Imperial/State/Republic/Federation Managed Asteroid Belt", 64 rocks
  each), 16 lowsec (Omber/Kernite/Jaspet/Hemorphite/Hedbergite deposits,
  8-77 rocks) and 16 nullsec (Arkonor/Bistot/Mercoxit deposits and
  "Asteroid Cluster", 4-64 rocks). 83 contain an ore that no belt carries:
  Gneiss, Dark Ochre, Crokite, the Pochven ores (Kylixium and others) and the
  moon-style ores. **Gneiss, Dark Ochre and Crokite occur in no belt in this
  build** [V-data], so those ores exist only through anomalies.
- **Ice is 20 dungeons in a clean ladder** [V-data]: four empire ices (Blue
  Ice, White Glaze, Glacial Mass, Clear Icicle, 19-21 rocks each); each of
  those plus Glare Crust (24-26); a lowsec/null-style tier (+Dark Glitter,
  Gelidus, IV-Grade, 32-37); a +Krystallos tier (43); and one "Shattered Ice
  Field" with 206 rocks. That ice and empire correspond (Blue Ice to
  Gallente, and so on) is **[A]** lore, not client data.
- **Gas:** 65 "… Nebula" sites plus one training site (Cytoserocin or Mykoserocin, booster gas, k-space)
  and 9 "… Reservoir" sites (Fullerite, wormhole-space gas) [V-data].
- **In-pocket respawn is authored per object** [V-data]:
  `guardCommand.entityMinRespawnDelay` is 3,600 s on 5,602 of arch 24's guard
  objects, 2,880 s on 1,013, and 86,400 s on 263. Arch 27 uses 3,600 s (166)
  and 900 s (64); arch 28 uses 3,600 s on all 81; arch 30 uses 3,600 s (398)
  and 900 s (13). This is a *rat inside an instantiated site* coming back,
  not the site respawning.

---

## 3. The security bands, and what the belts say per band

`findings/content-C3-bands.tsv`, from `systems.static` plus the belt counts.
The band rule is: `securityStatus >= 0.45` is high, `> 0` is low, otherwise
null; `31xxxxxx` systems are wormhole space and `32xxxxxx` are void. The
0.45 cut is **[I]**: it is the client's displayed rounding to 0.5, not a
constant read.

| band | systems | with belts | belts | belts/system (mean, max) | rocks/belt p10 / p50 / p90 / max | ore types |
|---|---:|---:|---:|---|---|---:|
| high | 1,246 | 1,179 | 11,960 | 10.1, 50 | 78 / 104 / 136 / 186 | 21 |
| low | 687 | 682 | 7,137 | 10.5, 40 | 37 / 56 / 80 / 120 | 18 |
| null | 3,552 | 2,113 | 21,831 | 10.3, 42 | 4 / 21 / 34 / 64 | 15 |
| wh | 2,604 | 0 | 0 | — | — | 0 |
| void | 401 | 0 | 0 | — | — | 0 |

**[V-data]**. So hisec belts are big and plain, null belts are small and
rich, and wormhole space has no belts at all (its ore is in anomalies, which
are out of scope here).

**Band from difficulty, for exploration sites, [I]:** {1, 2} → high, 3 →
low, {4, 5} → null. The inference rests on the data and relic name tiers
(Local/Crumbling = 2, Regional/Decayed/Looted = 3, Central/Ruined = 4) and on
the combat ladder. **Falsified by** a human who knows TQ saying "Haven in
hisec" or "Local Data site in null". The fix is then one row of
`population.tsv` (§4.1).

---

## 4. The generator (G1-G5): one pure function per system and day

### 4.1 Author table T1, `population.tsv` (server-authored, in the repo)

```
#band  archetype  entry      count  difficulties  respawn_s
high   24         anomaly    2      1,2           1200
high   24         signature  1      1,2           3600
high   27         anomaly    1      *             3600
high   44|45      signature  1      2             3600
high   30         signature  1      2             3600
low    24         anomaly    3      2,3           1200
low    24         signature  1      2,3,4         3600
low    27         anomaly    2      *             3600
low    44|45      signature  2      3             3600
low    30         signature  1      4,5           3600
null   24         anomaly    5      3,4,5         1200
null   24         signature  2      3,4,5         3600
null   27         anomaly    3      *             3600
null   44|45      signature  2      4             3600
null   28         anomaly    0.25   *             86400
```

- **Every count is [A]**, and is the number *live at once*. A fraction is a
  per-system probability, seeded per system (ice: one system in four).
  **Falsified by** humans saying "empty" or "a theme park". The fix is a
  number, not code.
- `*` for ore means "the archetype-27 dungeons whose ore-tier band (§7.2)
  is this band". The 83 "special" ore anomalies are drawn only by a later,
  explicit row.
- Wormhole and void bands get **no rows** in v1: their content needs
  statics and Sleeper NPCs that no lane has.
- **Seeded by rule R1:** the table above. It is checked in and small, it is
  not client data, and it is the one file an operator edits.

### 4.2 Draw rule: which dungeon fills a slot ([V] filters plus an [A] pick)

For slot `(system, archetype, entry, k)`:

1. **Candidates** are dungeons with `archetypeID ∈ row.archetype`;
   `entryTypeID` = 28356 for an `anomaly` row and 19728 for a `signature` row
   **[V]**; `difficulty ∈ row.difficulties`; `factionID` = the system's rat
   faction (§8.1) when the archetype carries factions (24, 44, 45), else any.
2. **Excluded:** names containing `DO NOT`, `QA`, `TEST`, `Test`,
   `Placeholder`, ` OLD`, or `Training`; gas sites whose gas is Fullerite
   (the 9 wormhole "Reservoir" sites); a missing difficulty (arch 24 has 49
   of these, mostly event content); and faction 500021 (Disciples of
   Purity and the other mercenary corps, 91 combat sites) until C6 has fleets
   for it. This filter is **[A]**. **Falsified by** an excluded name a human
   wants, or an included one that is broken.
3. **Pick:** `candidates[splitmix64(seed) % len]` with
   `seed = hash(systemID, archetype, entry, k, generation)`. `generation`
   counts that slot's respawns (§6), so a slot cycles through dungeons and a
   restart reproduces the draw (the same scheme as
   `dungeon::fill`/`npc::roll`).
4. **Empty candidate set → empty slot**, logged once, never a panic.

### 4.3 Placement: author rule R2, the anchor ([A] throughout)

The anchor is the room-0 origin, and the dungeon's room-local positions are
relative to it (`dungeon-model.md` §2, **[V]** room-local):

- **Anchor** = the position of a planet or moon of the system, chosen by the
  seed, plus a seeded random unit vector times U(0.05, 0.25) AU. Reject and
  retry (up to 16 times) when the anchor is within 1,000 km of any
  celestial's surface (the `celestial::Catalogue` radius) or outside
  `solarsystemcontent.radius`. **Falsified by** a warp that lands inside a
  body, or a site that sits on a gate or a station grid.
- **Rooms after the first:** the anchor plus a seeded offset of 50-200 AU.
  This is only used when G6 escalations or multi-room G1 arrive. The
  destination room of a gate is the next room id, as
  `dungeon-model.md` §9.1 infers **[I]**.
- **Signature "deviation"** (the `SignatureInfo` field) is a scan-lane
  concern (`scan.rs` `certainty`). The generator emits the true position
  only.

### 4.4 Output record, shape S1

```
Site { site_id: i64, instance_id: i64, target_id: "ABC-123",
       system: i64, dungeon_id: i64, archetype: i64, difficulty: i64,
       faction: Option<i64>, entry_type: i64, scan_attr: i64,
       anchor: [f64;3], slot: (arch, entry, k), generation: u32,
       state: Dormant|Live|Completed{at}|Expired{at} }
```

- `scan_attr` comes from the archetype by the client's table **[V]**:
  24→1136, 27→211, 30→209, 44→210, 45→208. For 28 it is **unknown**, because
  the client's table has no ice row; ice belts are anomalies, which the
  scanner shows without probing.
- **This is everything** the C2 shapes (`CosmicAnomalyInfo` and
  `SignatureInfo`, listed in §1) need. `dungeonNameID`, `allowedTypes` and
  `entryObjectTypeID` come from the dungeon row at emit time.
- **Ids, rule R3, [A]:**
  - `site_id`/`instance_id` = 8,000,000,000 + seq;
  - the instance's objects take `8,100,000,000 + seq·1,000 + n`, because the
    largest dungeon holds 813 objects [V-data];
  - `target_id` is three letters and three digits from the seed, unique per
    system.
  - This shares `npc::MIN_NPC_ID`'s risk (ids above 2³¹) and is falsified by
    the same thing.

### 4.5 Instantiation: when a pilot arrives (lazy)

A `Live` site is **only a record** (shape S1) until a character is within
`dungeon::BELT_RANGE` of its anchor. On that **first arrival** the server
instantiates room 0, in four steps:

1. **One ball per object**, at `anchor + object.position` with
   `object.radius` and `object.typeID`. The objects are the entry beacon
   (`entryTypeID` at the `entryObjectID` object), structures, clouds and
   rocks. Rocks are put in `asteroid::Field`, with the quantity taken from
   the radius by `asteroid::Law`'s inverse **[V]**. Every ball goes out in
   **one** `AddBalls2` on the clock, as `dungeon::announce` does.
2. **One NPC group per `entity` without `triggerSpawn`**, placed at its
   object ± `guardCommand.jitterRadius` and orbiting it at
   `guardCommand.orbitRange` **[V]**. The ships are not in the corpus: until
   C6 delivers fleets, a group is
   `npc::faction_for(system).of_size(size_for(difficulty))`, two or three
   hulls **[A]**.
3. **Trigger-gated entities** (9,355 of 30,610 corpus-wide [V-data]) spawn
   when the first ungated group is wiped. This is **[A]**, a stand-in for the
   missing trigger instances (`dungeon-model.md` §5.3).
4. **An in-pocket respawn** of a killed group takes
   `U(entityMinRespawnDelay, entityMaxRespawnDelay)` **[V]** per object.
   `entitySpawnOnce` objects never respawn [V].

**Why lazy:** 5,485 k-space systems × about 6 sites is about 33,000
records, cheap as records. Instantiated, they would be about 1.5 M balls,
which is pointless with a few dozen pilots.

---

## 5. What the client asks, and where the generator answers

The calls are C2's to shape. C3 only commits to having the data ready:

| client | wants | generator answers from |
|---|---|---|
| `dungeonInstanceCacheMgr.GetCombatAnomalyInstances` / `…Count` (`anomalyTracker.pyc:235-238`) | the system's arch-24 anomalies | `Site`s with `archetype == 24 && entry == anomaly && state == Live` |
| `GetOreAnomalyInstances` / `…Count` / `…CountInRange` (`:255-261`) | arch 27 | same filter, arch 27 |
| `GetIceBeltInstances` / `…Count` (`:245-248`) | arch 28 | same filter, arch 28 |
| probe scan results, scan groups 3 (signatures) and 8 (anomalies) | per-site position and strength | `Site.anchor`, `scan_attr`, `difficulty` (scan lane) |
| arriving at an ore site | depletion banner | `asteroid::Field` counts versus `asteroidcountbytypeidroomiddungeonid` (§7.1 law) |

---

## 6. Lifecycle: completion, despawn, respawn

The states:

```
Dormant --(rollover or slot free)--> Live --(completed)--> Completed --(respawn_s)--> [slot refills, generation+1]
                                      |--(unvisited for ttl)--> Expired --> [slot refills, generation+1]
```

- **Completed**, [A]:
  - **G1 (combat):** every NPC of the instance is gone from the roster
    after at least one kill. This is `dungeon::cleared`, the same test
    `step` uses.
  - **G2 and G3 (ore, ice):** total rock count ≤ 10% of the dungeon's
    authored count. That is the inverse of the client's "largely untouched"
    0.9 threshold (`mining_util.pyc:104`).
  - **G4 (gas):** every cloud is depleted.
  - **G5 (data, relic):** every container is hacked. Until hacking exists,
    a G5 site never completes and only expires.
  - **Falsified by** "it vanished while I was still there".
- **Despawn on completion** happens after a grace period: the instance stays
  in space until no character has been within `BELT_RANGE` for
  `VACANT_DESPAWN`. It then leaves as one `RemoveBalls` (the
  `dungeon::empty` path), and its rocks and NPCs go with it. The record moves
  to `Completed{at}`, and the scanner stops listing it at once.
- **Respawn:** the slot refills `respawn_s` after completion (T1's column):
  1,200 s for combat anomalies, 3,600 s for the rest, 86,400 s for ice. The
  site moves: a new anchor and usually a new dungeon, through `generation`.
  **[A]**; falsified by "the same Haven keeps reappearing in one spot", which
  means `generation` is not reaching the seed.
- **Expiry:** a `Live` site nobody has entered expires at the next daily
  rollover after `ttl` = 3 days **[A]**, so an unvisited universe slowly
  reshuffles. The rollover itself is C7's.
- **Persistence** ([I] that C7 will want it): the minimal state is
  `(slot, generation, state, at)` per site. Everything else is a pure
  function of the seed, so a restart rebuilds the identical universe from
  about 33,000 small rows.

---

## 7. Belts: contents, depletion, regrowth

### 7.1 What exists, and the client's own grading

- **Contents are done** (`asteroid.rs`, `tools/asteroids.py`): the counts are
  the client's exactly, and the layout and radius law are already in. **No
  change.**
- **The client's depletion banner** (`mining_util.get_depletion_status`,
  `mining_util.pyc:92-109`) **[V]**, with `ratio` = rocks of the belt's
  types in the ballpark divided by the static count:
  - `ratio >= 0.9` and no type at zero → `LARGELY_UNTOUCHED`;
  - `>= 0.3` → `MODERATELY_MINED`;
  - `> 0` → `NEARLY_DEPLETED`;
  - otherwise fully depleted.

  It counts **rocks, not ore**, so depletion that the client can see means
  rocks leaving the field.

### 7.2 Ore tier by band: derivation rule R4

An ore type's band is the lowest band in which it holds more than 1% of its
belt rocks [V-data, `content-C3-bands.tsv` §ore]. An ore anomaly's band is
the highest band among its ores, or `special` when any ore occurs in no belt.
This rule is what T1's `*` uses.

### 7.3 Downtime reset (owner decision 2026-11-03; replaces rule R5)

- **Before:** a depleted rock left the field, and the old rule R5 regrew
  25 % of a belt's gone rocks a day; a partly mined rock kept its delta
  forever, so a nibbled belt never recovered.
- **Now:** at the daily rollover (C7) **every belt resets whole**. Every
  `rock` delta (mined-out and partly mined) is dropped, and the rocks of a
  laid-out system are put back whole at their original id, position and
  quantity (`asteroid::on_rollover`). `REGROW_FRACTION` is gone.
  - Deterministic, because the id and position of rock `n` are pure
    functions of `(belt, n)`.
  - A client already in the system keeps a partly mined rock's old radius
    until it reloads the grid **[A]**; rocks that were gone are announced.
- **Persistence of depletion** ([I], for C7): per belt, the set of missing
  `n` plus the remaining quantity of each partly-mined rock. That is a few
  bytes per touched belt.

---

## 8. Belt rats: extend `tick_belts`, keep its shape

### 8.1 Faction by region: author table T2 `rat-factions.tsv`, seeded by rule R6

- **The client has no region→pirate table** (`npc.rs` `faction_for`
  says so; confirmed here). What it does have **[V-data]** is pirate-owned
  NPC stations, by owner corp → `npccorporations.factionID`:
  - Curse: Angel 56, Serpentis 21;
  - Stain: Sansha 75;
  - Delve: Blood Raiders 13;
  - Fountain: Angel 8, Serpentis 4;
  - Venal: Guristas 9.

  That is 186 stations in five regions.
- **Rule R6:**
  1. **NPC-null regions** with pirate stations: the station-owner
     distribution as weights **[V-data]**.
  2. **Other null regions** (the region's `factionID` is empty): the
     faction of the nearest pirate-station system by stargate hops. A BFS
     over the 13,978 gate edges in `solarsystemcontent.stargates` gives,
     across all null systems, Angel 1,372, Guristas 1,106, Sansha 503, Blood
     Raiders 193, Serpentis 121, and 257 unreachable (Pochven, Jove and the
     like) **[I]**.
  3. **Empire regions, by the region's empire `factionID`** **[A]**, CCP
     lore and not client data:
     - Caldari 500001 → Guristas 0.7, Serpentis 0.3;
     - Gallente 500004 → Serpentis 0.7, Guristas 0.3;
     - Minmatar 500002 → Angel 0.7, Blood Raiders 0.3;
     - Amarr 500003 and Khanid 500008 → Sansha 0.5, Blood Raiders 0.5;
     - Ammatar 500007 → Blood Raiders 0.5, Angel 0.5.

     **Falsified by** a player who knows TQ. Do **not** use nearest-station
     BFS for empire space: it gives Angel to 893 of 1,246 hisec systems,
     The Forge included, which is wrong.
  4. **Unreachable or special** (Pochven, Jove, Yasna Zakh, Exordium): no
     belt rats.
- **The tool writes T2 once and the repo keeps it.** It is about 68 rows of
  `regionID, factionID, weight`; region ids are facts, the weights are ours.
- **Code impact:** `npc::FACTIONS` gains Guristas, Serpentis, Sansha and
  Blood Raiders. Each is a `Faction` whose types are picked by group, the
  same way `ANGEL_CARTEL`'s are; the picks are C6's. `faction_for(system)`
  reads T2 by the system's region and picks by `roll(belt_id, spawns)`.

### 8.2 Composition by band ([A])

`COMPOSITIONS` becomes per band. The current list stays as hisec; lowsec
adds cruiser-heavy groups; null adds battlecruiser and battleship groups
once `Size` grows them. Null belts are small (median 21 rocks) and their
rats are big, which is a [V-data] fact about belts and an [A] about the
rats. **Falsified by** "a battleship in a 1.0 belt", or "null belts feel like
hisec".

### 8.3 Timing: CCP's spawner vocabulary, read from the corpus

These are `reactiveSpawnComponent` values across 882 types, **[V-data]**:

| field | dominant value | what we would do with it |
|---|---|---|
| `threatTimeoutSeconds` | 900 on 874 of 882 | **raise `dungeon::VACANT_DESPAWN` from 60 s to 900 s** [I]: CCP keeps a spawn tracking a threat for 15 min after it leaves. Falsified if 900 leaves too many idle balls in our tick |
| `sensorUpdateTimeSeconds` | 60 on 776 | evaluate a belt's spawn decision every 60 s, not every 4 Hz tick; despawn and kill bookkeeping stay per tick [I] |
| `threatRangeMeters` | 250,000 on 681 | `BELT_RANGE` 75 km stays (a belt is smaller than a dungeon grid) [A] |
| `minSpawnIntervalSeconds` / `max…` | spread widely: 5 to 86,400 s, 49 distinct values | no single value applies; keep `RESPAWN_AFTER` 5 min [A] |
| `minPointAmount` / `initialPointAmount` / `maxPointAmount`, `spawnPointThreshold` | points; 0.01, 1.0 and 0.5 are the common thresholds | a future "match the fleet" rule: spawn points = f(pilots present). Not in v1 |
| `shouldMatchPlayerPoints` | true on 1 of 882 | CCP almost never scales to the fleet; neither do we |
| `spawnTableID` | 505 distinct; `fleetCounterTableID` 83 distinct; `initialSpawnNpcFleetTypeID` 72 distinct | **targets not in the corpus** (C6's gap). The 504 spawn tables in the brief are these |
| `ownerID` | 500021 ×137, 500027 ×99, 500024 ×83 … | none of the five belt pirates owns a reactive spawner: **belt rats are not reactive spawns in this build** |

The only belt-related reactive rows are "Asteroid Belt Remnants Dungeon -
Miners Spawner S/M" (90003, 90031), whose owner is the NPC corp 1000129,
with 2,000-3,000 s intervals and a fixed 6 or 15 points. Those are *friendly
miners* in an ore dungeon, which is useful for the "inhabited" feel later
(§11, packet P-C3e).

---

## 9. Sizing: 2-10 humans plus about 50 bots

- **Records:** about 33,000 `Site` rows over k-space, and about 41,000
  `Belt` rows, which are created only when touched (as today).
- **Live balls**, only near characters: one instantiated combat anomaly is
  about 44 objects plus about 7 groups of 2-3 NPCs, roughly 60 balls. With
  60 actors spread over about 20 systems and about 1 site each, that is
  about 1,200 site balls, plus belt rocks at about 100 per hisec belt per
  system bootstrap. That is within what `asteroid.rs` already ships.
- **Ticks:** the slot bookkeeping runs at the 60 s spawn cadence (§8.3),
  only for systems with an actor or a live instance. Generation is
  O(slots); the draw is a hash plus an index.
- **Bots:** an `eve-bot` "ratter" or "explorer" reads the same
  `dungeonInstanceCacheMgr` answers a human does. Contention is solved by
  counts, not locks. When bots in a system outnumber its slots, T1's counts
  are the knob. **[A]**; C7 owns fairness.

---

## Proposals

1. **Adopt one generator for G1-G5**: a pure `populate(system, day, gen) ->
   Vec<Site>` over the corpus's archetypes, with the client's
   `entryTypeID` deciding anomaly or signature and its scan table deciding
   the probe class. No hand-written site list.
2. **Adopt T1 `population.tsv` as the single authored knob** for counts,
   difficulties and respawn per band. It holds no client rows, and an
   operator edits it.
3. **Adopt "difficulty is the band"** ({1, 2} high, 3 low, {4, 5} null)
   for exploration sites, and R4 for ore anomalies [I], instead of hand
   lists.
4. **Lazy instantiation:** sites are records until a character is within
   `BELT_RANGE`, then room 0 is placed from the dungeon's own objects in one
   `AddBalls2`.
5. **Determinism by `(system, slot, generation)` seeds**, with
   `generation` persisted. A restart reproduces the universe; a completion
   moves the site.
6. **Use the corpus's in-pocket respawn delays** (`guardCommand
   entityMin/MaxRespawnDelay`, `entitySpawnOnce`) verbatim; they are [V].
7. **Raise belt-rat vacancy to 900 s and decide spawns on a 60 s cadence**,
   taking CCP's dominant `reactiveSpawnComponent` values [V-data/I].
8. **Rat faction by region from T2, seeded by R6**: pirate-station
   ownership in NPC null, gate-BFS in sovereign null, the lore pair by
   empire faction in empire space. Never nearest-station in empire space.
9. **Belt regrowth of 25% of the static count per day at the rollover**,
   rocks returning by ordinal at their original id and position.
10. **Ids in their own block** (8.0e9 for sites, 8.1e9 + seq·1,000 for
    instance objects), next to the NPC block and sharing its falsifier.

## Packets

**P-C3a, the extractor:**
- **Files:** `tools/content.py` (or C1's unified extractor), `sites`
  sub-command.
- **Writes:**
  - `out/content/sites.tsv`, columns `dungeonID archetypeID difficulty
    factionID entryTypeID entryObjectID dungeonNameID rooms objects
    entities ore ice gas ore_band`, restricted to archetypes {24, 27, 28,
    30, 44, 45} after §4.2's filters;
  - `out/content/site-objects.tsv`, columns `dungeonID roomID objectID
    typeID x y z radius yaw pitch roll aggressionRange orbitRange
    jitterRadius entMinResp entMaxResp spawnOnce objTrigger entities
    trigger_entities`;
  - `out/content/rat-factions-seed.tsv`, R6's computed weights, so the
    operator can promote it to T2.
- **Tests (python, in the tool's self-check):** row counts per archetype
  equal the dungeon table's; every `entryObjectID` is an object of its
  dungeon; the object counts per room reproduce
  `asteroidcountbytypeidroomiddungeonid` for the ore rows.

**P-C3b, the authored tables:**
- **Files:** `server/content/population.tsv` (T1) and
  `server/content/rat-factions.tsv` (T2), plus a loader in a new
  `server/crates/eve-server/src/population.rs`.
- **Tests:** a malformed row is skipped; an absent file means "no sites"
  and the bootstrap is unchanged; a band with no row yields no slots.

**P-C3c, the generator:**
- **Files:** `population.rs` (new; it owns `Site`, `populate`, the
  lifecycle state machine and the ids).
- **Tests (none asserts an [A] number):**
  - deterministic: the same inputs give identical sites;
  - count per (band, archetype, entry) equals T1's;
  - every drawn dungeon satisfies the §4.2 filters (archetype, entry type,
    difficulty set, faction);
  - ids are unique, inside the block and disjoint from `MIN_NPC_ID..` and
    the rock range;
  - a completed slot refills only after `respawn_s` and with `generation`
    + 1;
  - an expired slot refills at rollover;
  - an empty candidate set gives an empty slot without a panic;
  - `target_id`s are unique per system.

**P-C3d, instantiation:**
- **Files:** `dungeon.rs`, after W10 lands. It adds `instantiate(site)`
  next to `spawn`, and its rocks go through `asteroid::Field`.
- **Tests:**
  - every object of room 0 becomes a ball at `anchor + position` with its
    radius and typeID, in one `AddBalls2` on the clock;
  - `every_slim_has_a_ball` still holds;
  - ungated entities spawn on arrival and gated ones only after the first
    wipe;
  - `entitySpawnOnce` objects never respawn;
  - an in-pocket respawn lands inside the authored [min, max] delay;
  - completion followed by vacancy gives one `RemoveBalls`;
  - an ore instance's rock quantities round-trip through `asteroid::Law`.

**P-C3e, belt changes:**
- **Files:** `dungeon.rs` (`tick_belts`: the T2 faction, per-band
  `COMPOSITIONS`, a 60 s decision cadence, `VACANT_DESPAWN` from a named
  constant), `npc.rs` (four more `Faction`s, picks per C6) and `asteroid.rs`
  (`regrow(fraction)`, called by C7's rollover).
- **Tests:**
  - a system's rats are from its T2 faction;
  - a band with no composition falls back as today;
  - regrowth restores missing rocks by ordinal at their original id and
    position, never above the static count, and a partly-mined rock keeps
    its radius;
  - a spawn decision is not re-taken inside the cadence.

**Ownership note:** P-C3d and P-C3e both touch `dungeon.rs`. Land P-C3e
first, or split the belt half into `belts.rs` first (lane rule 2).

## unknown

1. **Ice belts' scan attribute.** The client's site-type table has no ice
   row (`explorationSites.pyc:23-29`).
2. **The placement of landmarks** (archetype 2, 1,200 dungeons, Celestial
   Beacon entry) and of agents-in-space sites (22): they have fixed systems
   on TQ, and no system field here.
3. **What `npcGroupingID`, `spawnTableID`, `fleetCounterTableID` and
   `initialSpawnNpcFleetTypeID` point at.** This is C6's gap, and it caps
   G1 fidelity.
4. **The trigger instances** that gate 9,355 entities; §4.5's
   "after the first wipe" is a stand-in.
5. **Real per-system site counts and the respawn cadence on TQ.** None of
   it is in the client; T1 is a guess to be tuned.
6. **Whether the client cares which room-0 object is the warp-in.**
   `entryObjectID` exists for 5,499 dungeons; that warping to a site lands
   on it is [I].
7. **Scan strength per difficulty.** `difficulty` maps to `scanDifficulty`
   1-5 by name, but the signal-strength formula is server-side (`scan.rs`
   [A]).
8. **Wormhole signatures** (archetype 38) and their statics: they are
   absent from the corpus and from EvEJS (`evejs-data.md` §4).
9. **The archetype titles** resolve for 33 of 71 rows. The rest may sit at a
   different offset, or simply be unset; this was not checked further.
