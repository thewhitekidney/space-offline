# EvEJS research lane — four blocked features

Source: `$EVEJS` (AGPL-3.0). **Facts only** — rules, constants,
field names, call names, state machines. No source copied. All cites `path:line`.
Paths relative to `$EVEJS/`.

---

# 0. Shared static-data layer (all four features)

All world geometry comes from one static-table reader,
`server/src/services/_shared/referenceData.js`: `TABLE` (name → table id) at lines 15-60,
`ROW_KEY` (table id → the JSON array key inside the payload) at lines 62-86, read through
`readStaticTable(table)` / `readStaticRows(table)`, with a module-level `cache` Map
(`referenceData.js:88`). Localised names are unwrapped by `localName(value)` preferring
`value.en` (`referenceData.js:90-101`).

Tables relevant here (`TABLE` name → id → `ROW_KEY`):

| TABLE constant | table id | rows key | cite |
|---|---|---|---|
| `SOLAR_SYSTEMS` | `solarSystems` | `solarSystems` | referenceData.js:40, :74 |
| `STATIONS` | `stations` | `stations` | referenceData.js:41, :75 |
| `STATION_TYPES` | `stationTypes` | `stationTypes` | referenceData.js:42, :76 |
| `STARGATES` | `stargates` | `stargates` | referenceData.js:50, :82 |
| `STARGATE_TYPES` | `stargateTypes` | `stargateTypes` | referenceData.js:43, :77 |
| `CELESTIALS` | `celestials` | `celestials` | referenceData.js:44, :78 |
| `ASTEROID_BELTS` | `asteroidBelts` | `belts` | referenceData.js:48, :80 |
| `ASTEROID_FIELD_STYLES` | `asteroidFieldStyles` | `fieldStyles` | referenceData.js:49, :81 |
| `MOON_MINING_POINTS` | `moonMiningPoints` | `points` | referenceData.js:45, :79 |
| `MOVEMENT_ATTRIBUTES` | `movementAttributes` | `attributes` | referenceData.js:51, :84 |
| `STATION_STANDINGS_RESTRICTIONS` | `stationStandingsRestrictions` | — | referenceData.js:59 |

`server/src/space/worldData.js:28` (`buildMaps`) is the single place these are indexed, into
`solarSystemsById`, `stationsById`, `stationTypesById`, `stargateTypesById`, `celestialsById`,
`asteroidBeltsById`, `stationsBySystem`, `celestialsBySystem`, `asteroidBeltsBySystem`,
`stargatesById`, `stargatesBySystem`, `movementByTypeId` — `worldData.js:39-50`.
Every `*BySystem` list is sorted ascending by its id
(`stationID` for stations, `itemID` for the rest) — `worldData.js:117-128`.
**Note the primary keys: stations are keyed `stationID`; celestials, belts and stargates are
keyed `itemID`.**

Player structures are deliberately *not* in this layer: `worldData` reads them from
`loadedStructureState.getLoadedStructureState()` so the static module never pulls in the
structure runtime — `worldData.js:22-26`.

SQLite table names are listed in `server/src/gameStore/tableOwnership.js:369`
(`stargates`, `stationGraphicLocators`, `stationStandingsRestrictions`, `starterShipFittings`, ...).

Per-system authored scenery overrides live as JSON under
`server/src/gameStore/data/authoredSpaceProps/<SystemName>.json`
(e.g. `Jita.json`, `Amarr.json`, `Perimeter.json`, `Yulai.json`), loaded by
`server/src/space/authoredSpaceProps.js`; defaults by
`server/src/space/defaultEmpireScenery.js` and the layouts under
`server/src/space/empireGatePresence/` (`gateSceneryLayout.js`, `stationSceneryLayout.js`,
`empireSentryLayout.js`, `customsOperatorLayouts.js`, `everMoreGatePresence.js`).

---

# 1. Stargates and jumping between systems

## 1.1 Client call

Single macho service call, no answer expected:

- Service `beyonce`, method **`CmdStargateJump`** — `server/src/services/ship/beyonceService.js:4305`
- Args, positional: `[0] fromStargateID`, `[1] toStargateID`, `[2] shipID` (optional)
  — `beyonceService.js:4306-4309`
- `args[2]` is validated against the session's active ship; mismatch → user error
  `SHIP_ID_MISMATCH` — `beyonceService.js:4325-4331`
- **The CallRsp carries no payload.** Handler returns null; the comment records that Tranquility
  answers with an empty stream and the client ignores it, driving everything from the
  `PerformSessionChange` + Destiny stream instead — `beyonceService.js:4341-4346`
- Client-side callers named in that comment: `autopilot.py:358`, `gateJumpSvc.py:124`
- Errors are thrown as a wrapped user error via `_throwStargateJumpUserError`
  — `beyonceService.js:2386`
- Also allowlisted on the web/bot gateway as `{service:"beyonce", method:"CmdStargateJump"}`
  — `server/src/_secondary/express/evejsWebGatewayRuntime.js:343`

Related but separate calls (not plain gates): `structureJumpBridgeMgr.CmdJumpThroughStructureStargate`
(`server/src/services/structure/structureJumpBridgeMgrService.js:1724`),
`beyonce.CmdJumpThroughFleet` (`beyonceService.js:2997`),
`shipcasterTravelMgr.CmdJumpThroughShipcaster`
(`server/src/services/shipcaster/shipcasterTravelMgrService.js:23`).

## 1.2 Server sequence

Entry point `jumpSessionViaStargate(session, fromStargateID, toStargateID)`
— `server/src/space/transitions.js:4278`.

Phase A — admission checks, in order (each returns `{success:false, errorMsg}`):

| # | Check | errorMsg | cite |
|---|---|---|---|
| 1 | ship-death transition block | (varies) | transitions.js:4279 |
| 2 | session is in space (`session._space`) | `NOT_IN_SPACE` | transitions.js:4281-4287 |
| 3 | active abyssal run | (varies) | transitions.js:4287-4293 |
| 4 | pending warp landing | (varies) | transitions.js:4294-4296 |
| 5 | both gates resolve | `STARGATE_NOT_FOUND` | transitions.js:4304-4309 |
| 6 | `beginTransition(session,"stargate-jump",sourceGate.itemID)` | `STARGATE_JUMP_IN_PROGRESS` | transitions.js:4310-4315 |
| 7 | `sourceGate.destinationID === destinationGate.itemID` | `STARGATE_DESTINATION_MISMATCH` | transitions.js:4316-4325 |
| 8 | `sourceGate.solarSystemID === session._space.systemID` | `WRONG_SOLAR_SYSTEM` | transitions.js:4326-4334 |
| 9 | active ship record exists | `SHIP_NOT_FOUND` | transitions.js:4336-4343 |
| 10 | not criminally flagged (crimewatch) | `CRIMINAL_TIMER_ACTIVE` | transitions.js:4345-4351 |
| 11 | no active weapon timer | `WEAPONS_TIMER_ACTIVE` | transitions.js:4353-4362 |
| 12 | capital hull + scripted-HIC gate scramble | `STARGATE_JUMP_BLOCKED_BY_SCRIPTED_HIC` | transitions.js:4364-4375 |
| 13 | **surface distance to gate ≤ 2500 m** | `TOO_FAR_FROM_STARGATE` | transitions.js:4377-4387 |
| 14 | scene exists | `SCENE_NOT_FOUND` | transitions.js:4393-4400 |

HIC-blocked ship groups (capitals): `30` Titan, `485` Dreadnought, `547` Carrier, `659`
Supercarrier, `883` Capital Industrial, `1538` Force Auxiliary, `4594` Lancer Dreadnought,
`5120` Command Carrier — `transitions.js:212-221`. Exempt: `513` Freighter,
`902` Jump Freighter — `transitions.js:222-225`.

Phase B — `spaceRuntime.startStargateJump(session, sourceGate.itemID)`
— `server/src/space/runtime/spaceRuntime/stargateJump.js:50`. Further checks there:

- `WARP_LANDING_PENDING`, `LINK_WITH_SHIP_JUMP_DISABLED`, `SHIP_IMMOBILE` (movement lock),
  `JUMP_COOLDOWN_ACTIVE` (superweapon jump/cloak lock), `DISRUPTION_LANCE_DEBUFF_ACTIVE`
  — `stargateJump.js:73-101`
- entity kind must be `stargate` and `canEntitiesInteractLocally(ship, gate)` → else
  `STARGATE_NOT_FOUND` — `stargateJump.js:103-113`
- gate `activationState` must be `OPEN` else `STARGATE_NOT_ACTIVE` — `stargateJump.js:115-127`
- **Motion is frozen before the FX**: if `mode !== "STOP"` or `speedFraction > 0` or
  `|velocity| > 0.5`, the server calls `resetEntityMotion`, drops the entity from subwarp plans
  and persists it — `stargateJump.js:129-140`
- Broadcasts Destiny special FX **`effects.JumpOut`** on the ship ball, with
  `targetID = sourceGate.itemID`, `start:true`, `active:false`,
  `graphicInfo = tuple(destinationSolarSystemID, null)`, `includeStopInSameUpdate:true`,
  `useHistorySafeSessionStamp:true`, Destiny contract
  `CRITICAL_MOVEMENT_OR_SHIPPRIME` — `stargateJump.js:142-175`
- Returns `{shipID, sourceGateID, stamp, deliveredCount, resultSessionDelivered}`
  — `stargateJump.js:185-194`
- Caller requires `resultSessionDelivered === true`, else
  `STARGATE_JUMP_FX_NOT_DELIVERED` — `transitions.js:4420-4426`

Phase C — the handoff delay. The jump is **not** completed inline; a deadline is computed in
*simulation* time and the completion is scheduled:

- `STARGATE_JUMP_COMMAND_HANDOFF_DELAY_MS = 4000` — `transitions.js:175`
- `STARGATE_JUMP_POST_FX_HANDOFF_DELAY_MS = 3000` — `transitions.js:176`
- `STARGATE_JUMP_HANDOFF_RECHECK_MAX_DELAY_MS = 1000` (TiDi re-check slice) — `transitions.js:180`
- `buildStargateJumpHandoffDeadline(commandSessionSimTimeMs, jumpOutStamp,
  jumpOutObservedSessionSimTimeMs)` — `transitions.js:1271`; the deadline is the later of the
  4 s command hold and (JumpOut Destiny-stamp boundary + 3 s) — `transitions.js:1305-1322`.
  Invalid stamp → `STARGATE_JUMP_FX_STAMP_INVALID` — `transitions.js:4438-4444`
- `scheduleStargateJumpHandoff(...)` — `transitions.js:2358`; it calls `completeStargateJump`
  when the deadline is reached, trace `stargate-jump-handoff-deadline-reached`
  — `transitions.js:2422-2432`. If the source sim clock is gone the jump is cancelled
  — `transitions.js:2401`
- A jump queue notice is sent immediately after the FX — see §1.6

Phase D — `completeStargateJump(session, sourceGate, destinationGate, activeShip)`
— `transitions.js:2143`, in order:

1. Transition still `"stargate-jump"`, else `STARGATE_JUMP_CANCELLED` — `transitions.js:2163-2171`
2. `buildGateSpawnState(destinationGate, activeShip)` → arrival geometry — `transitions.js:2173`
3. `deactivateActiveModulesForSpaceTransition(session, "stargate-jump")` — `transitions.js:2184`
4. `sendMachoObjectDisconnects(session, SERVICES_LET_GO_AT_A_MOVE)` — releases the bound
   objects of services `beyonce` and `ship` only (`SERVICES_LET_GO_AT_A_MOVE =
   {"beyonce","ship"}`) — `transitions.js:2185`, set at `transitions.js:948`.
   The client is expected to re-bind them in the new system.
5. `spaceRuntime.detachSession(session, {broadcast:true, lifecycleReason:"stargate-jump"})`
   — `transitions.js:2186-2189`. `broadcast:true` means the old scene's observers get the ship's
   ball removal (Destiny `RemoveBalls`, see §1.7).
6. `moveShipToSpace(shipID, destinationGate.solarSystemID, {position, direction,
   velocity:{0,0,0}, speedFraction:0, mode:"STOP", targetPoint: position})`
   — `transitions.js:2191-2198`. **Arrival is stationary.**
7. `syncInventoryItemForSession(...)` with `emitCfgLocation:false` — `transitions.js:2204-2217`
8. `updateCharacterRecord` → `buildLocationIdentityPatch(record, destinationSolarSystemID,
   {stationID:null, structureID:null})`, guarded by
   `expectedLocationTransitionReason:"stargate-jump"` — `transitions.js:2219-2229`
9. `applyCharacterToSession(session, characterID, {emitNotifications:false, logSelection:true,
   selectionEvent:false})` — rebuilds the session's identity keys — `transitions.js:2238-2242`
10. `spaceRuntime.attachSession(session, shipItem, {...})` with
    `systemID: destinationGate.solarSystemID`, `beyonceBound:false`,
    `pendingUndockMovement:false`, **`stargateJumpCloak:true`**, `broadcast:true`,
    `emitSimClockRebase:false`, `previousSimTimeMs` (source clock),
    `deferInitialBallparkStateUntilBind:true`, `deferUniverseSiteReconcile:true`,
    `universeSiteReconcileReason:"stargate-jump"` — `transitions.js:2248-2263`
11. `spaceRuntime.emitStargateArrivalObserverFx(session, destinationGate.itemID, shipID)`
    — `transitions.js:2263-2267`
12. `queuePostSpaceAttachFittingHydration(session, shipID,
    {inventoryBootstrapPending:false, hydrationProfile:"stargate"})` — `transitions.js:2281`
13. `flushCharacterSessionNotificationPlan`, then
    `queuePendingSessionEffects(session, {awaitBeyonceBoundBallpark:true,
    previousLocalChannelID})`, then `flushPendingCommandSessionEffects` — `transitions.js:2285-2290`.
    `previousLocalChannelID` is read from `session.solarsystemid2 || session.solarsystemid ||
    dockedLocationID` **before** step 9 — `transitions.js:2231-2236`; it drives the Local chat
    channel swap.
14. `mapTelemetryState.recordSolarSystemJump(session, {fromSolarSystemID, toSolarSystemID,
    kind:"stargate", shipID, sourceID, destinationID})` — `transitions.js:2292-2299`
15. `dailyGoalsState.recordActivity(characterID, "space_jump", 1)` — `transitions.js:2313-2316`
16. `achievementRuntime.recordStargateJump(characterID, sourceGate, destinationGate)`
    — `transitions.js:2326-2330`; defined at
    `server/src/services/achievement/achievementRuntime.js:194`
17. `customsInspectionRuntime.registerArrival(session, {gateID, transitionID})` — contraband
    inspection is authored at **arrival**, never at departure (explicit comment: a queued or
    failed jump must never sanction cargo in another system) — `transitions.js:2337-2343`,
    comment at `transitions.js:4389-4391`
18. `endTransition(session, "stargate-jump")`, returns
    `{stargate, spawnState, customsInspection, boundResult}` — `transitions.js:2346-2355`

## 1.3 Arrival placement (geometry and distances)

`buildGateSpawnState(stargate, ship, options)` — `transitions.js:1508`:

```
direction = sampleDirectionInCone(forward, 45°, random)
offset    = stargatePhysicalRadius + shipRadius + 12000
position  = stargate.position + direction * offset
```

- `STARGATE_JUMP_IN_SURFACE_CLEARANCE_METERS = 12000` — `transitions.js:188`.
  Explicit design note at `transitions.js:182-187`: retail puts arrivals at a **random direction
  but a fixed distance** off the gate's hull; the distance is deliberately *not* scattered, so
  approach times and gate-camp coverage match retail.
- `STARGATE_JUMP_IN_SCATTER_CONE_RADIANS = 45 * π/180` (45° half-angle) — `transitions.js:189`
- `sampleDirectionInCone` samples **uniformly over the spherical cap**, not uniformly in angle
  (`cosTheta = 1 - r*(1-cos(cone))`, `phi = r*2π`) — `transitions.js:1489-1506`, note at 1487-1488
- Basis built by `buildPerpendicularBasis(forward)`: reference axis is `+X` when `|forward.x|<0.9`
  else `+Y`, then two cross products — `transitions.js:1476-1485`
- `forward` = `getResolvedStargateForwardDirection(stargate)` — called with **only the gate**, so
  the system-to-system term is skipped and the resolution order collapses to
  (1) gate's authored `dunRotation` → direction, (2) the normalized gate `position` vector
  (i.e. radially outward from the star) — `transitions.js:1510`, resolver at
  `server/src/space/stargateOrientation.js:81-92`
- Gate physical radius, `resolveStargatePhysicalRadius` — `server/src/space/stargateRadius.js:46`:
  first non-zero of `physicalRadius`, `ballRadius`, `destinyRadius`, `typeRadius` on the gate row
  (`stargateRadius.js:14-20`), then the inventory type's `radius`, then the instance `radius`,
  then `options.fallbackRadius`, then **`DEFAULT_STARGATE_PHYSICAL_RADIUS_METERS = 15_000`**
  — `stargateRadius.js:13`
- Ship radius, `resolveShipRadiusMeters` — `transitions.js:1456`: `ship.radius ?? ship.spaceRadius`,
  else `worldData.getMovementAttributesForType(shipTypeID).radius`, else 0
- Arrival `direction` is also written as the ship's facing and its `targetPoint` equals its own
  position (so it is pointing away from the gate and holding still) — `transitions.js:2191-2198`

Separate, simpler builder for a system-level (non-gate) arrival,
`buildSolarSystemSpawnState(solarSystemID)` — `transitions.js:1551`: prefers the system's
**first stargate** (`minOffset = max(gateRadius*0.4, 5000)`), else the **first station**
(`minOffset = max((station.radius||15000)*0.4, 5000)`, `clearance = 5000`), else a celestial,
preferring one that is **not** the sun (`kind !== "sun" && groupID !== 6`)
— `transitions.js:1551-1586`. `buildOffsetSpawnState` (`transitions.js:1530`) uses the anchor's own position vector as
the direction (fallback `{1,0,0}`) and `offset = max(anchor.radius + clearance, minOffset)`
— `transitions.js:1530-1549`.

## 1.4 Gate static data and destination linkage

Loaded in `server/src/space/worldData.js:28` from static reference tables
(`server/src/services/_shared/referenceData.js`, `TABLE` map at lines 40-51; SQLite table name
`stargates` is listed in `server/src/gameStore/tableOwnership.js:369`):

- `TABLE.STARGATES` → `"stargates"` (`referenceData.js:50`), indexed `stargatesById` and
  `stargatesBySystem` (sorted by `itemID`) — `worldData.js:105-111`, `126-128`
- `TABLE.STARGATE_TYPES` → `"stargateTypes"` (`referenceData.js:43`), indexed by `typeID`
  — `worldData.js:68-70`
- Accessors: `getStargateByID`, `getStargatesForSystem`, `getStargateTypeByID`,
  `getSolarSystemByID`, `getStationsForSystem`, `getCelestialsForSystem`,
  `getMovementAttributesForType`

Stargate row fields consumed (scene-entity builder `buildStaticStargateEntity`,
`server/src/space/runtime/staticSceneEntity.js:739-785`):

