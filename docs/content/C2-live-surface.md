# C2: the live-content surface of the core

Lane C2 of `docs/content/README.md` §3. This is a read-only research lane: it
wrote no code and ran no cargo. The question it answers is what the client
asks for, or expects to be pushed, before it will *show* a living universe.
For each surface it gives the call or push and its shape, what the server
answers today, and what happens if the server says nothing.

**Method.**
- The client sources were read decompiled (uncompyle6, in the analysis
  image, into a scratch copy that has since been deleted).
- **Every line number is a `co_lnotab` line from `tools/dis.py`**, never an
  uncompyle6 line. The drift is real: daily goals' `_send_request` is line
  142 in uncompyle6 and 199 by `co_lnotab`.
- Paths are `out/pyc/...` with `.pyc` dropped. `agencyNew/` stands for
  `eve/client/script/ui/shared/agencyNew/`.
- "Today" was read from `server/crates/eve-server/src`, mostly
  `service/space.rs` and `service/live.rs`.

**Ranks if the server says nothing:**
- **BREAKS**: a traceback that escapes, a live lock, or a UI that will not
  draw.
- **DEGRADES**: the feature partly works and logs an exception.
- **EMPTY**: it silently shows nothing, which is true of an empty universe.

**Marks:** [V] read from this build, [V2] EvEJS, [I] inferred (with the
reason), [A] an assumption we would ship (with what falsifies it), or
unknown.

The per-surface rows are also in `findings/content-C2-surface.tsv`.

---

## 0. The answer in one table

The table is ordered by **value to the end goal** (co-op and PvP with
friends plus bots, in a universe that feels inhabited), and within that by
how badly silence hurts.

| # | Surface | Transport | Today | If silent | Value |
|---|---|---|---|---|---|
| 1 | Sensor overlay, probe scanner sites, warp to site | macho push + bound call | `GetFullState` → 4 empty dicts; no push | **EMPTY** | **highest**: every PvE site and every "where are the ratters" hunt starts here |
| 2 | Site entry and objectives (dungeon tracking) | macho push | nothing | **EMPTY** (DEGRADES once sites exist without `dunObjectID`) | high: the objective panel inside each site |
| 3 | Agency site lists (combat, ore, ice, signatures) | macho | Instances `{}`; **Count and `GetSignatureInstances` `None`** | **BREAKS (bounded)**: a 5 s stall plus an exception | high: where friends and bots pick content |
| 4 | Factional warfare: warzones, occupation, victory points | macho | local `(ssid, None)` and all `{}`; **`GetAllWarzones`, `GetHQSystemIDs`, battlefields and VP unanswered** | **BREAKS** the FW dashboard; VP breaks inside warzones | high: the only built-in "two sides fight over systems" loop |
| 5 | Solar-system interference (low and null) | macho | **unrouted** | **BREAKS** the system tooltip in every low and null system | medium: low-sec is where friends fight |
| 6 | `dynamicResourceCacheMgr` (ESS and DBS settings, map, Agency) | macho | **unrouted** | **BREAKS** the map DBS filter, the ESS tooltips and the Agency ESS group | low, but answering it is cheap |
| 7 | ESS proper | macho | flag-off, `None` | **EMPTY** (CCP's own off path) | medium later: a PvP magnet |
| 8 | World-event tales | macho push + gateway | `GetGlobalWorldEventTales` `[]`; **`get_active_tales_by_template` `None`** | EMPTY / BREAKS that caller | medium: the cheapest "the universe did something" |
| 9 | Epic arcs | macho | status `{}`; **`GetMessagesForEpicArcMissions` `None`** | **DEGRADES** (the Agency tab is empty and logs) | low-medium (C4) |
| 10 | Incursions | macho push | `map.GetIncursionGlobalReport` `[]`; **`rewardMgr` unrouted** | EMPTY / BREAKS the LP-incursion panel | low now (group PvE, later) |
| 11 | Pirate insurgency, corruption and suppression | macho + **gateway** | empties | EMPTY, plus a 10 s gateway timeout | low (needs FW and the gateway) |
| 12 | New-activities news (`newactivities`) | macho | **unrouted `activityMgr`** | **BREAKS** (`set(None)`) once a row is in its date window | none (fix it anyway: size S) |
| 13 | Seasons and challenges | macho | `None` (CCP's off) | EMPTY | low (the only macho challenge vehicle) |
| 14 | Login rewards, seasonal login, milestones | macho | CCP's off values | EMPTY | none |
| 15 | Achievements, daily, career and corp goals, campaigns | **gateway only** | not served | EMPTY after a timeout; achievements DEGRADES | low until P6.2 |
| 16 | Agents in space, void space, operations (NPE) | macho | none / `False` gate | EMPTY | low; skip |
| 17 | Pirate strongholds (`custom_agency_provider`) | macho | **unrouted** | DEGRADES (uncaught `TypeError` from `len(None)`) | none (answer `[]`) |

**The ranking that decides what we build:**
1. Rows 1–3 are one feature: **sites exist**. C3's generator places
   them, this surface shows them, and nothing else is needed for "the
   universe has content".
2. Rows 3, 4, 5, 6, 8, 12 and 17 contain **live breakers**. Each one is a
   size-S typed-empty answer, independent of any content work.
3. Row 4 is the first *populated* surface worth building after sites.

---

## 1. Cross-cutting facts

**1.1 Two transports, and only one is ours.** [V]
- MachoNet carries every surface above except the gateway-only ones:
  achievements, daily goals, career goals, corp goals, campaigns, the
  corruption and suppression numbers, and tale start/stop notices.
- Those go through `publicGatewaySvc` (protobuf, a 10 s local deadline,
  `docs/guides/debugging-the-client.md` §3c).
- Package census, `grep RemoteSvc|ConnectToRemoteService|moniker` against
  `_pb2|publicGateway`:

  | Packages | machoNet files | gateway files |
  |---|---|---|
  | `achievement_system`, `careergoals`, `campaigns` | 0 | 18, 9 and 28 |
  | `probescanning`, `explorationscanner`, `objectives`, `pirateinsurgency` | 0 | 0 |

  The last four reach the server through `eve/client/script` services and
  pushes. [V]
- **The feature flags do not switch the gateway features off.**
  LaunchDarkly is NXDOMAIN in the jail, so every flag takes its
  `default_value`, and `daily-goals-enabled-feature-flag`,
  `corp-goals-enabled-feature-flag` and
  `air_career_program_progression_claiming_enabled` all default to `True`.
  [V] (`featuremanagement/client/flag`)

**1.2 Most live-content state objects are new-style classes in
`globalsWhitelist`.** [V] (`server/crates/eve-rowset/src/whitelist.rs`)
- Site info: `explorationscanner.common.{cosmicAnomalyInfo.CosmicAnomalyInfo,
  signatureInfo.SignatureInfo, structureInfo.StructureInfo, ...}`.
- Factional warfare: `factionwarfare.{occupationState.OccupationState,
  victoryPointState.VictoryPointState, advantageState.AdvantageState}`.
- `pirateinsurgency.campaignClientSnapshot.CampaignClientSnapshot`,
  `solarsysteminterference...SolarsystemInterferenceState` and
  `seasons.common.challenge.Challenge`.

**Wire form:** a `0x23` ObjectEx with header
`((Global(cls),), state_dict)`. The client runs `cls.__new__` and then
`__dict__.update`, and **`__init__` and its validators never run**, so the
server must uphold them.
- **The server can already emit this.** `skill.rs:795`
  `SkillEntry::to_pyvalue` builds `PyValue::ObjectEx { form:
  ObjectForm::New, ... }` for `CharacterSkillEntry`, and that works against
  the client. [V]
- What is missing is a **shared helper**, so each lane does not hand-roll
  one: `pyutil::new_style(class, state)`. *(The C2 sub-pass that read factional
  warfare called this "a missing builder"; it is a missing helper, and the
  encoding is proven.)*
- State keys are the instance `__dict__` names:
  - the explorationscanner classes use **plain names** (`position`,
    `targetID`, ...) [V: their `__init__` bodies];
  - factional warfare, insurgency and interference use **underscored**
    names (`_occupierID`, ...) [V: their `__init__` bodies].

**1.3 The Agency's base provider decides most Agency ranks.** [V]
- `agencyNew/contentProviders/baseContentProvider:60-71` builds pieces
  inside `except (AttributeError, KeyError, IndexError)`, with a `finally`.
- A `None` used as a dict or object is caught: the tab is empty and one
  exception is logged (DEGRADES).
- `len(None)`, `sorted(None)` and `set(None)` raise `TypeError`, which
  escapes.
- No provider live-locks, because `contentPieces` becomes `[]` and not
  `None`.

**1.4 Pushes are cheap to add.** A new push is a type in the module that
owns it (`src/notify.rs`, `docs/lanes/README.md` rule 2). Every push below
is narrowcast to characters or singlecast to a solar system.

---

## 2. Sites: the sensor overlay, the probe scanner and system scan results

**The best evidence in the build:**
`explorationscanner/signalTracker` is **CCP's own server-side
implementation**, shipped in the client tree. [V] Read it as the spec; what
follows is that file plus the client consumers.

### 2.1 The flow the real server runs [V] (`explorationscanner/signalTracker`)

1. **Registration.** The client sends `scanMgr.SignalTrackerRegister()` on
   every `OnEnterSpace` (`sensorSuiteService:214`).
   - The server adds the character to `newClients`.
   - In space the client **never asks for the state itself**; it waits for
     the push.
   - `GetFullState` is pulled only from:
     - the docked path (`anomalyTracker.get_sites` →
       `FullyUpdateSignalTrackerDebounced`, `sensorSuiteService:897`);
     - the Agency signature piece (`UpdateSignalTracker`).
2. **Update tick.** `DoUpdateTick` narrowcasts
   `OnSignalTrackerFullState(solarSystemID, fullState)` to the new clients,
   then moves them to `activeClients`. On every tick it also narrowcasts the
   batched deltas to active clients:
   - `OnSignalTrackerAnomalyUpdate(ssid, added: {siteID: CosmicAnomalyInfo}, removed: iterable of siteID)`;
   - `OnSignalTrackerSignatureUpdate(ssid, added: {siteID: SignatureInfo}, removed)`;
   - `OnSignalTrackerStructureUpdate(ssid, added: {structureID: StructureInfo}, removed)`.
3. **Singlecasts to the whole system** (`SinglecastBySolarSystemID2`), the
   moment an instance appears:
   - `OnCosmicAnomalyAdded(KeyVal(**anomalyInfo.__dict__))`, which only
     drives a highlight swipe (`scanResults`, `.targetID`);
   - `OnCosmicSignatureAdded(targetID)`;
   - `OnCosmicAnomalyRemoved(instanceID, dungeonID, ssid)`.
4. **`fullState`** is `(anomalies, signatures, staticSites, structures)`,
   four dicts keyed by **site ball itemID**.
   - The consumer unpacks it at `sensorSuiteService:615`
     (`UNPACK_SEQUENCE 4`) and iterates each part with `.iteritems()`.
   - `({}, {}, {}, {})` is CCP's own answer for an overlay-suppressed
     system.

### 2.2 The value shapes (the class `__init__` bodies plus the consumer lines) [V]

| Dict | Class (whitelisted) | State keys (plain names) | Consumer |
|---|---|---|---|
| anomalies | `CosmicAnomalyInfo(SiteInfo)` | `position` (x,y,z tuple), `targetID` (`'ABC-123'`), `difficulty` (int ≥ 1; `sensorsuite/common.MapDungeonDifficulty`), `dungeonID`, `archetypeID`, `siteID`=`instanceID`, `instanceID`, `dungeonNameID`, `factionID`, `scanStrengthAttribute`, `allowedTypes`, `entryObjectTypeID`, `solarSystemID` | `sensorsuite/overlay/anomalies.GetSiteData:66-75` reads the first eleven |
| signatures | `SignatureInfo(SiteInfo)` | `position` (**true position plus a random offset of up to `deviation`**), `targetID`, `difficulty`, `dungeonID=None`, `archetypeID=None`, `deviation` (metres) | `signatures.GetSiteData:150-155` |
| staticSites | `StaticSiteInfo` | `position`, `dungeonNameID`, `factionID` | `staticsites.GetSiteData:64`; warp is `WarpToItem(siteID)` |
| structures | `StructureInfo` | `typeID`, `groupID`, `categoryID`, `position`, `targetID` | `structures.GetSiteData` |

**The deviation rule**, CCP's own (`signalTracker.CalculateDeviationForSignal`)
[V]:
- `s = sigRadius / scanStrength`;
- if `s < 2`, the deviation is 8 AU; if `s > 20`, it is 4 AU;
- otherwise it is linear between them.
- The offset is `RandomVector(random() * deviation)`, cached per site, so
  every pilot sees the same blurred circle.

### 2.3 Where the sites go client-side [V]

- **Overlay brackets.** `siteController` turns each value into a SiteData.
- **Probe scanner result list.** `ScannableSiteHandler.ProcessSiteUpdate`
  calls `sensorSuite.InjectScannerResults`, and `scanSvc:339` injects
  `probeScannerController.GetAllSites()`. So **anomalies appear in the probe
  window without probing**, and signatures appear unidentified.
- **Agency trackers.** `AnomalyHandler.ProcessSiteUpdate` calls
  `tracker.scan_results_updated` on every `dungeonTracking` tracker whose
  `ARCHETYPE_IDS` changed (`anomalies:62-80`). So the local-system rows of
  §3 are fed by this push, not by `dungeonInstanceCacheMgr`.
- **Warp.** `menusvc._WarpXToScanResult:1697-1704` resolves the target id
  through `sensorSuite.GetPositionalSiteItemIDFromTargetID:995-999`, which
  searches **ANOMALY and STRUCTURE** sites only.
  - An anomaly is warped to as `CmdWarpToStuff('item', siteBallID)`.
  - A signature is warped to as `CmdWarpToStuff('scan', targetID)`, so the
    server must map a signature's `targetID` to its ball once the pilot has
    a perfect probe result.
  - The server's `CmdWarpToStuff` and the `'scan'` subject already exist
    (`scan.rs:75-79`, `:1154`). [V]

### 2.4 What the server needs (spec)

- **A site is a ball.** The real tracker drops any instance whose
  `signatureID` slim is not group **885** (`groupCosmicAnomaly`, type
  `typeCosmicAnomaly` 28356) or **502** (`groupCosmicSignature`). [V]
  (`inventorycommon/const`) Landmark beacons are group **310**
  (`groupBeacon`, type 10124). [V]
  - [I] The client need not have the ball in its own ballpark, because the
    position travels in the info object and warp resolves server-side.
  - So a site is a **server-side item with a position**, not necessarily a
    Destiny ball pushed to anyone.
  - [A] We register it in the system's item table and push it as a ball
    only to pilots in its bubble. *Falsified if* warp-to-`item` needs a
    client ball.
- **Server does** `SignalTrackerRegister`. It marks the character as new
  and, on the next world tick (or immediately), narrowcasts
  `OnSignalTrackerFullState(ssid, (anoms, sigs, statics, structs))`. Each
  value is `pyutil::new_style("explorationscanner.common.<mod>.<Class>",
  plain-name state)`.
- **Server does** `GetFullState`: the same tuple, synchronously. It is
  already routed (`service/space.rs:515`) and only needs filling.
- **Server does**, on instance spawn and despawn (C3's generator):
  - accumulate deltas and narrowcast
    `OnSignalTracker{Anomaly,Signature}Update(ssid, added_dict, removed_list)`
    to the system's registered characters once per tick;
  - singlecast `OnCosmicAnomalyAdded(KeyVal)`, `OnCosmicSignatureAdded(targetID)`
    and `OnCosmicAnomalyRemoved(instanceID, dungeonID, ssid)` to the system.
  - [I] The client iterates `removed`, so a list will do in place of CCP's
    `set`.
- **Probe results for signatures.** Per `docs/lanes/scan.md` §5, these are
  `scanGroupID` 3 for signatures and 8 for anomalies, with
  `strengthAttributeID`, `dungeonNameID` and `archetypeID` disclosed by
  certainty. Signature strength comes from the dungeon's scan-strength
  value and `signatureRadius` (the tracker's `scanStrengthValue`), with
  `difficulty` from `dungeons.difficulty`.
- **Instance record C3 must produce** (one per live site):
  `{instanceID, signatureID (site item id), dungeonID, solarSystemID, position, isScannable, scanStrengthAttribute, scanStrengthValue, signatureRadius, targetID}`.
  - `dungeonNameID`, `archetypeID`, `factionID`, `difficulty` and
    `entryTypeID` are read from `dungeons` at emit time, so they are not
    stored.
  - `allowedTypes` is the dungeon's ship restriction: `connections[].allowedShipsList`
    → `typelist` (`docs/re/dungeon-model.md`). [I] The first gate's list
    applies; otherwise `None`.
- **Target ids.** These are `ABC-123` strings from the same scheme the
  probe lane already mints (`scan.rs`). They are stable per instance, and
  [A] seeded by `instanceID`, as CCP's `GetScanTargetID(..., targetSeedID=instanceID)`
  suggests.

**If silent:** EMPTY. No overlay, no anomalies in the probe window, and
nothing to warp to. Nothing throws. **Today:** `GetFullState` answers four
empty dicts and no push exists.

---

## 3. Agency site lists for other systems: `dungeonInstanceCacheMgr`

The `evedungeons/client/anomalyTracker` trackers (combat, ore, ice, Bulwark,
factional warfare, homefront, insurgency and Triglavian) each fetch
`{solarSystemID: [site rows] | int}`. An `int` means *n unknown sites*
(`_unpack:122-127`). [V]

| Call | Today | If silent |
|---|---|---|
| `Get{CombatAnomaly,IceBelt,OreAnomaly,FactionWarfare,HomefrontSite,PirateInsurgency,TriglavianSite}Instances`, `GetBulwarkMiningSiteCounts` | `{}` (`service/space.rs:116-123`) | fine |
| **`GetCombatAnomaliesCount`, `GetIceBeltsCount`, `GetOreAnomaliesCount`** (`anomalyTracker:238,248,258`) | `None` | **BREAKS, bounded.** `get_unknown_sites` unpacks at `:221`, outside its `try`, so `None.iteritems()` raises. The `_fetch_time_unknown` reset is skipped, and the next reader spins `Yield()` for up to 5 s (`:207-222`). [V] |
| **`GetOreAnomaliesCountInRange`** (`:261`) | `None` | stored and returned `or {}`. Answer `{}` |
| **`GetSignatureInstances`** (`contentProviderSignatures:22`, `mapFilterSignatures:7`) | `None` | DEGRADES (caught `AttributeError`) |
| **`GetFactionWarfareBattlefieldInstances`** (`fwWarzoneSvc:186`) | `None` | **BREAKS** the FW dashboard map (`warzoneMapPanel:543` iterates; `:553` reads `[0].dungeonNameID`) |
| `GetPirateInsurgencyIceHeistInstances` (`insurgenceDashboardSvc:282`) | `None` | DEGRADES; the container type is unknown |

**Remote row fields** [V] (`agencyNew/contentPieces/siteSystemContentPiece:48-53`,
`siteContentPiece:45,48`):
- `siteID`, `factionID`, `dungeonNameID`, `position` and `dungeonID`. The
  `dungeonID` must exist in `dungeons.fsdbinary`, because it indexes
  `GetDungeon`.
- Factional-warfare rows add `allowedTypes`.
- [A] A `KeyVal` with these fields is enough. *Falsified by* an Agency
  `AttributeError` on any other field.

**Spec.**
- Server answers every `Get*Instances` from C3's live instance table,
  filtered by archetype:
  - combat: `dunArchetypeCombatSites`;
  - ore: `dunArchetypeOreAnomaly`;
  - ice: `dunArchetypeIceBelt`;
  - factional warfare: `dunArchetypesFactionalWarfare`
    (`eve/common/lib/appConst`).
  - [I] The KeyVal rows go out for the pilot's own system; the
    `{ssid: count}` form goes out for the rest.
- Server answers the `*Count` methods with `{ssid: n}` over the same table
  (`{}` until it exists), and `GetSignatureInstances` with `{ssid: n}`.
  - Use ints and never rows here. `SignatureContentPiece` calls `.GetName()`
    and `.IsAccurate()`, which exist only on the sensor-suite site object.
    [I]
- **Policy [A]:** a pilot "knows" only the rows of the system they are in.
  Every other system is a count. This is cheap and stops bots and friends
  from seeing perfect information. *Falsified if* the Agency's distance
  filter needs remote positions; it reads `position`, so a count simply
  drops out of range sorting.

---

## 4. Inside a site: dungeon tracking, objectives and beacons

**The objective chains run client-side from static data.** [V]
`evedungeons/client/data:52-57` reads `dungeons.objectivesID`, then
`dungeonObjectives[objectivesID].clientObjectives`, which holds
`objectiveChainID`, `nodeGraphID`, `blackboardParameters` and `overrides`.
The `objectives` package makes no macho or gateway call at all.

The server only says where the pilot is, and stamps fields on slims.
`eve/client/script/parklife/dungeonTracking` [V]:

| Push | Shape | Effect |
|---|---|---|
| `OnEnteringDungeonRoom(dungeon_id, room_id, room_position, instance_id, dungeon_values)` | 5 positional arguments; `dungeon_values` is a dict whose keys are split on `.` (`:208-209`); `None` is OK | the first push builds `ActiveDungeon` (`:146`) and `start()` (`:281-291`) seeds the blackboard and starts `ObjectiveChain` plus the node graph |
| `OnDungeonValues(dungeon_id, values)` | dict | `:170-172` |
| `OnDungeonMessage(dungeon_id, key, value)` | | the blackboard key gets a `dungeon_` prefix |
| `OnDungeonCompleted(dungeon_id)` | | `dungeon_completed=True` |
| `OnExitingDungeon(dungeon_id)` | | state is cleared; a `locationid` change also clears it |
| `keeper.GetCurrentDungeonForCharacter()` on relog in space (`:92-96`) | falsy, or a 4-tuple `(dungeon_id, room_id, instance_id, dungeon_values)` | `None` today is safe: the tracker is lost after a relog |

**Objective tasks advance from ballpark events**, not from calls. [V]
- The events are `OnObjectExplode`, `OnBallAdded`, `OnDamageStateChange`
  and `OnSlimItemChange`.
- They match these slimItem fields:
  - `dunObjectID` (`objectives/client/objective_task/target:89,411`);
  - `objectiveTargetGroup` (`target:394`, `destroy_counter:236`);
  - `entityGroupID` (`destroy_counter:248`, `destroy_target:128`).

**Spec.**
- Server pushes `OnEnteringDungeonRoom(dungeonID, roomID, None, instanceID, {})`
  when a ship lands in a room (at warp-in, or on a gate jump to the next
  room).
- It pushes `OnDungeonCompleted(dungeonID)` when the last spawn group dies,
  and `OnExitingDungeon(dungeonID)` on warp-out.
- **Every slim spawned from a room object or a spawn entity carries
  `dunObjectID`** (from `docs/re/dungeon-model.md`), plus `entityGroupID` and
  `objectiveTargetGroup` where the static data has them. This is a field
  list for C6 and `npc.rs`.

**If silent:** EMPTY, with no objective panel. Once sites exist without
`dunObjectID` it DEGRADES: the chain starts and never ticks.

**Beacons.**
- Landmarks and static sites come from `keeper.GetStaticSpawns` in the real
  tracker, as `StaticSiteInfo` (§2.2), and are warped to by item id.
- In-space beacons are ordinary balls (group 310) with a slim. They show on
  the overview from `AddBalls` like any other ball. **No call is needed.**
  [I] (an overview group filter on group id)
- The rowset pushes `OnDistributionDungeonEntered(row)` and
  `OnEscalatingPathDungeonEntered(row)` take one row with `.header`
  (`dungeonTracking:107,112`). They are needed only for distribution and
  escalation dungeons. **Escalations** are already answered
  (`GetMyEscalatingPathDetails` rowset); populating them is C4 and C3 work.

---

## 5. Factional warfare and warzones: the PvP loop

**Today** (`service/space.rs:85,694-695`):
- `GetLocalOccupationState` → `(own ssid, None)`;
- `GetAllWarzonesOccupationStates` → `{}`;
- `GetMyEnlistment` → `(None, None, None)`.

All three are correct "not at war" answers.

| Call / push | Populated shape | Today | If silent |
|---|---|---|---|
| `fwWarzoneSolarsystem.GetAllWarzones()` (`fwWarzoneSvc:221`, `@Memoize`) | `{warzoneID: [factionA, factionB]}` | `None` | **BREAKS** the dashboard. `GetWarzoneIdForFaction:203-207` iterates it, then `random.choice(keys())`, so **`{}` is also an `IndexError`. The answer must be non-empty.** [V, spot-checked] |
| `GetHQSystemIDs()` (`:225`) | `{factionID: ssid}` | `None` | **BREAKS** the dashboard and the Agency FW page (`warZones:69`, `in None`) |
| `GetAllWarzonesOccupationStatesUncached()` (`:113`) | `{wz: {ssid: OccupationState}}` | `None` | DEGRADES [I] |
| `dungeonInstanceCacheMgr.GetFactionWarfareBattlefieldInstances()` | `{ssid: [row with dungeonNameID]}` | `None` | **BREAKS** the dashboard map |
| `fwVictoryPointMgr.GetLocalVictoryPointState()` (`fwVictoryPointSvc:48/51`, unpack 2) and `GetAllVictoryPointStates()` (`:59`, then `.get`) | `(ssid, VictoryPointState)`, with state `_vpScore ≥ 0`, `_vpThreshold > 0`; `{ssid: VPState}` | unrouted | **BREAKS** inside warzone systems only (`infoPanelLocationInfo:642` gates on `isFacWar`); EMPTY elsewhere |
| `facWarMgr.*` (about 28 methods through `self.remoteFacWarMgr`, `facWarSvc:83`: stats, ranks, join and leave militia, LP per system) | per method, unknown | unrouted, and **missing from `client-calls.tsv`** (a census blind spot: the attribute-held remote) | BREAKS the FW stats and rank UI [I] |
| `OccupationState` | `0x23`, with state `_occupierID`, `_adjacencyState` (in `ADJACENCY_STATES`), `_attackerID`, `_pirateFactionID`, `_dateTaken` | | |
| Pushes | `OnWarzoneOccupationStateUpdated(ssid, OccupationState)` (`:115`; a changed `occupierID` plays the flip fanfare at `:130`), `OnVictoryPointStateUpdated(ssid, VPState)`, `OnFwObjectiveCompletedInSolarSystem(ssid, targetFactionID, mode)`, `OnFwScoreboardWon(ssid, itemID, winnerFactionID)`, `OnFacWarDungeonCompleted(wasConquered, factionID)`, `OnFacWarBunkerVulnerable/NonVulnerable(attackerID)`, `OnEnlistmentStateUpdated(f, direct, corp)` | | | EMPTY |

**Spec.**
- Author `warzones(warzoneID, factionA, factionB, hqA, hqB)` with two
  rows: Caldari/Gallente and Amarr/Minmatar.
  - [A] The HQs come from public knowledge. *Falsified by* the dashboard
    naming different HQs.
- Author `fw_systems(ssid, warzoneID, occupierID, adjacency, vpScore, vpThreshold)`,
  seeded from the warzone systems. How the client lists those systems
  locally is **unknown**: name the extractor once it is found.
- Answer `GetAllWarzones` and `GetHQSystemIDs` **now** from the authored
  table, even before occupation is live.
- VP accrues from factional-warfare complexes, which are C3 sites with
  `dunArchetypesFactionalWarfare`. Narrowcast `OnVictoryPointStateUpdated`
  to the system on change.

---

## 6. Interference, the DBS, the ESS and insurgency

**Solar-system interference** (`solarsystemInterferenceMgr`, unrouted).
- `GetLocalInterferenceState()` (`solarsystemInterferenceSvc:45`) is read
  for `.normalisedInterferenceLevel` by the location panel's interference
  tooltip (`infoPanelLocationInfo._interference_tooltip_panel:454+`).
- That tooltip exists only where `SystemCanHaveInterference`: not high-sec,
  abyss, void, wormhole or Triglavian space (`solarsysteminterference/util:7-16`).
- With no answer, the **tooltip BREAKS on hover in every low and null
  system**, and re-asks every time, because `None` is the cache sentinel.
  [V]
- `GetAllInterferenceBands()` (`:49`) answers `{ssid: band}`; a missing
  system is "none". `None` BREAKS the map filter (`in None`).
- **Spec:** answer `{}`, and a quiescent `0x23 SolarsystemInterferenceState`.
  Its state keys are `_interferenceLevel`, `_lastDecayTickTimestamp`,
  `_interferenceDecreasePerDecayTick > 0`, `_decayTickPeriod > 0`,
  `_maxInterferenceLevel > _quiescentInterferenceLevel` and
  `_quiescentInterferenceLevel`.
  - [A] Level 0.0 = quiescent, max 1.0, decrease 0.01, period 60 s.
    *Falsified by* real constants.
- The push is `OnSolarsystemInterferenceChanged(ssid, state)`.

**`dynamicResourceCacheMgr`** (unrouted; it is reached even with the ESS
flagged off).

| Call | Shape | Today and if silent |
|---|---|---|
| `GetDynamicResourceSettings()` (12 sites) | dict with `minOutput`, `maxOutput`, `equilibriumValue` (`mapFilterDynamicBounty:19-21`) and `autopaymentIntervalSeconds`, `bountiesCut`, `bufferTimeSeconds`, `mainBankPayRatio`, `reserveBankMaxPayout`, `reserveBankTimeAccessFactor`, `timeIntervalSeconds` | BREAKS the tooltips (subscript) |
| `GetDBSMapData()` (`mapFilterDynamicBounty:17`) | `{ssid: float}` | BREAKS the map filter |
| `GetESSAgencyData()` (`contentProviderESS:64`) | `{ssid: {currentOutput, mainValue, reserveValue, vaultOpen}}` (`:11-14`) | DEGRADES the Agency (caught) |
| `GetESSReserveBankKeys(ssid)` | a list of typeIDs from `essreservekeys.fsdbinary` | BREAKS its caller (iterate) |
| `GetESSSystemDetails(ssid)` (`essSystemContentPiece:17`) | dict, keys unknown | BREAKS the card [I] |

**Spec:**
- `{}` for the map and Agency calls, and `[]` for the keys.
- Settings `{minOutput 0.0, maxOutput 2.0, equilibriumValue 1.0, ...}`.
  [A] *Falsified by* the real constants.
- **The DBS** is answered with `(1.0, False)`, CCP's off value
  (`space.rs:720`). A real DBS is a per-system float driven by ratting
  volume, with `OnSystemDynamicBountyStateChanged(ssid, rate)`. Leave it
  for C5.

**The ESS** is flagged off (`global_config.rs:60-65`, `ess_disabled=1`), so
`essMgr.GetDataForClientSolarSystem` is never reached and the panel's
`IsAvailable` is false (`infoPanelESS:61-65`): **EMPTY**.
- Populated, the data is a dict: `essID` (a ball in the park), `mainValue`,
  `reserveValue`, `mainBankLink` (`None` or
  `{linkID, characterID, startedAt, completesAt}`),
  `reserveBankLastPulseInitiated`, `reserveBankPulsesRemaining`,
  `reserveBankPulsesTotal` and `reserveBankActiveLinks`
  (`dynamicResourceSvc:320-364`).
- The pushes are `OnSystemESSDataChanged(ssid, data)` and the
  `OnMainBank*` / `OnReserveBank*` family.
- The thefts calls (`:190-191`) must be `[]`.
- **Keep it off** until low and null PvP wants a magnet. The ESS ball's
  typeID is unknown.

**Insurgency.**
- `GetLocalCampaignClientSnapshot` returns `(own ssid, None)` and
  `GetAllVisibleCampaigns` returns `[]` (`space.rs:587,615`). Both are
  correct.
- Populated, a `CampaignClientSnapshot` is a `0x23` with
  `_campaignID, _fsmState, _stateExpiryTimestamp, _warzoneID, _originSolarsystemID, _coveredSolarsystemIDs, _structureID, _pirate/antipiratePointsRequired/Scored, _pirateFactionID`.
- The pushes are `OnInsurgencyCampaign{Started,Updated}ForLocation(snapshot)`,
  `...EndingForLocation(ssid)` and so on.
- The corruption and suppression values are **gateway**
  (`eve_public.pirate.{corruption,suppression}.api`), so the hangar's call
  times out after 10 s regardless of what we answer. **Last priority:** it
  needs FW and P6.2.

---

## 7. Incursions and world events (tales)

**Incursions are push-only.** [V]
- `OnTaleData(ssid, {taleID: tale})` (`incursionSvc:89`). A tale carries
  `templateClassID` (in `INCURSION_TEMPLATE_CLASSES`), `taleID`,
  `incursedSystems`, `stagingSolarSystemID`, `influence`, `state`,
  `hasChat`, `hasFinalEncounter`, `rewardGroupID`, `effects`,
  `templateNameID` and `musicState`.
- They end with `OnTaleRemove(taleID, templateClassID, templateID)` and
  `OnInfluenceUpdate(taleID, influence)`.
- `rewardMgr.GetRewardLPLogs` and `GetDelayedRewardsByGroupIDs` are
  unrouted, which BREAKS the LP-incursion wallet panel and the journal tab.
  [I] **Spec now:** answer `[]` and `{}`.

**World events.** The same `OnTaleData` push, with `templateClass.worldEvents`.
[V]
- `tale_service:106` ignores a tale without an **`objectiveChainID`**.
- With one, the client renders a full objective tracker from the
  `objectivechains` static data. The other fields are `locationID`,
  `managerSolarSystemID`, `templateNameID`, `templateDescriptionID`,
  `aggressorFactionID`, `endTime` and `sceneTypesByLocationID`.
- `taleMgr.GetGlobalWorldEventTales` answers `[]` (`space.rs:136`).
- **`get_active_tales_by_template` answers `None`** (`tale_service:154`),
  which BREAKS its caller. Answer `[]`.
- **Spec later:** a world event is an authored tale row
  `(taleID, templateID, objectiveChainID, managerSolarSystemID, endTime)`,
  pushed on session change into the system. This is the cheapest
  "something is happening here" lever, and bots can be pointed at it.

---

## 8. Seasons, login rewards, achievements, goals and activities

Every macho surface here already gets **CCP's own "off" answer**, except
`activityMgr`. [V]

| Surface | Today | If silent |
|---|---|---|
| `seasonManager.get_season_data_for_character` (`seasonservice:271`) | `None` (`live.rs:175`); `is_season_active()` False then gates every other call | EMPTY |
| `loginCampaignManager.get_client_campaign_state` (`loginCampaignService:127`) | `None`, stored over the `'UNKOWN'` sentinel | EMPTY |
| `seasonalLoginCampaignManager.get_active_campaign` (`:56`, unpack 4), `get_claim_history` | `(None,)*4` and `{}` | EMPTY (a bare `None` would BREAK) |
| `milestoneMgr.ProcessCharacterLogon` | `None`; the timer is push-only | EMPTY |
| **`activityMgr`** (`activitiesSvc:45,138,163,167`) | **unrouted** | **BREAKS**: `set(None)` at `:46` once any `newActivities` row is inside its date window on the host clock |
| Achievements, daily, career and corp goals, campaigns | gateway | EMPTY after a 10 s timeout (4 s ×2 for campaigns); achievements DEGRADES (`repository:257` `TypeError` on CCP's own `None`) |

**Spec:**
- `activityMgr`: `GetActivityBadgeStatuses` → `[]` (or the persisted seen
  ids), `ActivitySeen(id)` → `True`, `GetActivitiesPopState` → `1`,
  `SetActivitiesPopState` → `True`.
  - [A] `1` means "do not auto-pop". The popup path is Chinese-server-only
    (`:119`).
- **Seasons** are the only macho challenge system. If the synthesis wants
  shared weekly goals, the cost is:
  - an authored season row set (the client reads the season and challenge
    static data);
  - `Challenge` as a `0x23`. It is whitelisted, and its state keys are
    unknown until probed;
  - about ten pushes (`OnChallengeProgressUpdate(id, progress)`, ...);
  - counting `process_client_event`.
- **Not recommended now.**

---

## 9. Epic arcs, agents in space, void space, operations and pirate strongholds

**Epic arcs** (the graph is client-side in `epicArcs.fsdbinary`).
- `agentMgr.GetMyEpicArcStatus` → `{}` is correct. Populated, it is
  `{arcID: {missionID: row(.acceptedDate, .completedDate, .quitDate, .nameID)}}`
  (`epicArcController._GetMissionStatus:95-100`).
- **`GetMessagesForEpicArcMissions` is `None` and DEGRADES.**
  `_ConnectAllNodes` calls `.get` on it, so the controller is never built.
  It is called three times per node with no cache.
  - **Answer `{}` now.**
  - Later answer `{messageKey: {missionID: messageID}}`, keyed by
    `'messages.epicMission.journalText.{chapterTitle,inProgressMessage,completedMessage}'`
    (`epic_arcs_data:14-16`) and derived from `missions.fsdbinary`
    `messages`. That is C4's job.
- `epicArcStatus.AgentHasEpicMissionsForCharacter` → `False`.
- The push is `OnEpicJournalChange()`.

**Agents in space.**
- `agentsInSpace.fsdbinary` has 360 rows (`dungeonID`, `solarSystemID`,
  `spawnPointID`, `typeID`).
- The Agency warps with `agents.GetAgentMoniker(id).WarpToAgentInSpace()`
  (`agentContentPiece._ExecuteWarpTo:273-276`).
- Server needs `GetAgents` rows with `stationID=None`, a ball at the spawn
  point, and that moniker method.
- **Which slim field binds the ball to the agent is unknown.** EMPTY;
  skip.

**Void space** is opt-in through a key item:
- the jump is `voidSpaceMgr.VoidSpacePlayerJump(itemID, typeID)`;
- the pushes are `OnVoidSpaceEntered({content_id, nebula_graphic_id})`,
  `OnVoidSpaceRelog(...)`, `OnAddExpiryDurationForVoidSpaceContent(end_time)`
  and `OnVoidSpaceContentFinished(content_id, timer_type)`;
- `ClientIsReady()` is called on login.

EMPTY; lowest priority.

**Operations** (`operations`, `operationSites`, ...) are the NPE and AIR
tutorial, gated by `operationsManager.can_character_play_the_tutorial`.
That answers `False`. **Keep it that way.** EMPTY.

**Pirate strongholds.** `custom_agency_provider.get_content_data()`
(`agency/client/custom_content:16`) is followed by `len()` at `:17`, so
`None` raises an uncaught `TypeError`. **Answer `[]`.** Populated, it is a
list of `agency.common.contentdata.CustomContentData`, whose wire form is
unknown.

---

## Proposals

1. **Adopt CCP's signal tracker as the site protocol, verbatim.**
   - `SignalTrackerRegister` → narrowcast `OnSignalTrackerFullState`.
   - Per-tick `OnSignalTracker*Update` deltas.
   - System singlecasts `OnCosmicAnomalyAdded`, `OnCosmicSignatureAdded`
     and `OnCosmicAnomalyRemoved`.
   - Values are `0x23` instances of the whitelisted explorationscanner
     classes.

   This one feed lights up the overlay, the probe window, the warp menu and
   the local Agency rows (§2).
2. **A site is a server item with a position, keyed by the instance
   record in §2.4. It is not a Destiny ball by default.** C3 owns the record
   and this surface only reads it. The record carries `signatureID`,
   `instanceID`, `dungeonID`, position, scan strength, signature radius and
   `targetID`.
3. **Agency visibility policy:** rows for the pilot's current system and
   counts everywhere else (§3). Bots consume the same API, so they cannot
   cheat.
4. **Land a breaker sweep now (packet K1)**, before any content. Each item
   is a typed empty value whose shape the consumer pins:
   - `dungeonInstanceCacheMgr`'s `*Count`, `GetSignatureInstances` and
     `GetFactionWarfareBattlefieldInstances` → `{}`;
   - `activityMgr` ×4;
   - `dynamicResourceCacheMgr` ×5;
   - `solarsystemInterferenceMgr.GetAllInterferenceBands` → `{}`;
   - `taleMgr.get_active_tales_by_template` → `[]`;
   - `rewardMgr` → `[]` and `{}`;
   - `custom_agency_provider.get_content_data` → `[]`;
   - `agentMgr.GetMessagesForEpicArcMissions` → `{}`;
   - `epicArcStatus.AgentHasEpicMissionsForCharacter` → `False`;
   - `essMgr` thefts → `[]`.
5. **Add `pyutil::new_style(class, state)`** as the one `0x23` helper,
   generalising `skill.rs:795`. Every populated surface in §2 and §5–§8
   needs it.
6. **Push `OnEnteringDungeonRoom`, `OnDungeonCompleted` and
   `OnExitingDungeon`, and stamp `dunObjectID`, `entityGroupID` and
   `objectiveTargetGroup` on every site slim** (§4). The objective panel is
   then free, because the client drives the chain from static data.
7. **Make factional warfare the first populated "world state" after
   sites.** It is an authored two-row `warzones` table and an `fw_systems`
   table, answered by `GetAllWarzones`, `GetHQSystemIDs` and occupation/VP
   as `0x23` objects, plus two pushes. This is the PvP-with-friends loop
   the client already renders.
8. **Do not serve, and do not populate:** seasons, login rewards,
   milestones, operations, void space, incursions, insurgency and
   achievements. Keep CCP's off answers. Revisit gateway goals (corp and
   daily) only after P6.2.
9. **Pick world-event tales as the cheap "living universe" lever** after
   FW. An authored tale with an `objectiveChainID` gets a full client
   tracker for the cost of one push.

## Packets

| Id | Size | Owns (files) | What | Tests |
|---|---|---|---|---|
| **K1 breaker sweep** | S | `service/space.rs` arms (`dungeonInstanceCacheMgr`, `taleMgr`, `essMgr`), a new `service/live_misc.rs` for the unrouted `activityMgr`, `dynamicResourceCacheMgr`, `solarsystemInterferenceMgr` (bands only), `rewardMgr` and `custom_agency_provider`, and `service/character.rs` (epic-arc arms) | the typed empties in Proposal 4 | one unit test per arm asserting the type (dict, list or bool) the consumer pins, with the consumer's file:line in the test name or a comment |
| **K2 `new_style` helper** | S | `pyutil.rs` (one fn), `skill.rs` (switch it to the helper) | `0x23` builder | round-trip encode of `CharacterSkillEntry` byte-identical to today; an encode of `CosmicAnomalyInfo` with plain keys |
| **K3 signal tracker** | M | new `src/signal_tracker.rs`, the `scanMgr::systemScanMgr.GetFullState` arm, and the `SignalTrackerRegister` arm | §2.4: the full state, deltas, the three singlecasts and target-id mapping for `'scan'` warp | full state from a fixture instance table has arity 4 and dict values; an add followed by a remove yields one `AnomalyUpdate` with the site in `added` then `removed`; deviation matches `CalculateDeviationForSignal` at s = 1, 11 and 30; `GetPositionalSiteItemID` semantics (anomaly → `item`, signature → `scan`) |
| **K4 Agency rows** | S | `dungeonInstanceCacheMgr` arms (after K1) | §3 rows and counts from the C3 instance table | per archetype: rows for the own system, ints for others; `dungeonID` exists in the dungeons extract |
| **K5 dungeon tracking** | M | new `src/dungeon_tracking.rs`, and the slim-field stamping in `dungeon.rs`/`npc.rs` (C6 coordination) | §4 pushes and `dunObjectID` | push arity 5 on room entry; completion fires once; every spawned slim has `dunObjectID` |
| **K6 factional warfare** | M | new `src/facwar.rs`, the `fwWarzoneSolarsystem` and `fwVictoryPointMgr` arms, and the authored `warzones` and `fw_systems` tables (C1 or C7 decide the format) | §5 | `GetAllWarzones` is non-empty and every faction maps to a warzone; `GetLocalOccupationState[0] == session ssid`; the OccupationState keys are underscored |
| **K7 interference state** | S | `solarsystemInterferenceMgr` arm (after K2) | quiescent `0x23` state | the state keys satisfy the class invariants |
| **K8 world-event tales** | S-M | new `src/tales.rs` | `OnTaleData` on session change and `GetGlobalWorldEventTales` | a tale without `objectiveChainID` is never sent; the dict values carry `taleID` and `managerSolarSystemID` |

**Order:** K1 and K2 in parallel, then K3 together with C3's generator,
then K4, K5 and K6, then K7 and K8.

## Unknown

- Whether warp-to-`item` for an anomaly needs the site ball in the
  **client's** ballpark (Proposal 2 assumes it does not).
- `GetScanTargetID`'s exact scheme, beyond "stable per instance".
- How the client enumerates warzone systems locally, which is the
  extractor for `fw_systems`. The real HQ systems are also unknown.
- `facWarMgr`'s ~28 per-method shapes. It is a census blind spot: the
  remote is held on an attribute.
- `Challenge`, `CustomContentData` and `OccupationState` state keys on the
  wire, and whether `dateTaken=None` renders.
- Whether any `newActivities` row is inside its window on today's host
  clock (this decides whether `activityMgr` ever fires).
- Whether the Agency's base `except` catches an error that
  `uthread.parallel` re-raises, from the unknown-site fetch.
- `GetESSSystemDetails` keys, the ESS ball's typeID, the
  `GetPirateInsurgencyIceHeistInstances` container, and the `rewardMgr` row
  shapes.
- The real interference, DBS and dynamic-resource constants.
- Which slim field ties an in-space agent ball to its `agentID`.