`itemID`, `typeID`, `itemName`, `solarSystemID`, `position{x,y,z}`,
**`destinationID`** (the *paired gate's itemID*), **`destinationSolarSystemID`**,
`activationState`, `poseID`, `interactionRadius`, `skinMaterialSetID`, `dunRotation`,
`hasVolumetricDrifterCloud`, and the four sov tuples
`localCorruptionStageAndMaximum` / `destinationCorruptionStageAndMaximum` /
`localSuppressionStageAndMaximum` / `destinationSuppressionStageAndMaximum`.
Derived on the entity: `kind:"stargate"`, `groupID`, `categoryID`, `ownerID`
(origin system owner, default `1`), `radius`, `interactionRadius`, `velocity {0,0,0}`,
`typeName`, `groupName`, `graphicID`, `raceID` (from the type row),
`originSystemOwnerID`, `destinationSystemOwnerID`, `destinationSystemWarning`,
`destinationSystemWarningIcon`, `destinationSystemStatusIcons`.

**The linkage is one-directional per row**: `destinationID` points at the far gate's `itemID`,
and the jump validates `sourceGate.destinationID === destinationGate.itemID`
(`transitions.js:4316-4319`). `toStargateID` may be omitted; the server then uses
`sourceGate.destinationID` — `transitions.js:4303`.
Gate group/category in the SDE sense: `groupID === 10`, `categoryID === 2`
— `server/src/space/transientStargates.js:28`.

Gate type radius sanity gate: a usable gate type needs `graphicID > 0`, a matching type
`graphicID`, and `type.radius > 0` — `transientStargates.js:28-29`.

Data the client is given about the far side, as a Destiny rowset on the gate ball:
`STARGATE_JUMP_HEADERS = ["toCelestialID", "locationID"]`, one row
`[destinationID, destinationSolarSystemID]`, rowset class
`eve.common.script.sys.rowset.Rowset`, under the ball key **`jumps`**
— `server/src/space/destiny/index.js:64`, `970-977`, `741`. Empty rowset when either field is
missing. The same `jumps` key is reused for Ansiblex jump bridges, alongside
`targetSolarsystemID` — `destiny/index.js:634-638`; comment notes the client's
`JumpTransitionGate.Prepare` unpacks `jumps[0]` for the arrival camera.

### Activation state machine

`STARGATE_ACTIVATION_STATE = {CLOSED:0, ACTIVATING:1, OPEN:2}`
— `server/src/space/runtime/staticSceneEntity.js:124-128`.
`STARGATE_ACTIVATION_TRANSITION_MS = 3000` — `staticSceneEntity.js:130`.

`resolveStargateActivationState(gate)` — `server/src/space/runtime/spaceRuntime/stargateActivation.js:36`:
1. per-gate override map, 2. else `getSolarSystemStargateActivationState(destinationSolarSystemID)`,
3. else the row's own `activationState` defaulting to `CLOSED`.

`getSolarSystemStargateActivationState(systemID)` — `stargateActivation.js:17`:
per-system override → `keepsAllStargatesActiveDuringLazyLoading()` → `OPEN` →
system is in `_garbageCollectedSolarSystemIDs` → `OPEN` → else
`isSolarSystemSceneLoaded(systemID) ? OPEN : CLOSED`.
**So a gate is closed when its destination system's scene is not loaded**, unless lazy-loading
keeps gates active (AGENT.md:189 notes gates stay active so jumping loads the target).

`refreshStargateActivationStates({targetGateID, targetSystemID, animateOpenTransitions,
broadcast})` — `stargateActivation.js:56`: a `CLOSED → OPEN` change is animated through
`ACTIVATING` with `activationTransitionAtMs = now + 3000`; any other change is applied
instantly with `activationTransitionAtMs = 0` — `stargateActivation.js:88-100`.

`INCLUDE_STARGATES_IN_SCENE` is an exported flag — `staticSceneEntity.js:789`.

## 1.5 Jump cloak / invulnerability

The gate cloak, not an undock invuln:

- Requested as `attachSession({stargateJumpCloak:true})` — `transitions.js:2252`; also used by
  wormholes (`server/src/services/exploration/wormholes/wormholeMgrService.js:123`) and
  Ansiblex (`structureJumpBridgeMgrService.js:1523`, suppressed when `starbaseCyno`)
- Applied by `applyStargateJumpCloakState(shipEntity, {nowMs, durationMs})`
  — `server/src/space/runtime/scene/sessions.js:390-394`
- **`DEFAULT_STARGATE_JUMP_CLOAK_DURATION_MS = 60_000`** (60 s)
  — `server/src/space/runtime/shipCloak.js:81`
- Entity fields set: `stargateJumpCloak = true`, `stargateJumpCloakStartedAtMs`,
  `stargateJumpCloakDurationMs`, `stargateJumpCloakUntilMs = now + durationMs`
  — `shipCloak.js:438-441`; cleared to `false/0/0/0` on cancel — `shipCloak.js:454-457`
- Destiny cloak mode is `DESTINY_CLOAK_MODE_RESTORE = 2` while gate-cloaked
  (`DESTINY_CLOAK_MODE_NORMAL = 1` is a module cloak) — `shipCloak.js:75-77`;
  `isEntityStargateJumpCloaked` requires both the flag and mode 2 — `shipCloak.js:342-348`
- Also demotes the ball's massive state (`isMassive = false`, `massive = false`)
  — `shipCloak.js:350-356`
- Client notice: **`OnJumpCloakUpdated`**, keyed `"shipid"`, args
  `[shipID, endTimeFileTime, durationFileTimeTicks]` where the duration is
  `durationMs * 10000` FILETIME ticks — `server/src/space/runtime/scene/cloaking.js:219-223`.
  Sent from `attachSession` when `stargateJumpCloak === true`
  — `server/src/space/runtime/scene/sessions.js:515-519`
- Cancellation notice: `sendStargateJumpCloakCancelled(entity)` — `cloaking.js:227`
- Expiry is swept per tick by `processStargateJumpCloakExpirations(nowMs)` over the scene's
  dynamic entities, calling `cancelStargateJumpCloak(entity, "expired", {nowMs})`
  — `cloaking.js:1322-1350`
- `MIN_CLOAKING_DISTANCE_METERS = 2000` is the proximity decloak distance — `shipCloak.js:79`
- Module attribute `canActivateInGateCloak` gates which modules may be used while gate-cloaked
  — `shipCloak.js:124` (`MODULE_ATTRIBUTE_CAN_ACTIVATE_IN_GATE_CLOAK`)
- Decloak exemptions: probe group IDs `{479, 492, 548, 972, 4088}` and `categoryID === 8`
  probes (`DECLOAK_PROBE_EXEMPT_GROUP_IDS`, `shipCloak.js:95`); non-physical groups
  `{502, 885}` (`shipCloak.js:97`)

**Contrast — undock invulnerability is a different, much longer timer:**
`UNDOCK_INVULNERABILITY_DURATION_TICKS = 300000000n` FILETIME ticks = **30 s**
(`/10000` → 30000 ms) — `transitions.js:196-199`, used at `transitions.js:681-682`.

## 1.6 Capacitor / fuel cost

**None.** There is no capacitor, fuel or isotope debit anywhere in the gate-jump path:
`grep capacitor server/src/space/transitions.js` → no matches;
`server/src/space/runtime/spaceRuntime/stargateJump.js` → no matches. The only capacitor checks
in this area belong to **jump portals / bridges**, a different feature
(`server/src/services/ship/beyonceService.js:2951` and `:3101`, "The bridge ship does not have
enough capacitor to activate the jump portal.").

## 1.7 Visibility handover between the two systems

- Departure: `detachSession(session, {broadcast:true, lifecycleReason:"stargate-jump"})`
  — `transitions.js:2186`. Scene-level handler at
  `server/src/space/runtime/scene/sessions.js:1383`; per-scene it releases starbase control,
  tow links, PvP filament queue, MJD spools, FireShow and transient gates
  (`scene/sessions.js:1384-1434`), then removes the ball. The broadcast is Destiny
  **`RemoveBalls`** (payload name, `server/src/space/destiny/protocol/payloads.js:21`,
  critical-action id `1` at `payloads.js:90`; batched at
  `server/src/space/destiny/batching/parkUpdateBatcher.js:367` as
  `["RemoveBalls", [{type:"list", items:[...ids]}]]`)
- Arrival: `attachSession(... broadcast:true ...)` — the arriving ball is published to the
  destination scene's observers; the arriving client's own scene bootstrap is Destiny
  **`AddBalls2`** plus **`SetState`** (`payloads.js:5`, `:40`; builders
  `buildAddBalls2Payload` / `buildSetStatePayload` at
  `server/src/space/destiny/index.js:995`, `1009`)
- `emitSimClockRebase:false` plus `previousSimTimeMs` / `initialBallparkPreviousSimTimeMs` /
  `initialBallparkPreviousTimeDilation` / `initialBallparkPreviousCapturedAtWallclockMs`
  carry the **source** system's simulation clock across, so the client's Destiny timeline is
  continuous instead of rebasing — `transitions.js:2255-2259`; clock offset computed as
  `preservedCurrentSessionSimTimeMs - attachedSceneCurrentSimTimeMs` into
  `session._space.clockOffsetMs` — `scene/sessions.js:500-505`
- `deferInitialBallparkStateUntilBind:true` and `beyonceBound:false` hold the destination's
  initial state until the client re-binds the `beyonce` object; combined with
  `queuePendingSessionEffects({awaitBeyonceBoundBallpark:true})` — `transitions.js:2260`, `2288`
- `session._space` is rewritten wholesale on attach with `systemID`, `sceneKey`, `sceneKind`,
  `instanceID`, `shipID`, `beyonceBound`, `initialStateSent`, `initialBallparkVisualsSent`,
  `initialBallparkClockSynced`, `pendingUndockMovement`, visibility-generation state,
  `clockOffsetMs`, `historyFloorDestinyStamp`, `lastSentDestinyStamp`
  — `scene/sessions.js:396-430`
- `deferUniverseSiteReconcile:true` + `universeSiteReconcileReason:"stargate-jump"` postpone the
  exploration-site reconcile — `transitions.js:2261-2262`
- `clearSameSceneDynamicClassification` and
  `buildPostAttachVisibilityReconciliationOptions` are the visibility primitives
  — `transitions.js:150-162`

### Session change keys

Whitelist `SESSION_CHANGE_ALLOWED_KEYS` — `server/src/network/clientSession.js:59-92`:
`role, countryCode, charid, corpid, corpAccountKey, hqID, allianceid, factionid, warfactionid,
genderID, bloodlineID, raceID, schoolID, stationid, stationid2, structureid, solarsystemid,
solarsystemid2, locationid, worldspaceid, constellationid, regionid, shipid, fleetid, fleetrole,
wingid, squadid, corprole, rolesAtAll, rolesAtBase, rolesAtHQ, rolesAtOther`.

A jump changes `solarsystemid`, `solarsystemid2`, `locationid`, `constellationid`, `regionid`
and nulls `stationid` / `structureid` (via `buildLocationIdentityPatch(..., {stationID:null,
structureID:null})` — `transitions.js:2219-2223`).

The *order* matters to the client. `INITIAL_SPACE_CHARACTER_SESSION_CHANGE_KEYS`
— `server/src/services/character/characterState.js:512-536`:
`genderID, corpAccountKey, bloodlineID, raceID, corprole, corpid, allianceid, regionid,
rolesAtAll, rolesAtBase, locationid, hqID, solarsystemid2, solarsystemid, shipid, rolesAtHQ,
charid, rolesAtOther, constellationid` — applied by `reorderSessionChangesByKeys`
(`characterState.js:538`) for a character's first in-space session change.
Reason cited: the client's `corporationWindow.py:383-403` gates alliance panels on
`session.allianceid`.

## 1.8 Jump queue (`OnJumpQueueUpdate`)

An in-memory per-destination queue, sent as soon as the JumpOut FX lands
(`sendJumpQueueUpdate(session, destinationGate.solarSystemID)` — `transitions.js:4446`):

- `STARGATE_JUMP_QUEUE_SPACE = 1000` (advertised capacity) — `transitions.js:190`
- `STARGATE_JUMP_QUEUE_TIMEOUT_SECONDS = 180` — `transitions.js:191`
- Store: `stargateJumpQueuesByDestination: Map<solarSystemID, Map<characterID, entry>>`
  — `transitions.js:201`, `transitions.js:814-822`
- Entry fields: `{characterID, fileTime, expiresAtMs: now + 180_000}`
  — `transitions.js:860-864`
- `cleanupStargateJumpQueue` drops expired entries and deletes empty destination maps
  — `transitions.js:825-841`
- Notice: **`OnJumpQueueUpdate`**, keyed `"clientID"`, args
  `[destinationSolarSystemID, 1000, [characterID...], dict{characterID: fileTime},
  180]` — `transitions.js:914-922`
- This is **not** admission control; nothing blocks on queue size in the version read.

## 1.9 Observer FX at arrival

`emitStargateArrivalObserverFx(session, destinationGateID, shipID, options)`
— `server/src/space/runtime/spaceRuntime/stargateJump.js:197`:
Destiny special FX **`effects.JumpIn`** on the arriving ship ball, built by
`buildObserverStargateJumpFxOptions({excludedSession: session, duration: 5000, ...})`
— `stargateJump.js:229-238`. **Duration 5000 ms**, and the arriving pilot's own session is
excluded (they already played the tunnel from the JumpOut side).

Abyssal uses a different guid, `effects.JumpOutAbyssal`
— `server/src/space/runtime/scene/abyssalTrace.js:712`.

Timing traces emitted along the way (useful as a debugging contract):
`stargate-jump` (begin), `stargate-jump-source-observer-fx`,
`stargate-jump-handoff-scheduled`, `stargate-jump-handoff-deadline-reached`,
`stargate-jump-destination-observer-fx`, `stargate-jump-attached`, `attach-session`,
`detach-session` — `stargateJump.js:177`, `:240`; `transitions.js:4400-4412`, `:4447-4455`,
`:2269`; `scene/sessions.js:521`, `:1421`.

## 1.10 GM-created ("transient") stargates

`server/src/space/transientStargates.js` — a staff/debug chat command builds temporary gates:

- `LIMITS = {maxGates: 64, lifetimeMs: 1200000}` (**20 min** lifetime, 64 gates)
  — `transientStargates.js:17`
- Gate types are discovered at runtime from installed world samples: a row of
  `TABLE.STARGATE_TYPES` qualifies only if its `typeID` appears in `TABLE.STARGATES`, and
  `groupID === 10`, `categoryID === 2`, `graphicID > 0`, type `graphicID` matches, `radius > 0`
  — `transientStargates.js:22-37`
- `resolveRoute(session, fromStargateID)` is consulted *before* the static table in
  `jumpSessionViaStargate`, and a transient route synthesizes
  `{destinationGate, itemID: sourceGate.destinationID, solarSystemID}`
  — `transitions.js:4299-4303`
- Transient gates get `destinationID: spawn.anchorID` and
  `destinationSolarSystemID: request.destination.solarSystemID`
  — `transientStargates.js:147`
- `scheduleJump(route, {session, ship, characterID, transition, spaceGeneration,
  deadlineSimTimeMs, cancel, complete})` — `transitions.js:4461-4478`. When the transient
  destination anchor is an existing station/celestial rather than a fabricated arrival gate,
  the completion falls back to `jumpSessionToSolarSystem(session, systemID,
  {spawnStateOverride: route.spawn, stargateJumpCloak:true})` — `transitions.js:4469-4474`
- Chat command surface: `server/src/services/chat/commands/transientGates.js`;
  commands include `gateshowcase` — `transientStargates.js:54`
- Related: `server/src/space/gateSkinCommand.js` (gate skins, `skinMaterialSetID`),
  `server/src/space/stargateVisualOverrides.js`

## 1.10b Routing / autopilot — not a server feature

There is **no server-side route or set-destination service**. `server/src/services/map/`
exposes no route/jump-graph handler (`grep -rn "Handle_" server/src/services/map/` lists only
sov, FW warzone, insurgency, dev-index, incursion, history, visits and beacon handlers;
`mapService.js:532-661`). The client owns the jump map from its local SDE — `doc/FUTURE_FEATURES.md:23`
("stargates, positions ... is in the SDE tables") — and the autopilot simply issues one
`CmdStargateJump` per hop from `autopilot.py:358` (cited in `beyonceService.js:4343-4345`).
The server only *records* jumps, via `mapTelemetryState.recordSolarSystemJump`
(`transitions.js:2292`) and `mapService.Handle_GetSolarSystemVisits` (`mapService.js:606`).
A bot-side jump graph is planned separately at `bots/src/world/jumpGraph.mts`
(`doc/FUTURE_FEATURES.md:23`), not present in `bots/src/` today.
Shortest-gate-distance *is* computed server-side for one unrelated purpose — industry
restrictions — by `resolveShortestStargateJumpDistance(sourceSolarSystemID,
destinationSolarSystemID)` walking `stargate.destinationID`
(`server/src/services/industry/industryRestrictions.js:211`, gate walk at `:163`).

## 1.11 `eve_public.stargate.lock.api.GetRestrictedSystemsRequest`

**EvEJS stubs it, and answers an empty success.**

- Handled by the "compatibility" gateway service, in a map named
  `EMPTY_SUCCESS_RESPONSE_TYPES` —
  `server/src/_secondary/express/gatewayServices/compatibilityGatewayService.js:3-24`;
  the stargate-lock pair is at `:21-22`:
  `eve_public.stargate.lock.api.GetRestrictedSystemsRequest` →
  `eve_public.stargate.lock.api.GetRestrictedSystemsResponse`
- The reply is `statusCode: 200`, `statusMessage: ""`,
  `responseTypeName: "...GetRestrictedSystemsResponse"`,
  `responsePayloadBuffer: emptyPayload` — `compatibilityGatewayService.js:146-152`.
  `emptyPayload` is a **zero-length buffer**, i.e. a protobuf message with every field at its
  default — so the restricted-systems list comes back empty and no system is locked.
  Source of `emptyPayload`: `server/src/_secondary/express/publicGatewayLocal.js:1122`
  (`EMPTY_PAYLOAD`), default `Buffer.alloc(0)` — e.g.
  `gatewayServices/campaignGatewayService.js:57`
- Advertised in `handledRequestTypes` so the registry routes it
  — `compatibilityGatewayService.js:84-89`
- Dispatch: `publicGatewayLocal.getEmptySuccessResponseType(requestTypeName)` →
  `gatewayServiceRegistry.getEmptySuccessResponseType` — `publicGatewayLocal.js:2238-2239`,
  used at `publicGatewayLocal.js:2338`; exported at `:2618`
- Contrast with the deliberate-404 path: only
  `eve_public.payment.schedule.api.GetActiveByTypeRequest` is in
  `INTENTIONAL_NOT_FOUND_REQUEST_TYPES` (404 "no active payment schedule"), with a comment that a
  typed empty 200 there would create schedule ID 0 and make the client loop
  — `compatibilityGatewayService.js:26-31`, `:131-138`. **An empty 200 is the right answer for
  `GetRestrictedSystems`; a 404 is not.**
- Other stubs in the same file for reference: `plex.vault.api.Balance`,
  `career.goal.api.GetAll`, `character.skill.plan.GetAll` / `SetActive` (empty 200);
  `career.goal.api.GetDefinitions` returns 4 synthetic goals with UUIDs
  `...0401/0501/0601/0701` and `career` 1-4 — `compatibilityGatewayService.js:33-62`, `:94-116`;
  `character.skill.plan.GetActive` returns a 16-byte zero UUID — `:118-129`
- There is **no** `restrictedSystems` data model anywhere in the repo
  (`grep -rn RestrictedSystems server/ doc/ tools/` → only the two lines above).
- Gateway transport: protobuf over the Express "public gateway"
  (`server/src/_secondary/express/publicGatewayLocal.js`, stream handler wired in
  `server/src/_secondary/express/server.js:28`, edge runtime
  `server/src/edge/gateway/gatewayEdgeRuntime.js:62`). Certificates `gateway-*.pem`
  — `server/src/config/certificateFiles.js:8`.

## 1.12 What data a server must have — stargates / jumping

Per **stargate** row (static):
`itemID`, `typeID`, `itemName`, `solarSystemID`, `position {x,y,z}`,
`destinationID` (paired gate itemID), `destinationSolarSystemID`,
`activationState` (0/1/2), `radius` (or `physicalRadius`/`ballRadius`/`destinyRadius`/`typeRadius`),
`interactionRadius` (default 2500), `dunRotation [yaw,pitch,roll]` (optional; derived from
destination-system direction when absent), `poseID`, `skinMaterialSetID` (optional),
`hasVolumetricDrifterCloud`, and optionally
`localCorruptionStageAndMaximum`, `destinationCorruptionStageAndMaximum`,
`localSuppressionStageAndMaximum`, `destinationSuppressionStageAndMaximum`.

Per **stargate type** row: `typeID`, `typeName`, `groupID` (10), `categoryID` (2),
`graphicID`, `raceID`, `radius`.

Per **solar system** row: `solarSystemID`, `constellationID`, `regionID`,
`position {x,y,z}` (needed to derive a gate's forward direction),
`security`, owner/faction id (for `originSystemOwnerID` / `destinationSystemOwnerID`).

Per **ship type**: `radius` (via a movement-attributes table keyed by `typeID`) —
used in the arrival offset.

Per **session / character** (mutable): `characterID`, `shipID`, `solarsystemid`,
`solarsystemid2`, `locationid`, `constellationid`, `regionid`, `stationid`, `structureid`,
plus the transition guard `_transitionState {kind, transitionID, startedAt}` and the space
binding `_space {systemID, sceneKey, shipID, beyonceBound, clockOffsetMs, ...}`.

Per **ship entity in space** (mutable): `position`, `velocity`, `direction`, `speedFraction`,
`mode`, `targetPoint`, `radius`, `isMassive`/`massive`,
`stargateJumpCloak`, `stargateJumpCloakStartedAtMs`, `stargateJumpCloakDurationMs`,
`stargateJumpCloakUntilMs`.

Server-side state, not static: crimewatch criminal/weapon timers keyed by characterID;
the per-destination jump queue (`characterID → {fileTime, expiresAtMs}`); the per-gate and
per-system activation override maps; the set of loaded / garbage-collected solar system IDs.

Constants to reproduce:
`jump range 2500 m`, `arrival clearance 12000 m`, `arrival cone 45°`,
`default gate radius 15000 m`, `default interaction radius 2500 m`,
`gate cloak 60000 ms`, `undock invulnerability 30000 ms`,
`command handoff 4000 ms`, `post-FX handoff 3000 ms`, `TiDi recheck slice 1000 ms`,
`activation transition 3000 ms`, `JumpIn observer FX 5000 ms`,
`jump queue space 1000`, `jump queue timeout 180 s`,
`session change cooldown 10000 ms` (`transitions.js:193`),
`space boarding range 6550 m` (`transitions.js:192`).

Messages to implement:
client → server `beyonce.CmdStargateJump(fromGateID, toGateID, shipID?)`, answer empty.
server → client: Destiny `effects.JumpOut` (with `graphicInfo = (destSystemID, null)`),
Destiny `effects.JumpIn` (5 s, excluding the jumper), Destiny `RemoveBalls` / `AddBalls2` /
`SetState`, `OnJumpCloakUpdated(shipID, endFileTime, durationTicks)`,
`OnJumpQueueUpdate(destSystemID, 1000, [charIDs], {charID: fileTime}, 180)`,
a session change over the allowed-keys whitelist, and MachoObjectDisconnect for the
`beyonce` and `ship` bound objects.
gRPC public gateway: `eve_public.stargate.lock.api.GetRestrictedSystemsRequest` →
200 with a zero-length `GetRestrictedSystemsResponse`.


---

# 2. A second station in a system, and docking / undocking

Paths below are relative to `$EVEJS/`.

## 2.1 Where station data lives

Runtime reads **per-table JSON**, not SQLite: table `stations` → `<dataDir>/stations/data.json`,
rows under key `stations` — `server/src/services/_shared/staticJsonTableReader.js:11-19`,
`server/src/services/_shared/referenceData.js:41`, `:75`.
`dataDir` resolution: `EVEJS_GAMESTORE_DATA_DIR` → `<repo>/_local/gameStore/data` →
`server/src/gameStore/data` — `server/src/gameStore/storeRoot.js:41-60`;
Docker path `/var/lib/evejs/gameStore/data` — `AGENT.md:172`.
**The generated tables are not in the checkout** (purged); only build inputs are.

Tables: `stations`, `stationTypes`, `stationGraphicLocators`, `stationStandingsRestrictions`,
plus build-only `stationDockingPlacements` — `server/src/gameStore/tableOwnership.js:369-370`,
`:381-387`; all classed STATIC/read-only — `tableOwnership.js:372-374`.

Builder: `tools/DatabaseCreator/database-creator.js`, from SDE `npcStations.jsonl`
(`:2760-2824`) joined with `stationOperations.jsonl` (`:2755`), `npcCorporations.jsonl`
(`:2750`), celestials (orbitID), solarSystems/constellations/regions, and the type table.

## 2.2 `stations` row fields

Built at `tools/DatabaseCreator/database-creator.js:2783-2822`:

`stationID`, `security` (from system), `dockingCostPerVolume` (**hardcoded 0**),
`maxShipVolumeDockable` (**hardcoded 50000000**), `officeRentalCost` (**hardcoded 10000**),
`operationID`, `stationTypeID`, `corporationID` (= SDE `ownerID`), `solarSystemID`,
`solarSystemName`, `constellationID`, `constellationName`, `regionID`, `regionName`,
`stationName`, `position {x,y,z}`, `reprocessingEfficiency`, `reprocessingStationsTake`,
`reprocessingHangarFlag` (default 4), `itemName` (= stationName), `itemID` (= stationID),
`groupID`, `categoryID`, `orbitID`, `orbitName`, `orbitGroupID`, `orbitTypeID`, `orbitKind`,
`stationTypeName`, `stationRaceID`, `stationGraphicID`, `radius`, `interactionRadius` (= radius),
`useOperationName`, `dockEntry`, `dockPosition`, `dockOrientation`, `undockDirection`,
`undockPosition`.

- `radius` = the station **type**'s `radius` — `database-creator.js:2781`; `interactionRadius`
  is set equal to it (`:2819` region)
- Name formula: `"<orbitName> - <corporationName> <operationName>"` when `useOperationName`,
  else `"<orbitName> - <corporationName>"` — `database-creator.js:2777-2781`
- `dunRotation` is **deleted from every station**, then re-added for exactly two hand-authored
  IDs: `60003760 → [206.8031, 0, 0]`, `60015249 → [305, 0, 0]`
  — `database-creator.js:465-492`. The comment states ordinary stations must omit
  `dunRotation` so the client keeps the asset's default orientation.

## 2.3 `stationTypes` row fields

`database-creator.js:840-867`: `stationTypeID`, `typeName`, `groupID`, `categoryID`,
`groupName`, `raceID`, `graphicID`, `radius`, `basePrice`, `volume`, `portionSize`,
`published`, `dockEntry` (= first directional locator's position), `dockOrientation`
(= its direction), `graphicLocationID`, `directionalLocatorCategories`,
`undockLocatorCategories`.

## 2.4 `stationGraphicLocators` — the authored undock geometry (44 profiles)

File `tools/DatabaseCreator/staticTables/stationGraphicLocators/data.json`, top-level key
`locators`. Per profile: `stationTypeID`, `typeName`, `graphicID`, `graphicLocationID`,
`hasUndockLocators`, `graphicFile`, `directionalLocatorCategories`, `directionalLocators[]`,
`hasDirectionalLocators`, `undockLocatorCategories`.
Each locator: `category`, `position {x,y,z}`, `direction {x,y,z}`, `name`.
Categories present: `undockPoint`, `undockPointSmall`, `undockPointLarge`,
`undockPointCapital`, `undockPointSupercapital`.
Example row: stationTypeID 54 "Caldari Logistics Station", graphicID 237,
graphicLocationID 28, one `undockPoint` at position (36, 6854.26221, 7948.4668),
direction (0, 0, 1).

## 2.5 `stationDockingPlacements` — build input only, 5210 rows

`tools/DatabaseCreator/staticTables/stationDockingPlacements/data.json`; column tuple order
`stationID, stationTypeID, dockEntry, dockPosition, dockOrientation, undockDirection,
undockPosition` — normalizer `database-creator.js:884-907`.
`dockEntry` is a small station-local offset (e.g. `[2447.21484, -3841, 2988]` for typeID 1531);
`dockPosition` / `undockPosition` are **absolute world metres**; `dockOrientation` ==
`undockDirection` (unit vector) in every sampled row. Merged only when
`placement.stationTypeID` matches the station's type — `database-creator.js:2771-2773`,
`:2820-2824`. Its own `source` block records that it was reverse-engineered from client data.

## 2.6 Runtime station record (`getStationRecord`)

`server/src/services/_shared/stationStaticData.js:224-345`.
Station ID resolution order: explicit override → `session.stationid` → `session.stationID` →
`session.locationid` → `DEFAULT_STATION.stationID` — `:224-228`.

`DEFAULT_STATION` is **Jita 4-4**: stationID `60003760`, orbitID `40009077`, solarSystemID
`30000142`, constellationID `20000020`, regionID `10000002`, ownerID/corporationID `1000035`,
factionID `500001`, stationTypeID `1529`, operationID `22`, security `0.9`,
dockingCostPerVolume `0.0`, officeRentalCost `10000`, reprocessingStationsTake `0.05`,
reprocessingHangarFlag `4`, maxShipVolumeDockable `50000000`, upgradeLevel `0`
— `stationStaticData.js:37-64`.

Structures are served through the same function, shaped as pseudo-stations with
`isStructure: true` — `:238-290`.
`x/y/z` are flattened from `position` — `:329-331`, with a bug-shaped guard: a falsy component
(including a legitimate `0`) collapses to 0.

Station services, serviceID → mask bit — `stationStaticData.js:111-161`:
`16` Office Rental, `512` Cloning, `4096` Repair Facilities, `8192` Reprocessing Plant,
`16384` Market, `65536` Fitting, `1048576` Insurance, `25` Loyalty Point Store
(`serviceMask: 0`). `stationServiceItemID`s `28155`-`28166`.
NPC stations return every service enabled; structures gate on
`serviceStates[structureServiceID] === ONLINE` — `:455-470`.
`buildStationServiceMask` ORs `serviceID` (or an explicit `serviceMask`) for enabled services
— `:440-453`.

## 2.7 What the client is told about a station

- **`stationSvc.GetStation(stationID)`** → a `util.KeyVal` dict with keys
  `stationID, stationName, stationTypeID, solarSystemID, constellationID, regionID, ownerID,
  corporationID, dockingCostPerVolume, maxShipVolumeDockable, officeRentalCost, operationID,
  stationTypeID` (sent twice), `security, x, y, z, reprocessingEfficiency,
  reprocessingStationsTake, reprocessingHangarFlag, serviceMask`
  — `server/src/services/station/stationService.js:78-107`.
  **`x`, `y`, `z` are hardcoded `0.0` and `reprocessingEfficiency` hardcoded `0.5`**
  (`:98-101`) — the record's real values are discarded here. Default stationID when args are
  empty: `60003760` (`:74`). Wrapped by
  `buildCachedMethodCallResult(..., versionCheck: "run")` (`:108-113`)
- `stationSvc.GetStationItemBits` → bare 4-tuple `[ownerID, stationID, operationID,
  stationTypeID]` — `:118-133`
- `stationSvc.GetGuests` → list of `[charID, corp, alliance, warFaction]` — `:135-156`
- `stationSvc.GetStationsForOwner(ownerID)` → list of KeyVals with
  `stationID, stationName, solarSystemID, constellationID, regionID, stationTypeID, ownerID,
  corporationID, security` — `:158-190`
- `stationSvc.GetSolarSystem` → KeyVal `solarSystemID, solarSystemName, constellationID,
  constellationName, regionID, regionName, security, factionID, factionName, stationID,
  stationName, ownerID, ownerName, corporationID, corporationName, orbitID, stationTypeID`
  — `:219-241`. **Single-station-shaped**: it reports "the" station of the system, meaning the
  session's.
- The service is registered twice, as `station` and alias `stationSvc` — `:63-66`, `:243-247`
- `corpStationMgr.GetStationDetails` → `stationName, stationID, orbitID, description, security,
  dockingCostPerVolume, officeRentalCost, reprocessingStationsTake, reprocessingHangarFlag,
  corporationID, maxShipVolumeDockable, exitTime (null), standingOwnerID, upgradeLevel`
  — `server/src/services/corporation/corpStationMgrService.js:230-249`
- `GetMyCharacterStructures` NPC-station entry: dict `typeID, structureID, wars, services,
  ownerID, solarSystemID` — `server/src/services/structure/npcStationDirectory.js:136-150`
- In-space slimItem for `kind === "station"`: `corpID, allianceID, warFactionID, online: 1,
  incapacitated: 0, activityLevel, skinMaterialSetID, celestialEffect` (optional),
  `dunRotation` (only when a 3-element array)
  — `server/src/space/destiny/index.js:553-566`

**Confirmed absence:** `dockingCostPerVolume` and `maxShipVolumeDockable` are *only ever
reported, never enforced*. The only hits across `server/src` are the two payload builders and
the defaults (`stationStaticData.js:58`, `:62`; `stationService.js:92-93`;
`corpStationMgrService.js:241`, `:246`). **No ship-volume docking restriction and no docking
fee is charged.** `reprocessingEfficiency` is read for real only by
`server/src/services/reprocessing/reprocessingRuntime.js:529` and
`server/src/services/mining/miningIndustry.js:328`.

## 2.8 Station in-space entity

`buildStaticStationEntity` — `server/src/space/runtime/staticSceneEntity.js:355-401`:
`kind:"station"`, `itemID` (= stationID), `typeID` (= stationTypeID), `groupID`, `categoryID`,
`itemName`, `ownerID` (corporationID or 1), `corporationID`, `allianceID: 0`,
`warFactionID: 0`, `radius` = `getStationInteractionRadius(station)`, `position`,
`dockPosition`, `dockOrientation`, `undockDirection`, `undockPosition` (all from
`stationLocatorGeometry.buildStationDockingGeometry(station, {selectionStrategy: "first"})`
with stored-record fallbacks), `dunRotation`, `activityLevel`, `skinMaterialSetID`,
`celestialEffect`, `velocity {0,0,0}`.

## 2.9 Dock — the client calls

- Station: **`beyonce.CmdDock([stationID])`** — `server/src/services/ship/beyonceService.js:4232`.
  One argument. Returns `None`; the comment records 17 TQ recordings where the `CallRsp` stream
  is None — `:4296-4300`
- Upwell structure: **`structureDocking.Dock([structureID, shipID])`**
  — `server/src/services/structure/structureDockingService.js:137-139`; `shipID` mismatch with
  the session's active ship → user error `DeniedShipChanged` (`:147-149`)
- Gateway allowlist: `{service:"beyonce", method:"CmdDock"}`
  — `server/src/_secondary/express/evejsWebGatewayRuntime.js:344`

## 2.10 `CmdDock` server sequence

`beyonceService.js:4232-4300`:

1. `breacherPodDeathRuntime.isShipDeathPending(session._space.shipID)` → user error
   `DockingRequestDenied`, reason "Your ship is being destroyed." — `:4233-4235`
2. `spaceRuntime.getDockingDebugState(session, stationID)` logged — `:4240-4243`
3. `spaceRuntime.isDockingDisallowedByModules(session)` → `DockingRequestDenied` — `:4245-4253`
4. `spaceRuntime.canDockAtStation(session, stationID)` false → start an approach and throw
   user error **`DockingApproach`** with
   `buildDockingApproachUserErrorValues(dockingDebug)` — `:4255-4273`
5. In range → `spaceRuntime.acceptDocking(session, stationID)`; error mapping
   `DOCKING_DISALLOWED_BY_MODULE`, `WEAPONS_TIMER_ACTIVE` ("You cannot dock until your weapons
   timer expires."), `CRIMINAL_TIMER_ACTIVE` → all wrapped as `DockingRequestDenied`
   — `:4275-4295`

`canDockAtStation` gate chain — `server/src/space/runtime/spaceRuntime/docking.js:37-94`,
in order: crimewatch `isCriminallyFlagged` → entity + station exist
(`worldData.getStationByID(stationID) || worldData.getStructureByID(stationID)`) →
`canEntitiesInteractLocally` → `isShipMovementLockedByRuntime` →
`isEntityDisruptiveLanceDebuffed` (comment: Disruptive Lance disables tether/docking 60 s) →
for structures `structureTetherRestrictionState.getCharacterStructureDockingRestriction` →
`canShipDockAtStation(entity, station, maxDistance)`.

## 2.11 Docking range — the exact numbers

`server/src/space/runtime/stationDockingGeometry.js`:

| Constant | Value | cite |
|---|---|---|
| `DEFAULT_STATION_DOCKING_RADIUS` | **2500 m** (the default `maxDistance`) | :60 |
| `DEFAULT_STATION_INTERACTION_RADIUS` | **1000 m** | :56 |
| `DEFAULT_STATION_UNDOCK_DISTANCE` | 8000 m (unreachable, see §2.15) | :58 |
| `STATION_DOCK_ACCEPT_DELAY_MS` | **4000 ms** | :62 |
| `LEGACY_STATION_NORMALIZATION_RADIUS` | 100000 m | :64 |

Distance is **surface-to-surface**:
`getShipDockingDistanceToStation = max(0, |shipPos − stationPos| − ship.radius −
getStationInteractionRadius(station))` — `:236-247`.
`canShipDockAtStation` = that distance `<= max(0, maxDistance)` — `:249-251`.
**So: dock allowed within 2500 m of the station's hull, hull-to-hull** — not a type attribute,
not centre-to-centre.

`getStationInteractionRadius(station)` precedence — `:150-165`:
`station.radius > 0` → it; else `station.interactionRadius > 0` → it; else **1000**.
`station.radius` is the SDE station-type `radius` (attribute 162 lineage,
`database-creator.js:271`, `:2781`).
The structure path hardcodes the same 2500 as its `followBall` range
— `server/src/services/structure/structureDockingService.js:171-176`.

**There are two different station radii with different defaults that coexist**: the docking one
(default 1000, `stationDockingGeometry.js:56`) and the scenery/physical one
(default 15000, `server/src/space/stationRadius.js:11`). See §2.17.

## 2.12 Docking approach (dock issued out of range)

- `startDockingApproach` → for `kind === "station"` calls `scene.startStationDockingApproach`,
  otherwise `followBall(session, stationID, 2500, {dockingTargetID})`
  — `server/src/space/runtime/spaceRuntime/docking.js:130-142`
- `startStationDockingApproach` issues a single **`GotoPoint` at the station's own position**
  (not a follow holding at docking range), sets `entity.dockingTargetID = station.itemID`, and
  registers the entity in `scene._stationDockingApproaches`
  — `server/src/space/runtime/scene/pilotCommands.js:736-760`.
  Measured rationale in the comment at `:724-735`: 9 recorded TQ docks each answered
  `DockingApproach` followed by one GotoPoint at the station position, accepted 1.2-9.0 s later,
  7 of 9 with no second `CmdDock`; a follow-to-edge was measured 20 m outside range closing at
  0.03 m/s 200 s after the order; Carbon's homing law scales thrust by the 4th power of
  remaining distance.
- Any later movement order clears `dockingTargetID` and so cancels the approach — `:749-752`
- Per-scene post-tick `completeArrivedDockingApproaches` → `completeDockingApproach` re-runs,
  in `CmdDock`'s order: ship-death pending → `isDockingDisallowedByModules` →
  `canDockAtStation` → `acceptDocking` — `spaceRuntime/docking.js:153-215`.
  **A refusal there is not answered to anyone and not retried.**

## 2.13 `acceptDocking` — the state machine

`server/src/space/runtime/scene/pilotCommands.js:602-715`:

1. Reject if no entity/station, `hasPendingPilotWarpLanding` (`WARP_LANDING_PENDING`),
   station kind is neither station nor structure, or `!canEntitiesInteractLocally` →
   else `STATION_NOT_FOUND`
2. `hasActiveShipDockingDisallowedEffect` → `DOCKING_DISALLOWED_BY_MODULE`
3. `isShipMovementLockedByRuntime` → `SHIP_IMMOBILE`
4. `resolveSessionCrimewatchDockingRestriction` → its reason code
5. **Idempotence**: an existing `entity.pendingDock` for the same station returns success with
   `{acceptedAtFileTime, pending: true}`
6. `canShipDockAtStation` → `DOCKING_APPROACH_REQUIRED`
7. Structures: `getCharacterStructureDockingRestriction` → `STRUCTURE_DOCKING_RESTRICTED`
8. `resetEntityMotion(entity)`; `demoteEntityMassiveState(entity)`
9. `applyPendingDockStateCommand(entity, {stationID, acceptedAtMs,
   completeAtMs: nowMs + 4000, acceptedAtFileTime})` → sets `entity.pendingDock`
   — `server/src/space/destiny/commands/docking.js:3-5`
10. `forgetEntityFromNativeSubwarpPlans`; `persistShipEntity`
11. Notification **`OnDockingAccepted`**, id-kind `"charid"`, payload is a **1-element list
    `[stationID]`** — ship/station positions are accepted as parameters and discarded
    — `server/src/space/destiny/stream/actions.js:161-163`, builder chain
    `server/src/space/destiny/presentation/mode.js:151-161`
12. Destiny updates at one stamp: station →
    `buildOrdinaryDockingStopPresentationUpdates` = `SetBallMassive(id, 0)` then `Stop(id)`;
    **no `SetBallVelocity`** (comment: it would make Carbon snap hull orientation)
    — `destiny/presentation/mode.js:73-95`; structure →
    `buildStructureDockingTransitionUpdates`
13. Returns `{success: true, data: {acceptedAtFileTime}}`

Then in the scene tick, entities with `pendingDock` and `now >= completeAtMs` are collected
into `dockRequests` keyed by `session.clientID` and skipped for movement
— `server/src/space/runtime/scene/tick.js:2371-2383`; after the entity loop
`dockSession(request.session, request.stationID)` runs, and on failure `clearPendingDock(entity)`
— `:2955-2967`. **The dock completes 4000 ms after acceptance, on a tick.**

## 2.14 `dockSession`

`server/src/space/transitions.js:2828-2978`. Preconditions in order: ship-death block →
`CHARACTER_NOT_SELECTED` → abyssal block → `ALREADY_DOCKED` →
`resolveDockableLocation(stationID)` else `STATION_NOT_FOUND` → `getActiveShipRecord` else
`SHIP_NOT_FOUND` → `crimewatchState.isCriminallyFlagged` → `CRIMINAL_TIMER_ACTIVE` →
`hasActiveWeaponTimer` → `WEAPONS_TIMER_ACTIVE` (comment: a timer started *during the approach*
must still refuse the finalize) → structures `canCharacterDockAtStructure` →
`beginTransition(session, "dock", stationID)` else `DOCK_IN_PROGRESS`.

Body: `deactivateActiveModulesForSpaceTransition(session,"dock")` →
`clearPendingUndockInvulnerability` → `sendInvulnerabilityCancelled` →
`sendMachoObjectDisconnects(session, SERVICES_LET_GO_AT_A_MOVE)` →
`spaceRuntime.detachSession(session, {broadcast:true, lifecycleReason:"dock",
notifySelfOnTargetClear:true})` → `dockShipToLocation(shipItemID, locationID)` →
`topOffShipShieldAndCapacitorForDockingTransition` → `syncDockedShipTransitionForSession` →
`updateCharacterRecord` with `buildLocationIdentityPatch` setting `stationID` (station) or
`structureID` (structure), with homeStationID/cloneStationID defaulting to **60003760** →
`applyCharacterToSession` →
`flushCharacterSessionNotificationPlan(applyTqDockingTransitionSessionChangeShape(plan))` →
`sendDockingFinished(session, locationID)` → `broadcastOnCharNowInStation` /
`broadcastOnCharacterEnteredStructure`.
Returns `{success, data:{station: record, boundResult: buildBoundResult(session)}}`.

- `dockShipToLocation`: sets the ship item's `locationID = stationID`,
  `flagID = ITEM_FLAGS.HANGAR`, clears `customInfo` if it starts with `"Undocking:"`, and sets
  `spaceState: null`; refuses with `DOCK_LOCATION_NOT_FOUND` if neither station nor structure
  — `server/src/services/inventory/itemStore.js:7570-7595`
- `sendDockingFinished` → notification **`OnDockingFinished`**, id-kind `"charid"`,
  payload `[stationID]` — `transitions.js:800-810`
- Guest broadcasts **`OnCharNowInStation`** / **`OnCharNoLongerInStation`**, id-kind
  `"stationid"`, payload `[characterID, corpID, allianceID|None, warFactionID|None]`
  — `server/src/services/_shared/guestLists.js:47`, `:187`, `:218`.
  Marshal string-table indices **99** and **98** — `server/src/common/marshalStringTable.js:113-114`

### Session keys on dock / undock

Written in `applyCharacterToSession` — `server/src/services/character/characterState.js:4425-4446`:
`stationid`, `stationID`, `stationid2`, `structureid`, `structureID`, `structureTypeID`,
`worldspaceid`, `locationid`, `homeStationID`/`homestationid`,
`cloneStationID`/`clonestationid`, `solarsystemid2`, `solarsystemid`,
`constellationID`/`constellationid`, `regionID`/`regionid`.

- `locationid` = solarSystemID in a structure; else stationID when docked; else solarSystemID
  — `:4434-4436`
- `solarsystemid` = solarSystemID in a structure; **null when docked in an NPC station**; else
  solarSystemID — `:4441-4446`. Comment: a structure hangar's inventory bootstrap needs
  `solarsystemid` non-null.
- sessionChange build order: `...factionid, stationid, stationid2, structureid, solarsystemid,
  solarsystemid2, constellationid, regionid, shipid, locationid, worldspaceid, fleetid,
  fleetrole, wingid, squadid...` — `:4021-4110`
- For dock/undock the plan is reshaped by `applyTqDockingTransitionSessionChangeShape`:
  **`stationid2` is deleted** and the rest reordered to lead with
  `stationid, structureid, locationid, solarsystemid` — `transitions.js:647-667`
- `shipid` can be deferred (`deferDockedShipSessionChange`) → sent as null — `:4070-4073`

## 2.15 Undock — the client calls

- **`ship.Undock([shipID, ignoreContraband])`** —
  `server/src/services/ship/shipService.js:2121-2124`. `shipID` defaults to the session's ship;
  `ignoreContraband` defaults false; also accepts `kwargs` used by
  `_syncRequestedOnlineModulesForUndock` (`:2135`). **The service object is bound per station**:
  the comment records `ship::MachoBindObject((stationID, 15), ('Undock', ...)) →
  (<RemoteObject>, None)` and that the client takes no answer from the call — `:2158-2162`.
  Bot wire shape confirms it: `client.call("ship","Undock",[shipID,false])`
  — `bots/src/skills/ship.mts:79`
- **`structureDocking.Undock([structureID, shipID], kwargs{ignoreContraband})`**
  — `server/src/services/structure/structureDockingService.js:184-191`; structureID or shipID
  mismatch → `DeniedShipChanged` (`:197-208`)
- Contraband flow: the first call is refused with `SHIP_CONTRABAND_WARNING_UNDOCK`, the server
  throws the warning user error, the client re-calls with `ignoreContraband` set, and nothing
  has changed in between — `transitions.js:2659-2676`, `shipService.js:2141-2153`

## 2.16 `undockSession`

`transitions.js:2587-2827`. Preconditions: `CHARACTER_NOT_SELECTED` → abyssal block →
`getDockedLocationID` else `DOCK_REQUIRED` → `resolveDockableLocation` else
`STATION_NOT_FOUND` → `getActiveShipRecord` else `SHIP_NOT_FOUND` →
`getStrategicCruiserTransitionUseError` (`STRATEGIC_CRUISER_FIT_INCOMPLETE`, data:
`missingSubsystemFlags`, `invalidRegularSlotCount`, turret/launcher/cpu/power/droneBay/
fighterBay overloads — `:2552-2585`) → `beginTransition(session,"undock",locationID)` else
`UNDOCK_IN_PROGRESS` → structure one-way-undock restriction
(`UNDOCK_RESTRICTED_BY_STRUCTURE`, bypassed by GM) → contraband warning →
`inspectContrabandForTransition`.

Then:

1. `spaceRuntime.getStationUndockSpawnState(dockable.record, {shipTypeID,
   selectionStrategy: structure ? "random" : "first", selectionKey: shipItemID})`
   — `:2685-2689`. **No `extraUndockDistance` is passed, so it is 0.**
2. `moveShipToSpace(shipItemID, solarSystemID, {position: undockState.position,
   direction: undockState.direction, velocity {0,0,0}, speedFraction 0, mode "STOP",
   targetPoint: undockState.position, customInfo: "Undocking:<locationID>"})` — `:2691-2699`
3. `topOffShipShieldAndCapacitorForDockingTransition` — **shield and capacitor are refilled on
   undock, as on dock** — `:2704-2708`
4. `emitItemsChangedForSession` (structures add `clientCustomInfo: "UndockingStructure:"`) →
   `sendMachoObjectDisconnects(session, SERVICES_LET_GO_AT_A_MOVE)` →
   `broadcastOnCharNoLongerInStation` / `broadcastOnCharacterLeftStructure` — `:2709-2728`
5. `updateCharacterRecord` → `buildLocationIdentityPatch(record, solarSystemID,
   {stationID: null, structureID: null, homeStationID/cloneStationID defaulting to 60003760})`
   — `:2730-2746`
6. `clearDeferredDockedShipSessionChange`, `clearDockedFittingBootstrap`,
   `applyCharacterToSession`
7. **`spaceRuntime.attachSession(session, shipData, {systemID,
   undockDirection: undockState.direction, speedFraction: 1, pendingUndockMovement: false,
   undockBootstrapMovementPrime: true, skipLegacyStationNormalization: true, broadcast: true,
   emitSimClockRebase: false, deferUniverseSiteReconcile: true,
   universeSiteReconcileReason: "undock", ...})`** — `:2765-2782`
8. Structures: `session._space.pendingUndockStructureSlimItemChangeID = locationID`;
   `scene.syncStructureEntitiesFromState({broadcast:true, excludedSession: session})`
9. `queuePostSpaceAttachFittingHydration(..., {hydrationProfile: "undock"})`;
   `flushCharacterSessionNotificationPlan(applyTqDockingTransitionSessionChangeShape(plan))`;
   `queuePendingSessionEffects(session,{previousLocalChannelID})`;
   `flushPendingCommandSessionEffects`
10. `sendUndockInvulnerabilityUpdated` + `scheduleUndockInvulnerabilityCancellation` (§2.18)
11. Returns `{success, data:{station, ship, contrabandInspection, ignoreContraband,
    boundResult}}`. `Handle_Undock` returns null to the client regardless
    — `shipService.js:2163`

`options.deferInitialSelection === true` short-circuits before step 4; used by the NPE/AIR
runtime — `server/src/services/npe/airEncounterRuntime.js:111`.

## 2.17 Undock point geometry — exactly how the exit position is computed

`server/src/space/runtime/stationDockingGeometry.js:167-227` dispatches: structures →
`structureLocatorGeometry.buildStructureUndockSpawnState`; stations (any truthy station) →
`stationLocatorGeometry.buildStationUndockSpawnState`.
**The hand-rolled fallback at `:194-226` — the only place `DEFAULT_STATION_UNDOCK_DISTANCE =
8000` and `interactionRadius + 2500` would apply — is unreachable for any real station record**;
it sits after an unconditional `if (station) { return ... }`.

`server/src/services/station/stationLocatorGeometry.js`:

- `buildStationUndockSpawnState(station, options)` = `buildStationDockingGeometry` with
  `selectionStrategy` defaulting to `"random"`, returning
  `{direction, position, source, locatorCategory, locatorName}` — `:386-398`.
  (`undockSession` overrides to `"first"` for stations.)
- `buildStationDockingGeometry` — `:330-377`:
  - profile = `getStationLocatorProfile(station.stationTypeID)` from table
    `stationGraphicLocators` — `:16`, `:161-203`
  - candidates = locators whose `category` starts with `"undockPoint"`, preferring exactly
    `getUndockCategoryByShipType(shipTypeID)`, then generic `"undockPoint"`, else all
    — `:205-233`
  - `selectLocator`: `"first"` (index 0), `"random"` (`floor(rand*n)`), or `"hash"` (FNV-1a over
    `selectionKey`: offset basis `2166136261`, prime `16777619`, mod n) — `:235-275`
  - transform = `buildStationLocatorTransform`: **null unless the station has `dunRotation`
    (array) or `dockPosition`** — `:277-295`. With `dunRotation` → yaw/pitch/roll **degrees** →
    quaternion (`:81-98`) → vector rotate (`:100-132`); with only `dockPosition` → rotation
    `[0,0,0]`, `source: "identityFallback"`
  - **Authored path** (`source: "authored"`):
    `dockPosition = station.position + rotate(locator.position)`;
    `dockOrientation = undockDirection = normalize(rotate(locator.direction))`;
    `undockPosition = dockPosition + undockDirection * extraUndockDistance` — `:346-376`
  - **Stored fallback** (no locator or no transform; `source: "stored"`):
    `direction = normalize(station.dockOrientation || station.undockDirection)` with default
    forward `(0,0,1)`; `basePosition = station.dockPosition || station.undockPosition ||
    station.position`; `undockPosition = basePosition + direction * extraUndockDistance`
    — `:297-328`
- **Because `undockSession` passes no `extraUndockDistance`, it is 0, and the undock spawn
  position equals the dock position** — the authored locator point
  (`station.position + locator offset`), or the stored `dockPosition` from
  `stationDockingPlacements`. **There is no station-radius multiplier on the station undock
  path.**

Ship-size locator selection, by groupID
— `server/src/services/structure/structureLocatorGeometry.js:25-49`:
default `undockPointSmall`; `undockPointLarge` for groups
`27, 28, 380, 381, 513, 898, 900, 902, 941, 1202`; `undockPointCapital` for
`485, 547, 883, 1538, 4594`; `undockPointSupercapital` for `30, 659`.
Structure constants for comparison: `DEFAULT_FALLBACK_DOCK_OFFSET = 7500`,
`DEFAULT_FALLBACK_DOCK_PADDING = 3500`, `DEFAULT_FALLBACK_UNDOCK_PADDING = 2500`
— `structureLocatorGeometry.js:21-23`.

### `stationRadius.js` — a *different* radius, not used by docking

`server/src/space/stationRadius.js`:
`DEFAULT_STATION_PHYSICAL_RADIUS_METERS = 15_000` (`:11`);
`PHYSICAL_RADIUS_FIELDS = ["physicalRadius","ballRadius","destinyRadius"]` (`:12-16`).
`describeStationPhysicalRadius(station, options)` returns `{radius, source}` with resolution
order (`:65-112`): station's explicit `physicalRadius|ballRadius|destinyRadius`
(`"station.physicalRadius"`) → station type's same three (`"stationType.physicalRadius"`) →
`station.radius` → `stationType.radius` → `station.interactionRadius` →
`options.fallbackRadius || 15000` (`"fallback"`). Only strictly positive finite values count
(`:18-21`). Type resolution: `options.stationType` → `options.stationTypeByID(id)` →
`worldData.getStationTypeByID(id)`, type ID read from `stationTypeID || typeID || typeId`
(`:28-62`).
**Its only consumer is empire sentry-gun placement**:
`getStationSentryDistanceMeters = max(DEFAULT_EMPIRE_SENTRY_DISTANCE_METERS,
resolveStationPhysicalRadius(station, {fallbackRadius: DEFAULT_EMPIRE_SENTRY_DISTANCE_METERS}))`
— `server/src/space/empireGatePresence/stationSceneryLayout.js:13-26`.

### Other `stationDockingGeometry.js` helpers

`getStationDockPosition` / `getStationDockDirection` dispatch — `:98-148`: structures →
`structureLocatorGeometry.getStructureDockPosition`; stations →
`stationLocatorGeometry.getStationDockPosition` (strategy default `"hash"`, `selectionKey` =
ship itemID in debug use). Dead branches at `:115-119` (after the unconditional
`if (station)` return).
`getStationApproachPosition` returns the station **centre** unless `useDockPosition === true`
— `:122-128`.
`getStationDockDirection` falls back to `DEFAULT_RIGHT = {1,0,0}`
— `server/src/common/vector.js:24`; `DUPLICATE_DIRECTION_DOT = 0.99999` is the
`isNearlySameDirection` threshold (`vector.js:26`, used at `stationDockingGeometry.js:229-234`).
`snapShipToStationPerimeter` (legacy / login normalization only): desired distance =
`max(8000, getStationConfiguredUndockDistance(station), interactionRadius + ship.radius + 500)`;
direction = away from the station (or `station.undockDirection`); sets `entity.targetPoint` to
the station centre — `:291-307`. `getStationConfiguredUndockDistance` =
`|station.position − station.undockPosition|`, 0 if either is missing — `:66-76`.
`buildDockingDebugState` returns `canDock, dockingDistance, distanceToStationCenter,
distanceToDockPoint, distanceToApproachPoint, dockingThreshold, shipRadius, stationRadius,
shipPosition, shipVelocity, stationPosition, approachPosition, dockPosition, targetEntityID,
dockingTargetID, mode, speedFraction` — `:253-289`.
`stationLocatorGeometry.js` exports: `getStationLocatorProfile`,
`buildStationDockingGeometry`, `getStationDockPosition` (strategy default `"hash"`),
`buildStationUndockSpawnState` (strategy default `"random"`),
`clearStationLocatorGeometryCache`, plus a `_testing` bundle — `:405-423`.
Table read via `database.read(table, "/")` expecting `{locators: [...]}`; a failure is warned
**once** and leaves an empty profile map — `:161-199`. Profile normalization keeps
`stationTypeID, typeName, graphicLocationID, hasUndockLocators, directionalLocators[],
locatorCategories[]` — `:143-159`.

## 2.18 Undock initial velocity and Destiny state

`buildUndockMovement(entity, direction, speedFraction = 1)`
— `server/src/space/destiny/simulation/motionState.js:109-128`, invoked from attach when
`options.undockDirection` is set and `spawnStopped` is not
— `server/src/space/runtime/scene/sessions.js:377-385`, mirrored at `:685-690`:

- `clearRuntimeTrackingState(entity)`
- `entity.direction = normalize(undockDirection)`
- `entity.targetPoint = position + direction * 1.0e16` (effectively a point at infinity)
- `entity.speedFraction = clamp(speedFraction, 0, MAX_SUBWARP_SPEED_FRACTION)`;
  `MAX_SUBWARP_SPEED_FRACTION = 1.0` — `server/src/space/runtime/shipMotionKinematics.js:27`.
  `undockSession` passes 1.
- `entity.mode = "GOTO"`
- **`entity.velocity = direction * (entity.maxVelocity * speedFraction)`** — the ship leaves at
  full hull max velocity **instantly**, not accelerating from 0; zero vector if
  `maxVelocity <= 0`
- `session._space` carries `pendingUndockMovement`, `undockBootstrapMovementPrime`,
  `undockBootstrapMovementPrimeSent: false` — `scene/sessions.js:411-414`

First-sight bootstrap prime, sent once instead of the ordinary ship-prime `SetBallMass` for the
ego — `server/src/space/destiny/presentation/shipPrime.js:72-122`, all at one stamp:

1. `OnSpecialFX(shipID, "effects.Uncloak", {start:true, active:false, duration:6000})` —
   `UNDOCK_BOOTSTRAP_EFFECT_GUID = "effects.Uncloak"`,
   `UNDOCK_BOOTSTRAP_EFFECT_DURATION_MS = 6000` — `:23-24`
2. `SetBallPosition(shipID, position)`
3. `SetBallMassive(shipID, 0)` (non-enabling mass demotion)
4. `SetBallMass(shipID, mass)`
5. `SetBallVelocity(shipID, velocity)` — **only if `|velocity| > 0`**
6. `GotoDirection(shipID, normalize(targetPoint − position))`

Gating / one-shot bookkeeping — `server/src/space/runtime/scene/presentation.js:3087-3105`,
`:3156-3161`; the ordinary `SetBallMass` for the ego is filtered out of the prime batch when
this fires.

### Undock invulnerability timer

`transitions.js:196-199`: `UNDOCK_INVULNERABILITY_DURATION_TICKS = 300000000n` FILETIME ticks;
`FILETIME_TICKS_PER_MS = 10000n`; `FILETIME_EPOCH_OFFSET = 116444736000000000n` →
`UNDOCK_INVULNERABILITY_DURATION_MS = 30000`. **30 seconds.**

- Notification **`OnInvulunOnUndockingUpdated`** (sic), id-kind `"shipid"`, payload
  `[shipID, currentFileTime() + 300000000, 300000000]` — `transitions.js:669-684`
- Entity flags `undockInvulnerabilityActive = true`,
  `undockInvulnerabilityUntilMs = sceneSimNow + durationMs` — `:721-734`
- A `setTimeout(delayMs)` (overridable via `options.undockInvulnerabilityDelayMs`) then:
  flush the Destiny batch → **`OnInvulnCancelled`**, id-kind `"shipid"`, payload `[shipID]`
  (`:686-699`) → clear the two entity flags →
  `scene.syncSessionStructureTetherState(session, {forceReplayFx:true, repairOnEngage:true,
  replayFx:false})` and, if a tether engaged,
  `scene.sendTqUndockTetherPresentationToSession(session, shipID, structureID)` → flush again
  — `:736-797`. The timer is `unref()`ed and stored on
  `session._undockInvulnerabilityCancelTimer`.
- Docking clears it: `clearPendingUndockInvulnerability` (clears
  `_undockInvulnerabilityCancelTimer` and `_timedInvulnerabilityCancelTimer`) +
  `sendInvulnerabilityCancelled` — `:701-719`, called from `dockSession` at `:2904-2906`
- Tether interop reason string `"UNDOCK_INVULNERABILITY"`
  — `server/src/space/runtime/structureTether.js:312`

Auto-undock load-test constants: `DEFAULT_UNDOCK_DUMMY_COUNT = 100`,
`MAX_UNDOCK_DUMMY_COUNT = 500`, `UNDOCK_DUMMY_TARGET_DISTANCE = 1.0e16`,
`UNDOCK_WAVE_INTERVAL_MS = 200`, `TARGET_UNDOCK_BATCH_COUNT = 20`,
`MIN/MAX_UNDOCK_BATCH_SIZE = 4/12`
— `server/src/services/structure/structureAutoCommands.js:30-36`.
Measured undock handler cost: 212 ms cold / 110 ms warm in Obe (30000205)
— `doc/TICK_STALL_SCENE_LOAD.md:115`.

## 2.19 Multiple stations per solar system

**Yes — fully supported, N per system.**

- `worldData.buildMaps` builds `stationsById` (stationID → row) and `stationsBySystem`
  (solarSystemID → array, push per station), then **sorts each system's array ascending by
  `stationID`** — `server/src/space/worldData.js:56-62`, `:117-119`
- Lookups: `getStationByID` (`:177-179`), `getStationsForSystem` returns a **copy** of the array
  (`:200-204`), `getStationsForOwner` filters `corporationID || ownerID` across all stations
  (`:181-189`), `getStationTypeByID` (`:191-193`), plus a combined system-items accessor at
  `:257`
- Scene construction inserts **one static entity per station in the system**, in a loop
  — `server/src/space/runtime/solarSystemScene.js:143-146`
- Map/UI enumerate all of them: `mapService` spreads `getStationsForSystem(systemID)` into the
  solar-system item rows (`server/src/services/map/mapService.js:468`) and counts stations per
  system into `stationCountBySystemID` (`:761-770`); `corporationState` does the same
  (`server/src/services/corporation/corporationState.js:950-980`);
  `npcStationDirectory.listRegionNpcStations` walks every system of a region and every station
  of each, sorted by stationID
  (`server/src/services/structure/npcStationDirectory.js:118-134`).
  Also iterated per-system by `defaultEmpireScenery.js:426`,
  `stationDockingGeometry.js:334`, mobile-deployable placement checks,
  `npcDefaultConcordRules.js:339`, `structureDeploymentPlacement.js:233`,
  `wormholeRuntime.js:290`, `dungeonUniverseRuntime.js:950`
- Dock/undock target resolution is **purely by ID, never by system**:
  `resolveDockableLocation(locationID)` tries `worldData.getStationByID` then
  `worldData.getStructureByID`, returning
  `{kind: "station"|"structure", record, locationID, solarSystemID, label}`
  — `transitions.js:1201-1230`

### Remaining single-station assumptions (each a `[0]`-style pick, not a data-model limit)

- `buildSolarSystemSpawnState` prefers `stargates[0]`, else **`stations[0]`** as the spawn
  anchor, with `minOffset = max((station.radius || 15000) * 0.4, 5000)` and `clearance: 5000`
  — `transitions.js:1570-1582`
- `getLegacyStationNormalizationTarget` picks the **nearest** station of the system when a ship
  is stopped and snaps it to that perimeter if within
  `LEGACY_STATION_NORMALIZATION_RADIUS = 100000` — `stationDockingGeometry.js:309-343`
  (multi-station-aware)
- `stationSvc.GetSolarSystem` returns one stationID/stationName per system (the session's) —
  a single-station-shaped payload — `stationService.js:219-241`
- `getStationRecord` falls back to Jita 4-4 (`60003760`) when the ID resolves to nothing
  — `stationStaticData.js:335-344`; `homeStationID`/`cloneStationID` default to `60003760` in
  both the dock and undock patches — `transitions.js:2745-2751`, `:2931-2937`

### Station ID allocation for custom / added stations

**Confirmed absence.** There is no station-ID allocator, no ID base constant, no "add a
station" runtime path, and no content-pack station schema. Every station comes from SDE
`npcStations.jsonl` with `stationID = row._key` — `database-creator.js:2761`.
Grepping `addStation|customStation|STATION_ID_BASE|stationIDBase` across `server/`, `tools/`,
`doc/`, `config/` returns nothing. Content packs (`content-packs/*.json`) hold dungeons/sites,
not stations. The only hand-authored station data are the two `dunRotation` overrides and
`stationDockingPlacements`, both keyed by existing SDE stationIDs.
Player-built dockables are a separate concept (`structures` table / `structureState`,
`kind: "structure"`), served through the same `getStationRecord` / `resolveDockableLocation`
path. **So: adding a second station to a system means adding a row to the `stations` table with
an ID you allocate yourself, plus a locator profile (or a `dockPosition`/`dockOrientation`
pair) for its type — nothing in the runtime blocks it.**

## 2.20 What data a server must have — stations / docking

**Per station (static):** `stationID`, `stationName`, `stationTypeID`, `solarSystemID`,
`constellationID`, `regionID`, `operationID`, `corporationID` (= ownerID), `orbitID`
(+ `orbitName`, `orbitGroupID`, `orbitTypeID`, `orbitKind`), `position {x,y,z}`, `radius`,
`interactionRadius`, `security`, `dockingCostPerVolume`, `officeRentalCost`,
`maxShipVolumeDockable`, `reprocessingEfficiency`, `reprocessingStationsTake`,
`reprocessingHangarFlag`, `groupID`, `categoryID`, `stationTypeName`, `stationRaceID`,
`stationGraphicID`, `useOperationName`, `upgradeLevel`, `description`, and the placement block
`dockEntry`, `dockPosition`, `dockOrientation`, `undockDirection`, `undockPosition`,
plus optional `dunRotation [yaw, pitch, roll]` in **degrees**.

**Per station type:** `stationTypeID`, `typeName`, `groupID`, `categoryID`, `groupName`,
`raceID`, `graphicID`, `radius`, `basePrice`, `volume`, `portionSize`, `published`,
`dockEntry`, `dockOrientation`, `graphicLocationID`, `directionalLocatorCategories`,
`undockLocatorCategories`.

**Per station-type locator profile:** `stationTypeID`, `typeName`, `graphicID`,
`graphicLocationID`, `hasUndockLocators`, `directionalLocators[]` each
`{category, name, position{x,y,z}, direction{x,y,z}}`, categories `undockPoint`,
`undockPointSmall`, `undockPointLarge`, `undockPointCapital`, `undockPointSupercapital`.

**Per station service:** `serviceID`, `serviceName`, `serviceNameID`, `stationServiceItemID`,
optional `serviceMask`; plus
`stationStandingsRestrictions.restrictionsByOwnerID[ownerID][serviceID] = minimumStanding`.

**Indexes:** `stationsById`, `stationsBySystem` (array per solarSystemID sorted by stationID),
`stationTypesById`, locator profiles by `stationTypeID`.

**Per ship item (docked/undocked):** `itemID`, `locationID` (stationID when docked,
solarSystemID in space), `flagID` (`HANGAR` when docked, 0 in space), `customInfo`
(`"Undocking:<locationID>"` while leaving), `spaceState` (null when docked; else
`{systemID, position, direction, velocity, speedFraction, mode, targetPoint, targetEntityID,
followRange, orbitDistance, ...}`).

**Per character record:** `solarSystemID`, `constellationID`, `regionID`, `stationID`,
`structureID`, `homeStationID`, `cloneStationID`, `worldSpaceID`.

**Per session:** `stationid`, `stationID`, `stationid2`, `structureid`, `structureID`,
`structureTypeID`, `locationid`, `solarsystemid`, `solarsystemid2`, `constellationid`,
`regionid`, `shipid`, `shipTypeID`, `worldspaceid`, `homestationid`, `clonestationid`,
`_transitionState{kind, transitionID, targetID, startedAt}`,
`_undockInvulnerabilityCancelTimer`,
`_space{systemID, sceneKey, shipID, pendingUndockMovement, undockBootstrapMovementPrime,
undockBootstrapMovementPrimeSent, pendingUndockStructureSlimItemChangeID, simTimeMs, ...}`.

**Per ship entity in space:** `itemID`, `kind`, `typeID`, `radius`, `mass`, `inertia`,
`maxVelocity`, `position`, `direction`, `velocity`, `speedFraction`, `mode`, `targetPoint`,
`targetEntityID`, `dockingTargetID`,
`pendingDock{stationID, acceptedAtMs, completeAtMs, acceptedAtFileTime}`,
`undockInvulnerabilityActive`, `undockInvulnerabilityUntilMs`.

**Constants:** docking range **2500 m hull-to-hull**; station interaction radius default
**1000 m**; station physical radius default **15000 m** (scenery only); dock accept delay
**4000 ms**; undock invulnerability **30000 ms / 300000000 FILETIME ticks**; undock bootstrap FX
`effects.Uncloak` **6000 ms**; undock GOTO target at `position + direction * 1e16`; undock
`speedFraction 1` clamped to `1.0`; legacy station normalization radius **100000 m**; structure
fallback dock offset/padding **7500 / 3500 / 2500**; station undock fallback distance 8000 m
(present but unreachable for real stations).

**Messages:** client → server `beyonce.CmdDock([stationID])` (answer `None`),
`ship.Undock([shipID, ignoreContraband])` on a **per-station bound object**
`(stationID, 15)` (answer `None`), `structureDocking.Dock/Undock`.
server → client `OnDockingAccepted(charid, [stationID])`,
`OnDockingFinished(charid, [stationID])`,
`OnCharNowInStation` / `OnCharNoLongerInStation(stationid,
[charID, corpID, allianceID, warFactionID])`,
`OnInvulunOnUndockingUpdated(shipid, [shipID, endFileTime, durationTicks])`,
`OnInvulnCancelled(shipid, [shipID])`, Destiny `SetBallMassive`+`Stop` on dock accept, and the
undock prime batch `OnSpecialFX effects.Uncloak` / `SetBallPosition` / `SetBallMassive` /
`SetBallMass` / `SetBallVelocity` / `GotoDirection`.

---

# 3. Asteroid fields / mining

_(Lane 3 report, verbatim as delivered; paths absolute, facts only.)_


Paths are absolute. All `:LINE` cites are from the files named.

---

## A) BELT POPULATION

### A1. Static belt data (inputs)
- Belt rows come from static JSON tables `asteroidBelts` (row key `belts`) and `asteroidFieldStyles` (row key `fieldStyles`): `$EVEJS/server/src/services/_shared/referenceData.js:48-49,80-81`.
- Loader/indexes: belts by `itemID`, belts by `solarSystemID` (sorted ascending by `itemID`), field styles by string `fieldStyleID`: `$EVEJS/server/src/space/asteroids/asteroidData.js:14-45`; accessors `getBeltsForSystem`, `getBeltByID`, `getFieldStyleByID` at `:60-72`.
- Belt row fields, built from SDE `mapAsteroidBelts.jsonl` (`_key` → itemID): `itemID, typeID, groupID(9), categoryID(2), groupName("Asteroid Belt"), solarSystemID, constellationID, regionID, orbitID, position{x,y,z}, radius (default 15000), itemName ("<parent> - Asteroid Belt <orbitIndex>"), security, securityClass, celestialIndex, orbitIndex, kind:"asteroidBelt", fieldSeed = itemID` — `$EVEJS/tools/DatabaseCreator/database-creator.js:2728-2749`.
- Then `addAsteroidFieldProfile` adds, each via `seededInt(seed, salt, min, max)` = `min + sha256(`seed:salt`)[0..3] % (max-min+1)`: `fieldStyleID, fieldSeed, asteroidCount, clusterCount, fieldRadiusMeters, clusterRadiusMeters, verticalSpreadMeters, largeAsteroidCount` — `$EVEJS/tools/DatabaseCreator/database-creator.js:705-721`; `seededInt` at `:710-718` (hashInteger = sha256 readUInt32BE(0), `:709`).

### A2. Field-style selection (security → style)
`asteroidFieldStyleForBelt`: securityClass starting "W" or `solarSystemID >= 31000000` → `wormhole_standard`; security ≥ 0.45 → `empire_highsec_standard`; ≥ 0 → `empire_lowsec_standard`; else `nullsec_standard` — `$EVEJS/tools/DatabaseCreator/database-creator.js:691-704`.

Style table (`ASTEROID_FIELD_STYLES`), `$EVEJS/tools/DatabaseCreator/database-creator.js:341-461`:

| fieldStyleID | secMin/Max | asteroidCount min/max | clusterCount | fieldRadius m | clusterRadius m | verticalSpread m | innerExclusion m | largeAsteroidCount |
|---|---|---|---|---|---|---|---|---|
| empire_highsec_standard (`:343-371`) | 0.45 / 1.1 | 16 / 24 | 3–4 | 26000–34000 | 3500–7000 | 2000–5000 | 2000 | 0–1 |
| empire_lowsec_standard (`:372-400`) | 0 / 0.449999 | 20 / 30 | 4–5 | 30000–38000 | 4500–8500 | 2500–6500 | 2200 | 1–2 |
| nullsec_standard (`:401-430`) | -1 / -0.000001 | 24 / 36 | 5–6 | 34000–46000 | 5000–10000 | 3000–8000 | 2600 | 2–3 |
| wormhole_standard (`:431-460`) | -1 / 1.1 | 18 / 28 | 4–5 | 28000–40000 | 4500–9000 | 3000–7500 | 2400 | 1–3 |

Each style also carries `decorativeTypes[{typeID, weight, radiusMinMeters, radiusMaxMeters}]` (typeIDs 60559/60560/60561/60562) and `largeTypes[{typeID, weight}]` (90042, 90445, 90446) — same line ranges. The runtime decorative path is a dead stub (see A7).

### A3. Spawn trigger / respawn lifecycle
- Belts populate on **scene creation only**: `handleSceneCreated(scene)` guards on `scene._asteroidFieldsInitialized` and `config.asteroidFieldsEnabled !== true`, then loops `getBeltsForSystem(scene.systemID)` calling `populateBeltField` — `$EVEJS/server/src/space/asteroids/asteroidService.js:1168-1201`.
- `resetSceneAsteroidFields(scene, {broadcast, nowMs})` removes all generated asteroid entities (`kind==="asteroid" && beltID>0`), clears the flag and re-runs `handleSceneCreated` — `:1203-1240`, lister at `:1157-1166`.
- **No belt respawn timer, no scheduled job, no per-tick regrowth.** Belt fields are regenerated deterministically from `fieldSeed` every time the scene is built; what keeps a belt empty is only persisted depletion state, which `ensureSceneMiningState` applies by deleting any rock whose stored `remainingQuantity === 0` — `$EVEJS/server/src/services/mining/miningRuntimeState.js:1637-1650`, `:914-927`.
- Belts "repopulate on downtime": at startup `resetPersistedBeltAsteroidState()` drops every persisted mining row with `beltID > 0` (system-scoped; deletes the whole `/systems/<id>` node when nothing else remains) — `$EVEJS/server/src/services/mining/miningRuntimeState.js:1651-1711`; called from `$EVEJS/server/bootstrap.js:369-393`, gated on `stages.resetAsteroidBelts && config.asteroidBeltStartupReset === true` (default `true`, env `EVEJS_ASTEROID_BELT_STARTUP_RESET`, `$EVEJS/server/src/config/schema/mining.js:764-775`).
- Partially mined roids **do** restore across a scene rebuild (persisted `remainingQuantity` wins) but are fully restored by the startup reset. Depleted roid vanishes immediately (see B5).
- Generated **ice** sites are explicitly excluded from the belt reset and have their own respawn: `RESPAWN_DELAY_MINUTES = 360` (6 h) — `$EVEJS/server/src/services/mining/iceSystemAuthority.js:269`, `getRespawnDelayMsForSystem` `:753-757`; on depletion `completeGeneratedIceSite` sets dungeon instance lifecycle `"completed"`, `lifecycleReason:"depleted"`, `expiresAtMs = now + respawnDelayMs` (fallback 6 h) — `$EVEJS/server/src/services/mining/miningRuntimeState.js:1385-1431`.

### A4. Ore selection — the actual algorithm
Per-system ore pool, then per-belt subset, then per-roid weighted pick (`populateBeltField`, `$EVEJS/server/src/space/asteroids/asteroidService.js:1088-1155`):

1. **System pool** = `asteroidTypesBySolarSystemID` table (systemID → array of ore typeIDs), filtered to `classifyMiningMaterialType(...).kind === "ore"`, duplicate typeIDs merged by summing weight — `buildSystemOrePool` `:691-720`; weight read from first positive of `weight, spawnWeight, chance, probability, frequency, abundance, rarityWeight, quantity, count`, else 1 — `resolveRecordWeight` `:667-689`. Data source: `$EVEJS/tools/DatabaseCreator/staticTables/asteroidTypesBySolarSystemID/data.json` (from client `res:/staticdata/asteroidtypesbysolarsystemid.pickle`; counts: `systemCount 3974`, `distinctTypeCount 36`, `totalAssignmentCount 24071`; e.g. system 30000001 → [1224,1228,1230,17459,17460,17463,17464,17470,17471]). Reader: `$EVEJS/server/src/services/mining/miningVisuals.js:34-170`.
2. **Enrichment** when the system table is thin: `shouldEnrich = rawPool.length < 10 || distinctFamilyCount < 4`; adds security-band candidates with weight `common 1.5 / uncommon 1.15 / rare 0.8`, marked `enriched: true` — `buildEnrichedSystemOrePool` `:825-884`.
3. **Security class**: taken from `solarSystems.securityClass`, else derived from `securityStatus ?? security ?? trueSec`: ≥0.85→A, ≥0.65→B, ≥0.45→C, ≥0.25→D, ≥0.05→E, else F — `getSecurityMetadata` `:722-747`.
4. **Belt subset**: buckets families as common/uncommon/rare by their index in the security-class list (index ≤1 common, ≤3 uncommon, else rare; family not in list → rare) — `classifyFamilyBucket` `:768-784`; then per bucket picks `targetMin..targetMax` families (seeded Fisher–Yates shuffle) with bucket weight, and adds **every grade variant** of each chosen family — `buildBeltOreSubset` `:915-980`. `BELT_BUCKETS` = `common {targetMin 2, targetMax 4, weight 6}`, `uncommon {1,3,5}`, `rare {1,2,2.5}` — `:149-153`. Grade-variant weight multiplier: plain 1.35, `II-Grade` 1.0, `III-Grade` 0.7 — `resolveGradeVariantWeight` `:904-913`. Final per-entry `spawnWeight = bucketWeight * variantWeight`.
5. **Per-roid pick**: weighted draw over the belt subset using the belt RNG — `selectSystemOreType` `:982-998`, `pickWeightedEntry` `:249-269`.

**SECURITY_CLASS_FAMILIES** (`$EVEJS/server/src/space/asteroids/asteroidService.js:131-147`), ordered (order = rarity ranking):
- A: Veldspar, Scordite
- B: Veldspar, Scordite, Pyroxeres
- C / C1: Veldspar, Scordite, Pyroxeres, Plagioclase, Omber
- C2 / D / D1: Pyroxeres, Plagioclase, Omber, Kernite, Jaspet
- E / E1: Kernite, Jaspet, Hemorphite, Hedbergite
- F / F1: Hemorphite, Hedbergite, Dark Ochre, Gneiss
- G / G1: Dark Ochre, Gneiss, Crokite, Bistot
- H / H1: Crokite, Bistot, Arkonor, Mercoxit
(Trailing digits are stripped for fallback lookup, `:770-771`, `:786-789`.)

**ORE_FAMILY_TYPE_IDS** (`:113-129`) — family → [base, II-Grade, III-Grade]:
Veldspar [1230, 17470, 17471]; Scordite [1228, 17463, 17464]; Pyroxeres [1224, 17459, 17460]; Plagioclase [18, 17455, 17456]; Omber [1227, 17867, 17868]; Kernite [20, 17452, 17453]; Jaspet [1226]; Hemorphite [1231]; Hedbergite [21]; Dark Ochre [1223]; Gneiss [1229]; Crokite [1225, 17432, 17433]; **Bistot [] (empty — Bistot can never be enriched in; a confirmed gap)**; Arkonor [22, 17425, 17426]; Mercoxit [11396, 17869, 17870]. **Spodumain is absent from this table** (it appears only in the runtime yield templates, A5).
Family inference from a type name matches longest-family-first, allowing `"<Family> "`, `"<Family> II-Grade"`, `"<Family> III-Grade"`; else strips a trailing ` II|III-Grade` — `inferFamilyName` `:749-766`.

### A5. Second, independent ore table — runtime yield templates
`miningRuntimeState` has its own per-field-style ore template, used when a rock does not carry a direct yield type. `DEFAULT_TEMPLATE_BY_FIELD_STYLE` (`$EVEJS/server/src/services/mining/miningRuntimeState.js:68-101`), entries `{oreName, weight, quantityMultiplier}`:
- `empire_highsec_standard`: Veldspar 5/1.2, Scordite 4/1.15, Pyroxeres 3/0.95, Plagioclase 3/0.95, Omber 2/0.75, Kernite 1/0.65
- `empire_lowsec_standard`: Kernite 4/1.1, Omber 3/1.0, Jaspet 3/0.9, Hemorphite 2/0.8, Hedbergite 2/0.8
- `nullsec_standard`: Spodumain 4/1.0, Gneiss 3/0.9, Dark Ochre 3/0.85, Crokite 3/0.8, Bistot 2/0.75, Arkonor 2/0.7, Mercoxit 1/0.5
- `wormhole_standard`: Gneiss 4/1.0, Spodumain 3/0.95, Dark Ochre 3/0.9, Crokite 2/0.8, Bistot 2/0.75, Arkonor 2/0.7

Each entry is expanded over `ORE_GRADE_VARIANTS` — suffix "" ×1.0 weight, `" II-Grade"` ×0.45, `" III-Grade"` ×0.3, `" IV-Grade"` ×0.2; all `quantityMultiplier` 1 — `:46-67`, expansion `buildTemplateEntriesForOreDefinition` `:208-254` (resolved by *name* via `resolveItemByName`, filtered to `kind === "ore"`).
Style fallback by security when `entity.fieldStyleID` yields nothing: wormhole systemID range 31000000–31999999 → wormhole_standard; sec ≥ 0.45 → highsec; ≥ 0 → lowsec; else nullsec — `resolveTemplateSetForEntity` `:348-372`, `isWormholeSystemID` `:133-139`, `WORMHOLE_SYSTEM_MIN/MAX` `:44-45`.
Template pick is **deterministic, not RNG**: `seed = hash(itemID) ^ hash(beltID) ^ hash(len(fieldStyleID)*8191)`, cursor `= seed/0xffffffff * totalWeight` — `pickWeightedTemplateEntry` `:275-301`, `hashInteger` (two rounds of `imul ... 0x45d9f3b`) `:267-273`.

### A6. Ice / gas type tables (for completeness)
- Generated ice/gas sites: `TEMPLATE_NAMES` by band — ice highsec [Blue Ice, Clear Icicle, White Glaze, Glacial Mass]; ice lowsec [Dark Glitter, Glare Crust, Gelidus]; ice nullsec [Blue Ice IV-Grade, White Glaze IV-Grade, Glacial Mass IV-Grade, Clear Icicle IV-Grade, Glare Crust, Dark Glitter, Gelidus, Krystallos]; ice wormhole [] (none); gas highsec [] ; gas lowsec = 4 Mykoserocin + 4 Cytoserocin (Amber/Golden/Lime/Viridian); gas nullsec = Azure/Celadon/Malachite/Vermillion Myko+Cyto; gas wormhole = Fullerite-C50/C60/C70/C72/C84/C28/C32/C320/C540 — `$EVEJS/server/src/services/mining/miningResourceSiteService.js:42-93`. Band thresholds: wormhole 31000000–31999999, ≥0.45 highsec, ≥0 lowsec, else nullsec — `:204-221`.
- Ice-system authority maps source dungeon IDs 5496–5511 → ice type-name sets; empire "quarters" (amarr/…); `GUARANTEED_SLOTS_PER_SYSTEM = 1`; per-system `DOTLAN_SLOT_COUNT_BY_SYSTEM_ID` — `$EVEJS/server/src/services/mining/iceSystemAuthority.js:26-43`, `:45+`, `:269-300`. Row fields include `solarSystemID, regionID, constellationID, factionID, security, securityClass, securityBand, quarter, slotCount, slotCountConfidence, respawnDelayMinutes, sourceDungeonID, templateID ("client-dungeon:<id>"), siteFamily:"ice", siteKind:"anomaly", siteOrigin:"generatedmining", iceTypeNames, iceTypeIDs` — `:520-545`, `:590-610`.
- Ore **anomaly** (not belt) catalog keyed by security tier (`highsec_0_9_to_1_0` ≥0.85, `highsec_0_5_to_0_8` ≥0.45, `lowsec_0_1_to_0_4` ≥0, `nullsec_0_0_and_below`, plus excluded `wormhole_excluded`/`special_space_excluded`/`unknown_excluded`): highsec 0.5–0.8 → dungeons 1345/1349/1413 (Omber), 1347/1448/1456 (Kernite+Omber); lowsec → 1466,1464,1465,1482,1499,1505,1521,1540,1527,1533,1555,1348,1344,1350 (Jaspet/Gneiss/Dark Ochre/Crokite/Hedbergite+Hemorphite+Jaspet); nullsec → 1565,1455,1564,1562 (Arkonor+Bistot), 1558,1563,1559 (Bistot). **Note highsec 0.9–1.0 has no entry → no ore anomalies there.** `$EVEJS/server/src/services/mining/oreAnomalySecurityAuthority.js:23-92`, tier resolution `:104-135`.

### A7. Roid count, positions, geometry
`buildCurvedFieldProfile` (`$EVEJS/server/src/space/asteroids/asteroidService.js:446-613`) — a **curved ribbon (quadratic Bézier) with 1–2 lanes**, not a sphere or disc:
- Belt floor inputs with clamps: `asteroidCount >= 1`; `fieldRadiusMeters` max(12000, belt value, default 32000); `clusterRadiusMeters` max(2500, default 6000); `verticalSpreadMeters` max(900, default 4000) — `:449-461`.
- Seed: `mulberry32(hash("<itemID>:<fieldSeed>:<fieldStyleID>:curveProfile"))`, drawing 8 seeds in fixed order: density, span, width, bow, orientation, flow, visualWidth, sideOffset — `:462-472`; `createRng` `:237-239`; `hashText` `:280-290`.
- `multiBeltOrbit` = belt shares its `orbitID` with other belts; sibling angular `gapBias ∈ [-1,1]` computed from neighbour angles about the parent celestial — `resolveBeltOrbitContext` `:372-444`, `resolveOrbitSiblings` `:357-370`. Frame: `radial` (parent→belt), `tangent = normalize(-radial.z, 0, radial.x)`, `up = radial × tangent` — `:379-395`.
- **Count** = `clamp(max(authoredCount, areaCount), authoredCount, 511)` where `areaCount = round(axisSpan * ribbonWidth / densityAreaPerAsteroid)` — `:545-552`. Hard cap 511 = `STATIC_ASTEROID_ITEM_ID_STRIDE - 1`.
- Shape parameters, two profiles `FIELD_SHAPE_PROFILES.default` and `.empire_highsec_standard` (lookup by `fieldStyleID`, default otherwise) — `:38-109`, `resolveFieldShapeProfile` `:352-355`. For `empire_highsec_standard`: axisAngle −65…105° (single-belt orbit instead uses 88…96°, `:479-485`), spanFactor single 1.2–1.55 / multi 0.92–1.36 (× fieldRadius), widthFactor single 1.8–2.45 / multi 3.2–4.7 (× clusterRadius), bowFactor single 0.28–0.5 / multi 0.38–0.78 (× fieldRadius), areaPerAsteroid single 4.75e6–5.75e6 / multi 4.05e6–5.05e6, laneCount single 1 / multi 1–2, visualWidthFactor single 1.35–2.0 / multi 1.55–2.4, sideOffsetFactor single 0.58–0.88 / multi 0.36–0.74 (× fieldRadius), `verticalSpreadScale 0.12`, `alongJitterScale 0.025`, `crossJitterScale 0.36`. `default` profile numbers at `:39-73`.
- Derived: `verticalSpreadMeters *= verticalSpreadScale`; `alongJitterMeters = axisSpan * alongJitterScale`; `crossJitterMeters = visualRibbonWidth * crossJitterScale`; `laneSpacingMeters = visualRibbonWidth / laneCount`; `laneJitterMeters = visualRibbonWidth * 0.08` — `:601-606`.
- Bézier control points in the 2-D belt plane: `anchorBias = flowBias * axisSpan * (multi ? 0.26 : 0.42)`; `start = {anchorBias - halfSpan, sideOffset - bow*0.18*bowSign}`, `control = {anchorBias*0.22, sideOffset + bow*bowSign}`, `end = {anchorBias + halfSpan, sideOffset + bow*0.18*bowSign}` — `:544-588`. `sideSign` from gapBias or `hash("<itemID>:curveSide") & 1`.
- Per-roid offset `buildCurvedAsteroidOffset` (`:615-665`): ordered ratio `index/(count-1)` ± `rng/max(8,count)`; lane index `index % laneCount`, lane offset `(laneIndex - (laneCount-1)/2) * laneSpacing * 0.5 + bellJitter * laneJitter`; along-tangent jitter and cross-normal jitter both `signedBellJitter` (mean of 3 rng, re-centred, clamped ±1, `:296-302`); vertical `z = (2rng-1) * verticalSpread * (0.35 + 0.65*sin(π·ratio))`. Final world position = `belt.position + offset` — `:1030`.
- **No inner exclusion radius is enforced at runtime** (`innerExclusionRadiusMeters`, `clusterCount`, `largeAsteroidCount`, `decorativeTypes`, `largeTypes` exist in the generator table but the runtime uses none of them). `buildDecorativeFallbackAsteroidEntity` is a stub returning `null` — `$EVEJS/server/src/space/asteroids/asteroidService.js:1084-1086`.
- Determinism: `populateBeltField` creates one belt RNG `mulberry32(belt.fieldSeed || belt.itemID)` and draws the ore subset and then every roid offset from it in index order, so a belt is byte-identical every boot — `:1101-1132`.

### A8. Per-roid entity fields (output of A7)
`buildSystemOreAsteroidEntity` (`$EVEJS/server/src/space/asteroids/asteroidService.js:1009-1082`) emits:
`kind:"asteroid", generatedAsteroid:true, generatedFromSystemIDTable:true, resourceFieldSource:"systemID", itemID, typeID (visual shell), groupID, categoryID, slimTypeID/slimGroupID/slimCategoryID (the ore type), miningYieldTypeID (ore typeID), miningYieldKind:"ore", itemName, slimName, ownerID: 1, radius: max(500, shell/ore record radius, default 1800), graphicID, slimGraphicID, visualTypeID, position{x,y,z}, velocity{0,0,0}, direction{1,0,0}, beltID, fieldStyleID, staticVisibilityScope:"bubble"`.
- **itemID allocation is deterministic, not from an ID sequence**: `5_000_000_000_000 + beltID * 512 + (asteroidIndex+1)` — `buildAsteroidItemID` `:241-247`, constants `STATIC_ASTEROID_ITEM_ID_BASE = 5e12`, `STATIC_ASTEROID_ITEM_ID_STRIDE = 512` at `:35-36`.
- **The entity carries no quantity.** Quantity lives in the mining runtime state (A9). `solarSystemID` is stamped later as `entity.systemID` by `ensureSceneMiningState` — `$EVEJS/server/src/services/mining/miningRuntimeState.js:904`.
- Visual shell typeID picked from `GENERATED_ORE_ASTEROID_SHELL_TYPE_IDS = [64063..64077]` (15 entries) by `hash("<oreTypeID>:<itemID>:shell") % 15` — `:155-171`, `:1000-1007`.
- Comparable generated ice/gas mineable entity fields (`kind:"iceChunk"|"gasCloud"`, plus `generatedMiningSite, generatedMiningSiteKind, generatedMiningSiteIndex, resourceQuantity`, radius `3000 + (childIndex%5)*450` for ice / `1800 + (childIndex%4)*250` for gas) — `$EVEJS/server/src/services/mining/miningResourceSiteService.js:610-672`; site quantity range ice 1500–4500, gas 4000–12000, `DEFAULT_ICE_CHUNKS_PER_SITE 12`, `DEFAULT_GAS_CLOUDS_PER_SITE 14`, `DEFAULT_SITE_RADIUS_METERS 18000`, `DEFAULT_SITE_ANCHOR_OFFSET_METERS 120000`, ID bases ice `5.1e12` / gas `5.2e12`, strides `SITE_ID_SYSTEM_STRIDE 10000`, `SITE_ID_SITE_STRIDE 100` — `:31-41`; site positions are a **disc**: `angle` spiral + `radialDistance = sqrt(rng) * siteRadius`, `y = ±max(500, siteRadius*0.18)` — `:497-507`.

### A9. Per-roid quantity and radius
`buildMineableState` (`$EVEJS/server/src/services/mining/miningRuntimeState.js:785-870`) produces the persisted record. Fields (`normalizeStateRecord`, `:141-156`):
`version (=1), entityID, visualTypeID, beltID, fieldStyleID, yieldTypeID, yieldKind ("ore"|"ice"|"gas"), unitVolume (≥1e-6), originalQuantity, remainingQuantity, originalRadius (≥1), updatedAtMs`.
- `estimateOriginalQuantity` (`:387-427`): explicit `entity.resourceQuantity ?? mineableQuantity ?? originalQuantity` wins; otherwise `volume = clamp(radius² * miningBeltQuantityScale * templateQuantityMultiplier, miningBeltMinimumAsteroidVolumeM3, miningBeltMaximumAsteroidVolumeM3)` and `quantity = max(1, round(volume / unitVolume))`. Defaults: scale `0.08`, min volume `15000 m³`, max volume `3000000 m³` — `$EVEJS/server/src/config/schema/mining.js:388-425`.
- `originalRadius` = persisted value else `entity.radius`; `remainingQuantity` clamped to `[0, originalQuantity]`.
- Persisted-state validity gate (`isPersistedStateStillValid`, `:158-195`): entityID must match; beltID must match when both > 0; fieldStyleID must match when both present; `visualTypeID > 0`; reject if `entity.radius > 50 && persisted.originalRadius <= 1`; reject if `estimatedOriginal >= 100 && persisted.originalQuantity <= 1`.
- **Radius shrinks with remaining quantity.** `computeAsteroidRadiusFromQuantity` reproduces the client's `AsteroidQuantityToRadius` (ballparkCommon/ballRadius.py): `radius = asteroidRadiusUnitSize * exp(ASTEROID_EXP_SCALE * (max(1,q) - 1) * asteroidRadiusGrowthFactor)`, clamped to `[unitSize, asteroidMaxRadius]`, with `ASTEROID_EXP_SCALE = 4e-05`; inverse `AsteroidRadiusToQuantity` = `round(1 + ln(r/unitSize) / (SCALE*growthFactor))`. Fallback when those three dogma attributes are absent: `r = sqrt(q*unitVolume / fallbackScale)` clamped to `[max(1,fallbackMinRadius=250), max(min,fallbackMaxRadius=25000)]` — `$EVEJS/server/src/services/mining/miningMath.js:14`, `:107-190`. Attribute names: `asteroidRadiusUnitSize`, `asteroidRadiusGrowthFactor`, `asteroidMaxRadius` (`:107-122`).
- Applied radius floor: `config.miningDepletedAsteroidRadiusRatio` default `0.25` × originalRadius — `$EVEJS/server/src/services/mining/miningRuntimeState.js:554-581`; `$EVEJS/server/src/config/schema/mining.js:496-510`.
- `applyYieldPresentationToEntity` also rewrites the live entity: `visualTypeID, typeID/groupID/categoryID` (shell), `miningYieldTypeID, miningYieldKind, slimTypeID/slimGroupID/slimCategoryID, itemName/slimName` (unless `suppressSlimName`), `graphicID/slimGraphicID`, `radius` — `:485-583`.
- **Confirmed absence**: the shrunken radius is only written server-side (`applyMiningYieldRadiusCommand` just assigns `entity.radius`, `$EVEJS/server/src/space/destiny/commands/mining.js:3-5`); a `SetBallRadius` Destiny payload builder exists but is **never called** anywhere (`$EVEJS/server/src/space/destiny/stream/actions.js:197-198`, no other reference).

### A10. Persistence layout
- Table `miningRuntimeState` (`MINING_RUNTIME_VERSION = 1`), path `/systems/<solarSystemID>/entities/<entityID>` → state record — `$EVEJS/server/src/services/mining/miningRuntimeState.js:41-42`, `:739-783`, `:1618-1621`.
- Dungeon-placed rocks persist **no** row: quantities ride on the entity as `miningOriginalQuantity` / `miningRemainingQuantity` — `shouldPersistMineableState` `:594-603`, `readDungeonRockQuantities` `:605-620`, `recordDungeonRockRemainder` `:1497+`.
- `isMineableStaticEntity`: `kind === "asteroid"` ⇒ true; else type must classify as mining material or be a decorative asteroid; `generatedMiningSiteAnchor` excluded — `:872-885`.
- Scene summary fields: `systemID, trackedCount, activeCount, depletedCount, oreCount, iceCount, gasCount, activeAsteroidEntityCount` — `:1713-1758`. Startup summary fields: `systemID, securityBand, securityClass, securityStatus, beltIDs, asteroidCount, orePoolTypeIDs, beltSubsetTypeIDs, iceSiteKeys, gasSiteKeys, iceChunkCount, gasCloudCount, presentation{updatedCount, oreCount, iceCount, gasCount, otherCount, oreRemainingQuantity, iceRemainingQuantity, gasRemainingQuantity, otherRemainingQuantity, withGraphicCount}` — `$EVEJS/server/src/services/mining/miningStartupSummary.js:103-129`.

---

## B) THE MINING CYCLE

### B1. Client → server
- Service `dogmaIM` (`$EVEJS/server/src/services/dogma/dogmaService.js:514`), methods `Handle_Activate(args, session)` / `Handle_Deactivate(args, session)` — `$EVEJS/server/src/services/dogma/dogmaService/moduleOperation.js:3194`, `:3559`.
- `Activate` positional args: `[0] itemID (moduleID)`, `[1] effectName`, `[2] targetID`, `[3] repeat` — `:3194-3200`. Weapon-bank grouping remaps `itemID` to the bank master for `categoryID === 7` — `:3202-3213`. Non-cycle effect names handled separately: `online`, `onlineForStructures`, `anchorDrop`, `anchorLift`, `anchorDropForStructures`, `anchorLiftForStructures` — `:3218-3296`.
- Mining survey scanner: service `miningScanMgr`, method `perform_scan` (no args) — `$EVEJS/server/src/services/mining/miningScanMgrService.js:11-19`.
- Mining effect identification: effect `name` ∈ {`mininglaser`, `miningclouds`} (lower-cased), or, with no effect record, any module whose `miningAmount` attribute > 0 — `$EVEJS/server/src/services/mining/miningDogma.js:55-85`. Family: effect `miningclouds` → gas; else module name contains "gas" → gas, "ice" → ice, else ore — `resolveMiningFamily` `:87-106`.

### B2. Activation gate (state machine entry)
`resolveMiningActivation(scene, entity, moduleItem, effectRecord, {targetID})` — `$EVEJS/server/src/services/mining/miningRuntime.js:703-790`. Ordered rejections (`errorMsg`):
1. `TARGET_REQUIRED` — targetID ≤ 0
2. `TARGET_NOT_FOUND` — no entity, `airMiningTargetable === false`, no mineable state, `remainingQuantity <= 0`, or `!canEntitiesInteractLocally`
3. `TARGET_NOT_LOCKED` — targetID not in `scene.getTargetsForEntity(entity)`
4. `UNSUPPORTED_MODULE` — snapshot build failed (e.g. `miningAmount <= 0`)
5. `TARGET_INVALID_FOR_MODULE` — family/yieldKind mismatch (`isFamilyCompatibleWithYield`, `:683-694`)
6. `CHARGE_NOT_COMPATIBLE` — charge not valid for module, or crystal `specializationAsteroidTypeList` doesn't match the ore (`isCrystalTargetCompatibleWithYield`, `:662-681`; crystals only match `yieldKind === "ore"`)
7. `TARGET_OUT_OF_RANGE` — `surfaceDistance > maxRangeMeters + graceMeters(1)`; returns `rangeDecision {desiredRange, actualDistance, graceMeters: 1, sampledAtSimTimeMs}`
On success returns `runtimeAttrs {capNeed, durationMs, durationAttributeID, reactivationDelayMs, maxGroupActive, weaponFamily: null, miningSnapshot}`.
- Effect state flags written on activation include `miningEffect: true`, `chargeTypeID`, `deactivationRequestedAtMs: 0`, `deactivateAtMs: 0`, `stopReason: null`, `isGeneric: true` — `$EVEJS/server/src/space/runtime/scene/modules.js:2573-2621`.
- Distance is **surface-to-surface**: `|p1-p2| - r1 - r2`, clamped ≥ 0, evaluated at command time (`scene.getCommandTimeEntitySurfaceDistance` → `getEntitySurfaceDistance` → local fallback) — `$EVEJS/server/src/services/mining/miningRuntime.js:249-280`.

### B3. Dogma: cycle time, yield, modifiers
`buildMiningModuleSnapshot` — `$EVEJS/server/src/services/mining/miningDogma.js:108-277`. Attribute names and fallback IDs (`:31-54`):
`capacitorNeed` 6, `duration` 73, `speed` 51, `maxRange` 54, `miningAmount` 77, `maxGroupActive` 763, `moduleReactivationDelay`/`reactivationDelay` 669, `miningWastedVolumeMultiplier` 2865, `miningWasteProbability` 2864, `miningCritChance` 2868, `miningCritBonusYield` 2869, `crystalVolatilityChance` 783, `crystalVolatilityDamage` 784, `specializationAsteroidTypeList` 3148, `specializationAsteroidYieldMultiplier` 782.
- **Cycle time**: `durationMs = max(1, round6(duration > 0 ? duration : speed))`, with `durationAttributeID` recording which one was used — `:197-200`.
- **Yield per cycle**: `miningAmountM3 = max(0, round6(miningAmount))`; snapshot is rejected (`null`) when it is 0 — `:201-207`. There is **no `specialtyMiningAmount` attribute in this codebase** (confirmed absent); crystal specialisation comes through `specializationAsteroidYieldMultiplier` (ID 782) read off the charge as `crystalYieldMultiplier`, and `specializationAsteroidTypeList` (3148) as `crystalTargetTypeListID` — `:265-272`. Note `crystalYieldMultiplier` is stored but **not used** in `executeMiningCycle`; the crystal's effect reaches yield only through the dogma modifier pass (`applyOtherItemModifiersToAttributes(moduleAttributes, chargeItem)`, `:168`).
- **Modifier stack, in order** (`:144-195`): ship modifier attributes from `collectShipModifierAttributes(shipItem, skillMap, activeModuleContexts, {additionalDirectModifierEntries: implantShipModifierEntries})`; then `buildLocationModifiedAttributeMap(module, ship, skillMap, shipModifierAttributes, fittedItems, activeModuleContexts, {excludeItemID: moduleID, additionalLocationModifierSources: [...implantLocationModifierSources, ...extra]})`; then `applyOtherItemModifiersToAttributes(attrs, chargeItem)`; then `applyModifierGroups(attrs, additionalModifierEntries)` (command bursts). Charge attributes get the mirrored treatment. **No hardcoded skill/implant/rig multipliers anywhere** — skills, implants (`getActiveImplantLocationModifierSources`, `getActiveImplantShipModifierEntries`, `$EVEJS/server/src/services/mining/miningDogma.js:26-29`), rigs/fitted modules, command bursts and system-wide (wormhole/storm) effects are all generic dogma modifiers. The named examples in comments (Mining + Astrogeology +56 % on mining-drone yield) are documentation only — `$EVEJS/server/src/space/combat/weaponDogma.js:485-500`, `$EVEJS/server/src/services/drone/droneDogma.js:657-666`.
- Snapshot output fields: `family, moduleID, moduleTypeID, chargeItemID, chargeTypeID, chargeQuantity, effectID, effectName, effectGUID, durationMs, durationAttributeID, capNeed, maxRangeMeters, maxGroupActive, reactivationDelayMs, miningAmountM3, wasteVolumeMultiplier, wasteProbability, critChance, critQuantityMultiplier, crystalVolatilityChance, crystalVolatilityDamage, crystalTargetTypeListID, crystalYieldMultiplier, moduleAttributes, chargeAttributes, shipModifierAttributes` — `:209-276`.
- Additional system-wide source: `getLocationModifierSourcesForSystem(entity.systemID)` — `$EVEJS/server/src/services/mining/miningRuntime.js:445-447`.

### B4. Per-cycle math
`computeMiningResult(options)` — `$EVEJS/server/src/services/mining/miningMath.js:33-105`:
- `normalVolume = volume * clampFactor`
- residue: if `normalVolume > 0 && wasteProbability > randomInt(0..99)` then `wastedVolume = normalVolume * wasteVolumeMultiplier * efficiency` (`wasteProbability` is a raw 0–100 percentage)
- `normalQuantity = min(asteroidQuantity, max(0, randomizedRound((normalVolume/unitVolume) * efficiency)))`
- `wastedQuantity = min(asteroidQuantity - normalQuantity, randomizedRound(wastedVolume/unitVolume))`
- crit: if `normalQuantity > 0 && critProbability > rng()` (0–1 scale) then `criticalHitQuantity = randomizedRound(normalQuantity * critQuantityMultiplier)`
- `randomizedRound(v)` = floor, then +1 if the fractional remainder > rng() — `:23-31`
- Returns `clampFactor, normalVolume, wastedVolume, normalQuantity, wastedQuantity, criticalHitQuantity, criticalHitVolume, unitVolume, rewardedQuantity (0), rewardedVolume (0)`, plus `getTotalTransferredQuantity()` = normal + rewarded + crit and `getTotalDepletedQuantity()` = normal + wasted.

### B5. `executeMiningCycle` — the cycle boundary
`$EVEJS/server/src/services/mining/miningRuntime.js:819-1123`, driven from the scene tick at the cycle boundary (`$EVEJS/server/src/space/runtime/scene/tick.js:1539-1579`, gated on `config.miningEnabled === true && effectState.miningEffect === true`).
Order of operations and the `stopReason` each failure yields:
1. `efficiency` clamped 0..1 (default 1); `<= 0` → `"cycle"` (`:829-835`)
2. AIR-tutorial rock reconcile (`airMiningSettlement.isAirRock`), failure → `"cargo"` (`:839-844`)
3. target checks (missing / untargetable / `remainingQuantity <= 0` / not locally interactable / not locked) → `"target"` (`:845-857`)
4. snapshot rebuild or reuse of `preparedCycle` (matched on moduleID + cycleBoundaryMs); failure, offline module, or family mismatch → `"module"` (`:859-881`)
5. **range re-check every cycle**: `surfaceDistance > maxRangeMeters + 1` → `"range"` (`:882-887`)
6. charge compatibility → `"charge"`; burned-out crystal → `"ammo"` (`:889-903`)
7. destination hold resolution for players; `availableVolume <= 0` → `"cargo"` (`:905-917`)
8. clamping: `clampFactor = min(1, remainingQuantity*unitVolume / (miningAmountM3*efficiency), maxTransferQty*unitVolume / (miningAmountM3*efficiency))`; `maxTransferQty = floor(availableVolume / unitVolume)`; `<=0` → `"cargo"`; `clampFactor <= 0` → `"target"` if the rock is empty else `"cargo"` (`:919-941`)
9. `computeMiningResult` (waste and crit forced to 0 for AIR rocks), then normal and crit quantities re-clamped to the room actually left (`:943-967`); nothing transferable and nothing wasted → `"cargo"`
10. item grant: NPC → `appendNpcMiningCargo(entity, yieldTypeID, qty)`; player → `grantItemsToCharacterLocation(characterID, shipID, destinationFlagID, [{itemType: yieldTypeID, quantity: transferredQuantity}])` (AIR rocks via `airSettlement.extract`); failure → `"cargo"` (`:974-997`)
11. `syncMinedOreChangesToSession(session, shipID, changes)` (`:998-1004`)
12. `applyMiningDelta(scene, targetEntity, normalQuantity, wastedQuantity, {broadcast:true, nowMs, sourceEntity, moduleItem, quantityAdded, amountWasted, amountCritBonus, hasRewards})`; failure → `"target"` (`:1007-1025`)
13. ledger write, AIR daily goal `recordActivity(charID, "mine_ore", qty)` (ore only), seasons `trail.recordMining`, `achievementRuntime.recordMining` (`:1027-1096`)
14. crystal volatility damage applied **after** the payout so a durability failure can't make the payout repeatable (`:1098-1101`)
Success data: `{targetID, yieldTypeID, transferredQuantity, normalQuantity, criticalHitQuantity, wastedQuantity, efficiency, partialCycle, crystalResult, stopReason, depleted, nextCycleRuntimeAttrs {capNeed, durationMs, durationAttributeID}}` (`:1102-1122`).
`stopReason` promotion in the tick: cycle data `stopReason` wins, else `depleted === true` → `"miningDepleted"`; a thrown error or a missing runtime → `"mining"` — `$EVEJS/server/src/space/runtime/scene/tick.js:1556-1579`.

### B6. Server → client per cycle
- **`OnOreMined`**, sent to the miner's session only, id type `"charid"`, payload a single marshal dict — `$EVEJS/server/src/services/mining/miningRuntimeState.js:1071-1167`, send at `:1165`. Dict keys in emitted order (`:1124-1163`): `itemID, quantity_added, shipGroupID, amountCritBonus, hasRewards, oreType, oreGroupID, quantity_removed, moduleItemID, solarSystemFactionID, amountWasted, shipTypeID, moduleTypeID, solarsystemID, shipID, charID, moduleGroupID`. Semantics documented at `:1103-1117`: `quantity_removed` is the **yield alone** (`depletedQuantity - amountWasted`); the client subtracts `quantity_removed + amountWasted` from its survey-scan seed. Suppressed when all three of added/removed/wasted are 0, and when no player session resolves.
- **`OnOreMinedByOther(asteroidID, quantity)`** — two **positional** args (not a dict), id type `"charid"`, broadcast to every other session sharing the rock's `bubbleID`, skipping the miner; `quantity` is the **whole depleted amount**. Bubble is the exact visibility bound (rock registers `staticVisibilityScope: "bubble"`); rock with no bubble → nobody — `:1169-1219`.
- Inventory sync: `emitItemsChangedForSession(session, item, previousState, {idType:"charid", locationContext:["Ship", shipID, "ShipCargo"], changeDict})`; newly created stacks carry `clientCustomInfo = keyval[["isMined", true]]` and a change dict `{INV_UPDATE_LOCATION(3): 0}` — `$EVEJS/server/src/services/mining/miningRuntime.js:622-660`, `INV_UPDATE_LOCATION = 3` at `:105`.
- Module state: `OnGodmaShipEffect`, id type `"clientID"`, positional `[moduleID, effectID, when, active?1:0, active?1:0, environment, startedAt, duration, repeat, error]` — `$EVEJS/server/src/space/runtime/godmaShipEffectNotification.js:181-192`. On stop with `stopReason === "miningDepleted"` the error becomes user-error `MiningDronesDeactivatedAsteroidEmpty` with `{asteroidname: "", modulename: <typeID>}`, and `repeat` is forced to `0` (stop repeating) — `:456-461`, `:485-487`. `stopReason === "ammo"` on a weapon family → `NoCharges` with `repeat = true`.
- Crystal damage/consumption: `OnModuleAttributeChanges` / inner `OnModuleAttributeChange` with `attributeID 3` (damage) or `805` (quantity → 0), keyed on a charge tuple itemID built from `(shipID, moduleFlagID, chargeTypeID)` — `$EVEJS/server/src/services/mining/miningRuntime.js:171-247`, constants `ATTRIBUTE_ITEM_DAMAGE = 3`, `ATTRIBUTE_QUANTITY = 805` at `:107-108`.
- Rock depletion: `scene.removeStaticEntity(entityID, {broadcast, nowMs, terminalDestructionEffectID})` with `ASTEROID_DEPLETION_DESTRUCTION_EFFECT_ID = 2` for `kind==="asteroid"` or `yieldKind==="ore"`, else 0 — `$EVEJS/server/src/services/mining/miningRuntimeState.js:43`, `:585-592`, `:1000-1013`. That broadcasts Destiny actions `TerminalPlayDestructionEffect [entityID, effectID]` followed by `RemoveBalls [[ids]]` — `$EVEJS/server/src/space/destiny/presentation/removeBalls.js:54-78`, `$EVEJS/server/src/space/destiny/stream/actions.js:357-372`.
- Also on depletion: `scene.clearAllTargetingForEntity(entity, {reason:"Exploding", miningDepletionSourceEntityID, miningDepletionModuleID})` — the module that landed the killing blow gets stop reason `"miningDepleted"`, every other effect on that target gets the generic target-loss reason — `$EVEJS/server/src/services/mining/miningRuntimeState.js:974-988`, `$EVEJS/server/src/space/runtime/scene/targeting.js:774-780`; and `droneRuntime.idleMiningDronesTargeting(scene, entityID, {excludeDroneID})` — `:989-999`.

### B7. Partial cycles / short-cycling
- Manual `Deactivate` on a running mining module settles the elapsed fraction immediately: `efficiency = min(durationMs, max(0, now - startedAtMs)) / durationMs`, then `executeMiningCycle(..., {efficiency, partialCycle: true})`, then `finalizeGenericModuleDeactivation`; result recorded as `finalizeResult.data.partialMiningCycle {efficiency, elapsedMs, durationMs, result}` — `$EVEJS/server/src/space/runtime/scene/modules.js:3950-4026`.
- **Ice is excluded**: `miningModuleCanShortCycle(entity, effectState)` returns `false` only for family `"ice"` (ice harvesters yield 1 unit per *complete* cycle); unresolvable family → `true` (permissive) — `$EVEJS/server/src/services/mining/miningRuntime.js:148-163`, `resolveMiningFamilyForEffectState` `:121-146`. Gas scoops **do** short-cycle despite declaring `disallowEarlyDeactivation` (attr 906) — documented at `$EVEJS/server/src/space/runtime/scene/modules.js:3938-3949`.

### B8. Rock runs out / hold fills — exact behaviour
- Mid-cycle rock shortfall: `clampFactor` scales the cycle's volume down to what is left, `normalQuantity` is additionally hard-capped at `asteroidQuantity`, and `wastedQuantity` is capped at what remains after the yield — nothing over-mines (`$EVEJS/server/src/services/mining/miningMath.js:64-78`, `$EVEJS/server/src/services/mining/miningRuntime.js:921-941`). The rock is removed from the scene the moment `remainingQuantity <= 0`, the state row is kept with `remainingQuantity: 0` (so the next scene build re-deletes it), and the module stops with `"miningDepleted"`.
- Hold full: destination is the compatible mining hold, **never** merely the first bay with space; cargo is the fallback only for hulls with no compatible mining hold — `resolveDestinationFlagForPlayer` `$EVEJS/server/src/services/mining/miningRuntime.js:543-558`. The cycle aborts with `"cargo"` as soon as `floor(availableVolume / unitVolume) <= 0`, i.e. when a whole unit no longer fits — a sub-unit sliver stops the laser rather than looping forever (`:914-927`, and the same rule for drones at `$EVEJS/server/src/services/drone/droneRuntime.js:7288-7305`).
- Hold flags and capacity attribute names (`$EVEJS/server/src/services/mining/miningConstants.js:3-35`): `GENERAL_MINING_HOLD 134` ← `generalMiningHoldCapacity`; `SPECIALIZED_GAS_HOLD 135` ← `specialGasHoldCapacity`; `SPECIALIZED_ICE_HOLD 181` ← `specialIceHoldCapacity`; `SPECIALIZED_ASTEROID_HOLD 182` ← `specialAsteroidHoldCapacity`.
- Hold eligibility / material classification (`$EVEJS/server/src/services/mining/miningInventory.js:28-148`): gas = groupID 711 (Harvestable Cloud) or 4168 (Compressed Gas); ice = groupID ∈ {465, 903, 2022}; ore = categoryID 25 or groupID 4915 (Prismaticite), excluding groupIDs {519 Terran Artifacts, 4094 Scalable Decorative Asteroid, 4714 Non-Interactable Asteroid}. Preference order: ore→182, gas→135, ice→181, else 134, else null — `:158-188`.
- Used volume per bay = Σ `volume * (singleton ? 1 : stacksize)` — `computeUsedVolume` `$EVEJS/server/src/services/mining/miningRuntime.js:493-505`; available = `capacity - used`, clamped ≥ 0 — `:530-541`.

### B9. Mining drones
- Snapshot built from the **drone item**, effect names `{mining, miningclouds}`; `miningAmount` 77, duration 73 / fallback `speed` 51, `maxRange` 54; **residue attribute IDs differ from the module path: 3153 (`miningWastedVolumeMultiplier`) and 3154 (`miningWasteProbability`)**, not 2865/2864 — `$EVEJS/server/src/services/drone/droneDogma.js:81-105`, snapshot `:1080-1153`. Snapshot fields: `effectID, effectName, effectGUID, durationMs, miningAmountM3, wasteVolumeMultiplier, wasteProbability, maxRangeMeters, orbitDistanceMeters (default 200)`.
- Drones have **no crit**: `critQuantityMultiplier: 0, critProbability: 0, efficiency: 1` passed to `computeMiningResult` — `$EVEJS/server/src/services/drone/droneRuntime.js:7323-7337` (comment: no `miningCritChance`/`miningCritBonusYield` rows in the SDE for drones).
- Drone attributes use the `ownedEntityTarget: true` location domain (`charID` modifiers only, no `shipID`) — `$EVEJS/server/src/services/drone/droneDogma.js:648-665`; domain set `OWNED_ENTITY_LOCATION_MODIFIER_DOMAINS = {"charID"}` — `$EVEJS/server/src/space/combat/weaponDogma.js:500`.
- Drone cycle: `beginDroneMiningCycle` sets `cycleStartedAtMs`, `nextCycleAtMs`, `fxCycleKey` — `$EVEJS/server/src/services/drone/droneRuntime.js:2611-2630`; payout via `grantItemToCharacterLocation(characterID, shipID, flagID, yieldTypeRecord, qty)` then `applyMiningDelta(..., {sourceDroneID, sourceEntity: controllerEntity, moduleItem: pseudoModuleItem, moduleItemID: droneEntity.itemID, moduleTypeID, moduleGroupID})` then a ledger write — `:7353-7430`. Drone-specific stop: `resetDroneToIdle(drone, controller, {scene, stopMovement:true})`.
- **Strip miners / ice harvesters are not special-cased** for cycle time or yield: everything comes from dogma on the type. The only behavioural differentiation is the ice short-cycle ban (B7) and hold routing by `yieldKind` (B8).

### B10. Mining ledger
State: table `miningLedger`, `LEDGER_VERSION = 1`, `LEDGER_RETENTION_DAYS = 90`, `LEDGER_PRUNE_INTERVAL_MS = 5*60*1000`, FILETIME helpers (`FILETIME_TICKS_PER_MS = 10000n`, `FILETIME_EPOCH_OFFSET = 116444736000000000n`) — `$EVEJS/server/src/services/mining/miningLedgerState.js:19-29`. Paths `/characters/<characterID>` and `/observers/<observerItemID>`, `_meta.nextEntryID` — `:446-491`.
Entry fields (`normalizeLedgerEntry`, `:217-274`): `entryID, eventDate (FILETIME string), characterID, corporationID, solarSystemID, typeID, quantity, quantityWasted, quantityCritical, shipTypeID, moduleTypeID, yieldKind, observerItemID, characterVisibleAfter, observerVisibleAfter`.
Observer bucket fields: `itemID, corporationID, solarSystemID, itemName, entries[]` — `:276-289`; descriptor falls back to the structure's `ownerCorpID || ownerID`, `solarSystemID`, `name || itemName`, else `"Observer <id>"` — `:309-333`.
Write path `recordMiningLedgerEvent(payload)` rejects when `characterID <= 0 || typeID <= 0 || (quantity <= 0 && quantityWasted <= 0)`; appends to the character bucket and, when `observerItemID > 0`, also to the observer bucket — `:427-498`. Visibility delays from config `miningCharacterLedgerDelayMs` / `miningObserverLedgerDelayMs` (`$EVEJS/server/src/config/schema/mining.js:688-715`) are baked into `characterVisibleAfter` / `observerVisibleAfter` and enforced on read (`:300-307`). Sort is `eventDate` desc, then `entryID` desc (`:291-298`).
Caller payload from the laser path: `characterID, corporationID (session.corporationID || corpid), solarSystemID, typeID (yieldTypeID), quantity (transferredQuantity), quantityWasted, quantityCritical, shipTypeID, moduleTypeID, observerItemID, observerItemName, yieldKind, eventDateMs (cycleBoundaryMs)` — `$EVEJS/server/src/services/mining/miningRuntime.js:1030-1052`. NPC miners write no ledger rows (`:1027`).
Services:
- `characterMiningLedger.GetCharacterLogs` (characterID from the session, no args) → list of keyvals with `entryID, eventDate (filetime long), characterID, corporationID, solarSystemID, solarsystemID (both spellings), typeID, quantity, quantityWasted, quantityCritical, shipTypeID, moduleTypeID, observerItemID, yieldKind` — `$EVEJS/server/src/services/mining/characterMiningLedgerService.js:17-53`.
- `corpMiningLedger.GetObserversWithMiningEvents` → headers `{itemID, solarSystemID, solarsystemID, itemName}`; `corpMiningLedger.GetObserverLedger(args[0] = observerItemID)` → same entry shape as above. Both require a corporationID and `corprole & (DIRECTOR|ACCOUNTANT|JUNIOR_ACCOUNTANT)`, failing closed to an empty list — `$EVEJS/server/src/services/mining/corpMiningLedgerService.js:23-112`.
- Observer context for a cycle is resolved from the target entity — `resolveLedgerObserverContext` returns `{observerItemID, observerItemName}` — `$EVEJS/server/src/services/mining/miningRuntime.js:560-604`.

### B11. Range and survey scan
- Range source: dogma `maxRange` (ID 54) → `snapshot.maxRangeMeters`, checked at activation with a 1 m grace and again at **every** cycle boundary with `+1`; out of range mid-cycle stops the module with `stopReason: "range"` (no yield for that cycle) — `$EVEJS/server/src/services/mining/miningDogma.js:34`, `:228-231`; `$EVEJS/server/src/services/mining/miningRuntime.js:752-771`, `:882-887`.
- Survey scanner: `buildScanResultsForSession` filters visible entities to mineables that are locally interactable with `remainingQuantity > 0` and `surfaceDistance <= config.miningSurveyScanDistanceMeters` (default 250000), sorts by distance then entityID, and returns **tuples `[entityID, yieldTypeID, remainingQuantity]`** — `$EVEJS/server/src/services/mining/miningRuntime.js:1133-1184`; config at `$EVEJS/server/src/config/schema/mining.js:375-386`.
- Scene hooks: `miningRuntime.handleSceneCreated` runs resource sites → moon-mining content → `ensureSceneMiningState` → NPC mining ops; `tickScene(scene, now)` re-ensures state and ticks NPC mining with injected callbacks — `:1256-1304`.

---

## What data a server must have — belts

**asteroidBelts** (static, one row per belt): `itemID` (PK), `typeID`, `groupID`, `categoryID`, `groupName`, `solarSystemID`, `constellationID`, `regionID`, `orbitID`, `position{x,y,z}`, `radius`, `itemName`, `security`, `securityClass`, `celestialIndex`, `orbitIndex`, `kind`, `fieldSeed`, `fieldStyleID`, `asteroidCount`, `clusterCount`, `fieldRadiusMeters`, `clusterRadiusMeters`, `verticalSpreadMeters`, `largeAsteroidCount`.

**asteroidFieldStyles** (static, 4 rows): `fieldStyleID`, `name`, `securityMin`, `securityMax`, `asteroidCountMin/Max`, `clusterCountMin/Max`, `fieldRadiusMinMeters/MaxMeters`, `clusterRadiusMinMeters/MaxMeters`, `verticalSpreadMinMeters/MaxMeters`, `innerExclusionRadiusMeters`, `largeAsteroidCountMin/Max`, `decorativeTypes[{typeID, weight, radiusMinMeters, radiusMaxMeters}]`, `largeTypes[{typeID, weight}]`.

**asteroidTypesBySolarSystemID** (static): `solarSystemID → [oreTypeID, …]` (plus optional per-row `weight/spawnWeight/chance/probability/frequency/abundance/quantity/count`).

**solarSystems** (needed by the belt algorithm): `solarSystemID`, `securityStatus`/`security`/`trueSec`, `securityClass`, `factionID`, `regionID`, `constellationID`.

**celestials** (needed for the belt frame): `itemID`, `orbitID`, `position`.

**itemTypes** (per ore type): `typeID`, `groupID`, `categoryID`, `groupName`, `name`, `volume` (unit volume), `radius`, `graphicID`.

**Dogma attributes per ore type**: `asteroidRadiusUnitSize`, `asteroidRadiusGrowthFactor`, `asteroidMaxRadius`.

**miningRuntimeState** (persisted, per rock, keyed `/systems/<solarSystemID>/entities/<entityID>`): `version`, `entityID`, `visualTypeID`, `beltID`, `fieldStyleID`, `yieldTypeID`, `yieldKind`, `unitVolume`, `originalQuantity`, `remainingQuantity`, `originalRadius`, `updatedAtMs`.

**Live roid entity** (derived, never persisted): `kind`, `generatedAsteroid`, `generatedFromSystemIDTable`, `resourceFieldSource`, `itemID`, `typeID`, `groupID`, `categoryID`, `slimTypeID`, `slimGroupID`, `slimCategoryID`, `miningYieldTypeID`, `miningYieldKind`, `itemName`, `slimName`, `ownerID`, `radius`, `graphicID`, `slimGraphicID`, `visualTypeID`, `position`, `velocity`, `direction`, `beltID`, `fieldStyleID`, `staticVisibilityScope`, `systemID`, `bubbleID`.

**Config**: `asteroidFieldsEnabled`, `asteroidBeltStartupReset`, `miningBeltQuantityScale`, `miningBeltMinimumAsteroidVolumeM3`, `miningBeltMaximumAsteroidVolumeM3`, `miningDepletedAsteroidRadiusRatio`.

## What data a server must have — mining

**Module/charge dogma per type**: `duration` (73) or `speed` (51), `miningAmount` (77), `capacitorNeed` (6), `maxRange` (54), `maxGroupActive` (763), `moduleReactivationDelay` (669), `miningWastedVolumeMultiplier` (2865; drones 3153), `miningWasteProbability` (2864; drones 3154), `miningCritChance` (2868), `miningCritBonusYield` (2869), `crystalVolatilityChance` (783), `crystalVolatilityDamage` (784), `specializationAsteroidTypeList` (3148), `specializationAsteroidYieldMultiplier` (782).

**Effect records**: `effectID`, `name` (`mininglaser`, `miningclouds`; drones `mining`, `miningclouds`), `guid`.

**Ship resource state**: `cargoCapacity`, `generalMiningHoldCapacity`, `specialAsteroidHoldCapacity`, `specialIceHoldCapacity`, `specialGasHoldCapacity`; hold flagIDs 134/182/181/135 and `ITEM_FLAGS.CARGO_HOLD`.

**Per-item classification**: `categoryID` (25 = Asteroid), `groupID` (711, 4168 gas; 465/903/2022 ice; 4915 Prismaticite; blocked 519/4094/4714), `volume`, `portionSize`.

**Active effect state (per module)**: `moduleID`, `moduleFlagID`, `typeID`, `effectID`, `effectName`, `targetID`, `startedAtMs`, `durationMs`, `nextCycleAtMs`, `repeat`, `chargeTypeID`, `miningEffect`, `deactivationRequestedAtMs`, `deactivateAtMs`, `autoDeactivateAtCycleEnd`, `stopReason`, `isGeneric`, `pendingLocalReload`.

**Mining snapshot (derived per cycle)**: `family`, `moduleID`, `moduleTypeID`, `chargeItemID`, `chargeTypeID`, `chargeQuantity`, `effectID`, `effectName`, `effectGUID`, `durationMs`, `durationAttributeID`, `capNeed`, `maxRangeMeters`, `maxGroupActive`, `reactivationDelayMs`, `miningAmountM3`, `wasteVolumeMultiplier`, `wasteProbability`, `critChance`, `critQuantityMultiplier`, `crystalVolatilityChance`, `crystalVolatilityDamage`, `crystalTargetTypeListID`, `crystalYieldMultiplier`.

**miningLedger entry**: `entryID`, `eventDate`, `characterID`, `corporationID`, `solarSystemID`, `typeID`, `quantity`, `quantityWasted`, `quantityCritical`, `shipTypeID`, `moduleTypeID`, `yieldKind`, `observerItemID`, `characterVisibleAfter`, `observerVisibleAfter`; observer bucket `itemID`, `corporationID`, `solarSystemID`, `itemName`; meta `nextEntryID`; retention 90 days.

**Config**: `miningEnabled`, `miningSurveyScanDistanceMeters`, `miningCharacterLedgerDelayMs`, `miningObserverLedgerDelayMs`, `miningGeneratedIceSitesEnabled`, `miningGeneratedIceSiteLifetimeMinutes` (1440), `miningGeneratedSiteRadiusMeters` (18000), `miningIceChunksPerSite` (12), `miningGasCloudsPerSite` (14).

---

## Confirmed absences
- No belt respawn timer, scheduled job or per-tick regrowth — only deterministic regeneration at scene build plus a startup wipe of persisted depletion rows (A3).
- No `specialtyMiningAmount` attribute; crystal specialisation uses `specializationAsteroidYieldMultiplier` (782) and `specializationAsteroidTypeList` (3148); `crystalYieldMultiplier` is computed but never read in the cycle (B3).
- No hardcoded skill/implant/rig yield multipliers anywhere — everything is generic dogma (B3).
- `SetBallRadius` Destiny action is defined but never emitted; the shrinking roid radius is server-side only (A9).
- `innerExclusionRadiusMeters`, `clusterCount`, `largeAsteroidCount`, `decorativeTypes` and `largeTypes` exist in the belt generator but are unused by the runtime; the decorative fallback builder returns `null` (A7).
- `Bistot` has an **empty** typeID list in `ORE_FAMILY_TYPE_IDS`, and `Spodumain` is missing from it entirely — both appear in security class G/H and in the nullsec/wormhole yield templates, so the two tables disagree (A4/A5).
- Ore anomalies have no catalog for security ≥ 0.85 (A6).
- Strip miners and ice harvesters are not differentiated by hardcoded cycle time or yield; the only ice-specific rule is the short-cycle ban (B7, B9).

---

# 4. The sun / star

Paths relative to `$EVEJS/`.

## 4.1 What it is modelled as

**A row in the `celestials` static table that becomes a static scene entity (a Destiny static
ball).** Not client-only decoration, and not an inventory item.

Three layers:

1. **Static reference data** — the `celestials` table (one JSON blob, row key `celestials`)
   holds one row per star with `kind: "sun"`.
   `CELESTIALS: "celestials"` — `server/src/services/_shared/referenceData.js:44`; row key at
   `:78`. Loaded and indexed at `server/src/space/worldData.js:33`, `:72-78`
   (`celestialsById`, `celestialsBySystem`), sorted ascending by `itemID` at `:120-122`.
2. **Scene static entity (Destiny ball)** —
   `server/src/space/runtime/solarSystemScene.js:155-158` calls
   `buildStaticCelestialEntity` for every celestial of the system and `addStaticEntity`.
3. **Map rowset** for the client's system-items list —
   `server/src/services/map/mapService.js:466`.

## 4.2 itemID convention — NOT derived from solarSystemID

The star's `itemID` is the raw SDE `mapStars.jsonl` `_key` (the real EVE `40xxxxxx` star
itemID), copied verbatim — `tools/DatabaseCreator/database-creator.js:2654`
(`itemID: toInt(row._key)`), source file read at `:2621`.
**There is no `solarSystemID + offset` convention for the real star.** Worked example in a code
comment: system `30034971` (Henebene) has star itemID `40344201`
— `server/src/services/structure/structureState.js:4685-4687`.

Offsets *are* used for synthetic suns and props, which are a different thing:

| Thing | itemID formula | cite |
|---|---|---|
| wormhole secondary sun (e.g. magnetar) | `9_940_000_000_000 + systemID` | `server/src/services/exploration/wormholes/wormholeEnvironmentRuntime.js:25`, built `:177-183` |
| wormhole effect beacon | `9_941_000_000_000 + systemID` | same file `:26`, `:226-228` |
| authored props | `8_500_000_000_000_000 + systemID*10_000 + index + 1` | `server/src/space/authoredSpaceProps.js:25`, `:93-97` |
| billboards | base `8_410_000_000_000_000` | `server/src/space/defaultEmpireScenery.js:36` |
| gate sentries | base `8_420_000_000_000_000` | `defaultEmpireScenery.js:37` |
| station sentries | base `8_430_000_000_000_000` | `defaultEmpireScenery.js:38` |

## 4.3 The star's static record — exact fields and values

Built at `tools/DatabaseCreator/database-creator.js:2650-2671`:

| field | value | line |
|---|---|---|
| `itemID` | SDE `mapStars._key` | 2654 |
| `typeID` | SDE `mapStars.typeID` (e.g. the "Sun G5 (Yellow)" types) | 2655 |
| `groupID` | `type.groupID` else **6** | 2656 |
| `categoryID` | `type.categoryID` else **2** (Celestial) | 2657 |
| `groupName` | `type.groupName` else `"Sun"` | 2658 |
| `solarSystemID` | SDE `mapStars.solarSystemID` | 2659 |
| `constellationID`, `regionID` | copied from the system row | 2660-2661 |
| `orbitID` | **null** | 2662 |
| `position` | **hardcoded `{x:0, y:0, z:0}`** — always the system origin | 2663 |
| `radius` | `mapStars.radius`, else `type.radius`, else 0 | 2664 |
| `itemName` | `"<solarSystemName> - Star"` | 2665 |
| `security` | the system's security | 2666 |
| `celestialIndex`, `orbitIndex` | **null** | 2667-2668 |
| `kind` | `"sun"` | 2669 |

**Not ingested** from `mapStars.jsonl`: spectral class, luminosity, temperature, age, the
`statistics` block. The only reads of `mapStars` are `:2621-2624` (keeps the whole raw row in
`starsByID` to supply `sunTypeID`) and `:2650-2671`.
**Spectral class is nowhere a stored field** — the one place it exists is reverse-engineered
from the type *name* by the regex `\bSun\s+([OBAFGKM])`
— `server/src/services/sovereignty/sovStaticData.js:268`.

`solarSystems` rows carry `sunTypeID` separately
— `tools/DatabaseCreator/database-creator.js:2126`, inside `solarSystemRecord` (`:2113-2130`),
resolved via `raw.starID` → `mapStars` at `:2116`.

## 4.4 Scene entity shape (the Destiny ball)

`buildStaticCelestialEntity` — `server/src/space/runtime/staticSceneEntity.js:584-597`:

- `kind` ← `celestial.kind` (so `"sun"`), plus `itemID`, `typeID`, `groupID`, `categoryID`,
  `itemName`
- `ownerID: 1` (hardcoded)
- `radius: celestial.radius || (groupID === 10 ? 15000 : 1000)` — **fallback 1000 m** for a sun
  with no radius
- `position` cloned, `velocity: {0,0,0}`
- **No `staticVisibilityScope` field** → not bubble/grid/site scoped →
  `isIncrementalStaticVisibilityEntity` false
  (`server/src/space/runtime/visibility.js:195-201`), so
  `canSessionSeeStaticEntityForSession` falls through to true and the sun is
  **system-wide visible to every session**
  (`server/src/space/runtime/scene/visibility.js:2639-2710`, list built at `:2744-2757`)
- **No dogma/attribute set, no HP, no `graphicID`, no `mass`, no `dunRotation`**

Ball encoding for statics:

- mode `BALL_MODE.RIGID` = **11** — `server/src/space/destiny/stream/staticBallTail.js:73-75`,
  constant at `server/src/space/destiny/constants.js:17`
- flags fallback for a sun (kind is not station/stargate/orbital/container/wreck) =
  `BALL_FLAG.IS_GLOBAL` = **0x02** — `staticBallTail.js:77-87`, flags at
  `server/src/space/destiny/constants.js:24-32`. `IS_FREE` is explicitly cleared
  — `staticBallTail.js:91-94`
- RIGID tail is a single byte `0xFF`, no static header, no mini-geometry
  — `staticBallTail.js:227-235`
- the ball radius field is `entity.radius`, default 1
  — `server/src/space/destiny/stream/ballConfig.js:6-8`, used at
  `server/src/space/destiny/stream/ballEncoding.js:630`

## 4.5 How the sun reaches the client — two channels, both confirmed

### (a) Destiny `SetState` / `AddBalls2` — the sun **is** sent, as a ball plus a slimItem

- `SetState` keyval fields: `stamp`, `state` (the ball buffer), `ego`, `industryLevel`,
  `researchLevel`, `damageState`, `dbuffState`, `aggressors`, `droneState`, **`slims`**,
  `solItem`, `effectStates`, `allianceBridges`
  — `server/src/space/destiny/stream/statePayloads.js:205-225`
- The slim list is all ball entities with `omitSlimItem !== true` (`statePayloads.js:203-204`);
  the sun has no `omitSlimItem`, so it is included
- `AddBalls2` = `["AddBalls2", [[stateBuffer, list(extraBallData)]]]`, extra data filtered the
  same way — `statePayloads.js:92-132`
- The sun's slimItem keys, from `buildSlimItemDict`
  (`server/src/space/destiny/index.js:359-430`): `itemID`, `typeID`, `ownerID` (= 1),
  `name` (= `"<System> - Star"`), `groupID` (6), `categoryID` (2).
  No `graphicID` (the celestial builder sets none), no `nameID`, no `dunObjectID`
- `solItem` is the **solar system itself**, not the sun: `itemID = solarSystemID`, `typeID: 5`,
  `groupID: 5`, `categoryID: 2`, `ownerID: 1`, `locationID = constellationID`, `flagID 0`,
  `singleton 1`, `quantity -1`, `customInfo ""`
  — `server/src/space/destiny/index.js:979-993`
- Full-state entity plan: statics not marked `destinyBootstrapDelivery === "addBalls2"` stay in
  the `SetState` lane — `server/src/space/destiny/bootstrap/initialBallpark.js:41-51`,
  `:76-79`. The sun qualifies for the SetState lane.

### (b) `mapSvc.GetSolarsystemItems(solarSystemID)` — a rowset

- Handler `Handle_GetSolarsystemItems` — `server/src/services/map/mapService.js:672-679`
- Columns, in exact order — `mapService.js:137-150`:
  `groupID, typeID, itemID, itemName, locationID, orbitID, connector, x, y, z, celestialIndex,
  orbitIndex`
- Row builder `mapService.js:404-436`; for the sun:
  `groupID=6, typeID=<sun type>, itemID=<40xxxxxx>, itemName="<System> - Star",
  locationID=solarSystemID, orbitID=null, connector=false, x=y=z=0, celestialIndex=null,
  orbitIndex=null`
- Rows sorted ascending by `itemID` (`compareSolarSystemItemRows`, `:438-440`)
- **`connector` is never set true anywhere.** The only writer is the `false` literal at
  `mapService.js:452` and the ternary at `:430`; no source record carries `connector`/`isBeacon`.
  **Confirmed gap** — stargates should be `connector = True` on Tranquility.

### Confirmed absences on the wire

`sunTypeID` is **never sent to the game client**. Its only runtime read is an ops/monitoring
HTTP snapshot — `server/src/_secondary/redshift/server.js:113`, `:161`. The client's star
visual and colour are therefore derived entirely client-side from its own static data.
`solarSystems.visualEffect` is written at
`tools/DatabaseCreator/database-creator.js:2128` and **read nowhere** (single grep hit).

## 4.6 Sun-specific rules and constants

- **Group 6 = Sun**, 995 = Secondary Sun. `GROUP_SUN = 6` —
  `server/src/space/destiny/simulation/warpInPointParity.js:4`;
  `SECONDARY_SUN_GROUP_ID = 995` —
  `server/src/services/exploration/wormholes/wormholeEnvironmentRuntime.js:27`.
  Detection everywhere is `groupID === 6 || kind === "sun"` (11 occurrences)
- **Untargetable.** `NON_TARGETABLE_GROUP_IDS = {6 Sun, 7 Planet, 8 Moon, 9 Asteroid Belt,
  10 Stargate, 885 Cosmic Anomaly, 995 Secondary Sun}`
  — `server/src/space/runtime/targeting.js:32-40`; the test uses `slimGroupID` falling back to
  `groupID` (`:67-69`)
- **Warp-to is supported.** Warp stop distance for `kind === "sun"`:
  `max(radius + 5_000_000, desiredRange) + shipRadius*2`
  — `server/src/space/destiny/simulation/warpState.js:884-901`.
  (Verified directly: `planet`/`moon` use `+1_000_000`, `station` is
  `targetRadius + desiredRange + 2·shipRadius`, `stargate` is
  `max(max(2500, targetRadius*0.3), desiredRange) + 2·shipRadius`, `asteroidBelt` is
  `max(2500, desiredRange) + 2·shipRadius`, default is
  `max(max(1000, targetRadius), desiredRange) + 2·shipRadius`.)
- **Client-parity warp-in point**, sun formula:
  `x = px + (r + 100000)·cos(r)`, `y = py + r/5`, `z = pz − (r + 100000)·sin(r)`
  — `warpInPointParity.js:197-206`, dispatched at `:265-267`.
  `hasClientWarpInPoint` requires `radius >= MIN_WARP_IN_POINT_RADIUS = 90000` (`:8`,
  `:179-182`) and `categoryID === 2` (`:189-192`); asteroid belts and group 186 wrecks are
  excluded (`:162-187`)
- Warp entry point: `beyonce.CmdWarpToStuff(warpType, targetID, minRange=…, fleet=…)` →
  `spaceRuntime.warpToEntity(session, entityID, {minimumRange})`
  — `server/src/services/ship/beyonceService.js:3439-3520`
- **Bookmarkable** as a generic celestial; the bookmark stores `itemID`, `typeID`,
  `locationID = solarSystemID`, `x/y/z = null`
  — `server/src/services/bookmark/bookmarkRuntimeState.js:954-964`
- **It decloaks ships** (it is physical). The non-physical decloak exemption covers only groups
  502/885, `nonPhysicalDecloakExempt`, and `kind === "asteroidbelt"`
  — `server/src/space/runtime/shipCloak.js:97`, `:396-413`. Asteroid belts are explicitly
  exempt (`server/src/space/runtime/staticSceneEntity.js:702`); **suns are not**
- **Exclusion zones around the sun:**
  - mining-site placement: `sunRadius + 250_000` m, max over all group-6 entities
    — `server/src/services/mining/miningResourceSiteService.js:150-169`, clamp at `:266-272`
  - NPC/entry spawn clearance near a sun anchor: **250_000 m** vs **25_000 m** for other
    celestials, with `minOffset: 100_000` — `server/src/space/transitions.js:1584-1601`
    (verified directly at `transitions.js:1586-1601`)
- **Excluded as an anchor** by mission site anchors
  (`server/src/services/agent/agentMissionRuntime.js:2400-2411`), mining safe-anchor celestials
  (`server/src/services/mining/miningResourceSiteService.js:247-255`), dungeon universe anchors
  (`server/src/services/dungeon/dungeonUniverseRuntime.js:943-949`) and wormhole-site anchors
  (`server/src/services/exploration/wormholes/wormholeRuntime.js:286-291`).
  Session entry prefers a non-sun celestial first — `transitions.js:1585-1588`
- **Sov (Equinox) star records.** `starConfigurations` rows are `{starID, solarSystemID, power}`;
  the star is found by group 6 / `kind === "sun"` and keyed by `starID`
  — `server/src/services/sovereignty/sovStaticData.js:233-261`, map at
  `server/src/services/sovereignty/sovStaticAuthority.js:125`, diffed on key `"starID"` at
  `server/src/services/sovereignty/sovModernState.js:473`.
  - Star-power fallback by spectral prefix: **O 1000, B 1000, A 850, F 780, G 740, K 620,
    M 500**; clamp min 500 / max 1000; otherwise bucket
    `abs((typeID*31 + starID) % 11)` → `500 + bucket*50`
    — `sovStaticData.js:33-43`, `:263-280`
  - A build-time variant reads real `planetResources` power keyed by the sun's itemID
    — `tools/DatabaseCreator/database-creator.js:2033-2037`, `:2073-2077`
  - Sov hub placement is pinned to the sun's warp-in grid: default hub position =
    `getSunWarpInPoint(sun)` with `y + 100_000`, valid within `1_000_000` m
    — `server/src/services/sovereignty/sovHubPlacement.js:6-28`
- **A structure's `orbitID` in the map rowset is the system's star itemID** (TQ parity,
  documented with the Henebene example) — `getSolarSystemStarID` at
  `server/src/services/structure/structureState.js:4677-4684`, used at `:4699` inside
  `buildStructureMapEntry` (`:4688-4706`)

## 4.7 The full per-system scene emission (secondary context)

`getStaticSceneForSystem(solarSystemID)` concatenates, in this order
— `server/src/space/worldData.js:254-264`: **stations, player structures, asteroid belts,
celestials, moon-mining points, stargates**.

Scene hydration order in `SolarSystemScene`
— `server/src/space/runtime/solarSystemScene.js:142-181`:

1. stations → `buildStaticStationEntity` (`staticSceneEntity.js:355-...`; `ownerID =
   corporationID || 1`, `radius = getStationInteractionRadius`, `dockPosition` /
   `dockOrientation`)
2. player structures → `buildStaticStructureEntity` (`staticSceneEntity.js:419`)
3. asteroid belts → `buildStaticAsteroidBeltEntity` (`staticSceneEntity.js:699-713`;
   `radius || 15_000`, `nonPhysicalDecloakExempt: true`, `ownerID: 1`)
4. **celestials (sun, planets, moons — and the belt rows again)** →
   `buildStaticCelestialEntity` (`staticSceneEntity.js:584-597`)
5. moon-mining beacons → `buildStaticMoonMiningBeaconEntity` (`staticSceneEntity.js:599-615`;
   `itemID = point.beaconID`, `radius: 0`, carries `moonID`, `solarSystemID`,
   `moonMiningPointDatasetVersion`)
6. stargates, gated by `INCLUDE_STARGATES_IN_SCENE = true` (`staticSceneEntity.js:122`) →
   `buildStaticStargateEntity` (`staticSceneEntity.js:715-760`)
7. `defaultEmpireScenery.getConfiguredStaticEntitiesForSystem` — kinds `"billboard"` and
   `"sentryGun"` (`server/src/space/defaultEmpireScenery.js:220`, `:252`, `:289`, `:330`,
   `:374`; CONCORD billboard type constant imported at `:23`)
8. `authoredSpaceProps.getConfiguredStaticEntitiesForSystem` + `campaignTrailCatalog` +
   `gallenteOutreachCatalog`, run through `lcoTransforms.applySavedTransforms`
9. planet orbitals / customs offices → `buildStaticPlanetOrbitalEntity`
   (`staticSceneEntity.js:622-...`; `categoryID = planetOrbitalState.CATEGORY_ORBITAL`, radius
   default 7500, skyhook type `81080` gets `skyhook_state` 210/0); scene hooks at
   `solarSystemScene.js:286-289`
10. control towers / force fields from the live item store (`listSystemSpaceItems`)
    — `solarSystemScene.js:182-240`

Added later by other runtimes, not by the loader: wormhole endpoints (`kind "wormhole"`),
`kind "secondarySun"` (group 995, cat 2, radius default 10_000, position =
`descriptor.environmentPosition`), `kind "effectBeacon"` (group **920**, cat 2, radius default
10_000) — `wormholeEnvironmentRuntime.js:25-30`, `:176-245`; scene reconciliation
`server/src/services/exploration/wormholes/wormholeRuntime.js:1316-1400`.
Also `kind "cynoField"`, `"missile"`, `"sentryGun"`, `"billboard"`.

**No nebula / cloud prop system for known space.** `nebulaID` exists only on wormhole endpoints
(`server/src/services/exploration/wormholes/wormholeRuntimeState.js:102`); nebulae, clouds and
lens flares exist only for Abyssal and dungeon content
(`server/src/abyssal/catalog/abyssalProceduralEnvironment.js:6-40`,
`server/src/services/dungeon/dungeonUniverseRuntime.js:106`, `:1515`).

### Data source: JSON tables, not SQLite, and **no `mapDenormalize`**

- **`mapDenormalize` does not appear anywhere in this repo** (grep over `server/src`, `tools`:
  zero hits). **Confirmed absence** — do not look for an SDE-shaped map schema here.
- Static tables are per-table JSON at `<dataDir>/<tableName>/data.json`
  — `server/src/services/_shared/staticJsonTableReader.js:10-19`; dataDir resolution
  (`EVEJS_GAMESTORE_DATA_DIR` → `_local/gameStore/data` → `server/src/gameStore/data`)
  — `server/src/gameStore/storeRoot.js:41-62`. SQLite (`gamestore.sqlite`) holds only runtime
  tables — `storeRoot.js:14-18`.
- Built from CCP's public JSONL SDE by `tools/DatabaseCreator/database-creator.js`, reading
  `_sde.jsonl`, `categories`, `groups`, `types`, `dogmaAttributes`, `dogmaEffects`, `typeDogma`,
  `mapConstellations`, `mapRegions`, **`mapStars`**, **`mapSolarSystems`**, `mapStargates`,
  **`mapPlanets`**, **`mapMoons`**, **`mapAsteroidBelts`**, `npcCorporations`, `npcCharacters`,
  `agentTypes`, `stationOperations`, `npcStations` — `:2591-2760`.
  Build referenced: `eve-online-static-data-3294658-jsonl`
  (`server/src/_secondary/redshift/server.js:35-37`), client 3396210
  (`server/src/services/sovereignty/sovStaticData.js:29`).
- The generated `celestials` / `solarSystems` `data.json` files are **not checked into the
  repo** (`server/src/gameStore/data/` holds only `authoredSpaceProps` and `fighterAbilities`).

### Celestial row shapes per kind — all in the one `celestials` table

| kind | groupID default | source jsonl | extra fields | lines |
|---|---|---|---|---|
| `sun` | 6 | `mapStars` | — ; position forced `0,0,0` | 2650-2671 |
| `planet` | 7 | `mapPlanets` | `orbitID`, `celestialIndex`, `orbitIndex`; name `"<System> <RomanNumeral>"` | 2672-2695 |
| `moon` | 8 | `mapMoons` | name `"<Planet> - Moon <orbitIndex>"` | 2696-2720 |
| `asteroidBelt` | 9 | `mapAsteroidBelts` | `radius` default **15000**, `securityClass`, `fieldSeed = _key`, plus `addAsteroidFieldProfile` → `fieldStyleID, asteroidCount, clusterCount, fieldRadiusMeters, clusterRadiusMeters, verticalSpreadMeters, largeAsteroidCount` | 2721-2752; profile `:708-722` |
| stargate (separate `stargates` table) | 10 | `mapStargates` | `radius: 15000` hardcoded, `destinationID`, `destinationSolarSystemID`, `destinationName`; name `"Stargate (<DestSystem>)"` | 2635-2649 |

**Duplication bug worth noting:** the `asteroidBelts` table is a *filter* of the same array
(`authority.celestials.filter(kind === "asteroidBelt")`,
`tools/DatabaseCreator/database-creator.js:2957`) while `celestials` is written unfiltered
(`:3022`), so belts land in **both** tables. The scene dedupes by `itemID` in `addStaticEntity`
(`server/src/space/runtime/scene/staticEntities.js:61-63`), but `buildSolarSystemItemRows` maps
both lists (`server/src/services/map/mapService.js:466-467`) and does **not** dedupe →
**duplicate belt rows in `GetSolarsystemItems`**.

### `solarSystems` row fields

`tools/DatabaseCreator/database-creator.js:2113-2130`:
`regionID, constellationID, solarSystemID, solarSystemName, position{x,y,z},
security (from securityStatus), factionID (system → constellation → region fallback), radius,
sunTypeID, securityClass, visualEffect?`

## 4.8 What data a server must have — the sun

**Star static record** (one per solar system), table `celestials`, rows with `kind = "sun"`:
`itemID` (SDE `mapStars._key`, `40xxxxxx` — **not** derived from solarSystemID), `typeID`,
`groupID` (6), `categoryID` (2), `groupName` ("Sun"), `solarSystemID`, `constellationID`,
`regionID`, `orbitID` (null), `position` (always `{0,0,0}`), `radius` (metres), `itemName`
(`"<System> - Star"`), `security`, `celestialIndex` (null), `orbitIndex` (null), `kind`
("sun").

**Solar-system record**, table `solarSystems`: `solarSystemID`, `solarSystemName`, `regionID`,
`constellationID`, `position{x,y,z}`, `security`, `securityClass`, `factionID`, `radius`,
`sunTypeID`, `visualEffect?`.

**Scene-entity fields the Destiny layer needs per sun:** `kind`, `itemID`, `typeID`, `groupID`,
`categoryID`, `itemName`, `ownerID` (1), `radius` (fallback 1000), `position`, `velocity`
(zero); ball `mode = RIGID (11)`, `flags = IS_GLOBAL (0x02)` with `IS_FREE` cleared, RIGID tail
`0xFF`; no `staticVisibilityScope` (⇒ system-wide), no `omitSlimItem`, no dogma attributes, no
HP/damage state.

**Wire fields:**
`mapSvc.GetSolarsystemItems` rowset columns `groupID, typeID, itemID, itemName, locationID,
orbitID, connector, x, y, z, celestialIndex, orbitIndex` (sorted by `itemID`);
Destiny `SetState` fields `stamp, state, ego, industryLevel, researchLevel, damageState,
dbuffState, aggressors, droneState, slims, solItem, effectStates, allianceBridges`, with the
sun's slim dict keys `itemID, typeID, ownerID, name, groupID, categoryID`;
`solItem` (the system itself) `itemID = solarSystemID, typeID 5, groupID 5, categoryID 2,
ownerID 1, locationID = constellationID, flagID 0, contraband false, singleton 1, quantity -1,
customInfo ""`.

**Constants to reproduce:** groups 6 (Sun) / 995 (Secondary Sun) / 920 (effect beacon) / 7 / 8 /
9 / 10 / 885; category 2; untargetable group set `{6,7,8,9,10,885,995}`; warp stop distance
`max(radius + 5_000_000, minRange) + 2·shipRadius`; sun warp-in point
`(px + (r+100000)·cos r, py + r/5, pz − (r+100000)·sin r)`; `MIN_WARP_IN_POINT_RADIUS = 90_000`;
sun exclusion radius `radius + 250_000`; NPC/entry clearance 250_000 (sun) vs 25_000 with
`minOffset 100_000`; sov `starConfigurations {starID, solarSystemID, power}` with the spectral
power table O/B 1000, A 850, F 780, G 740, K 620, M 500 (clamp 500-1000); structure map rows
carry `orbitID = star itemID`.

**Not implemented / absent — do not look for it here:** `mapDenormalize` and any SQLite map
schema; spectral class / luminosity / temperature as stored fields; server→client transmission
of `sunTypeID`; any consumer of `solarSystems.visualEffect`; `connector = true` for any row;
sun dogma attributes; sun HP / damage state; nebula or cloud props in known space.
