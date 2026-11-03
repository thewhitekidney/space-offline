# Inventory: corporations, sovereignty, structures and fleets (EvEJS)

This is the whole "organisations and the things they own" half of EvEJS: who a
character belongs to (corp, alliance, war, vote, office), what an organisation
holds territory with (Equinox-era sovereignty hubs, skyhooks, mercenary dens),
what it anchors in space (Upwell structures and the legacy POS/starbase
system), and how players group up (fleets). Measured: **106,208 lines across
220 files** in the five assigned trees — corporation 36,583/60, sovereignty
22,596/40 (+1 protobuf bundle), starbase 6,896/23 (+1 JSON), structure
34,855/60, fleets 5,278/10. Several machoNet entry points for this area live
*outside* the trees and are counted separately where cited:
`services/planet/posMgrService.js` (677, the only POS entry point),
`services/map/sovMgrService.js` (151), `space/modules/entosisLinkRuntime.js`
(1,219), and `_secondary/express/gatewayServices/sovereigntyGatewayService.js`
(1,107). **There are no tests and no README.md anywhere in the five trees** —
`grep -rc 'TODO\|FIXME\|NOT_IMPLEMENTED'` over all five returns 0, so the usual
"look for the stubs" heuristic gives nothing and every status below is inferred
from whether a code path persists state, mirrors into space, and has a caller.

All paths below are relative to `$EVEJS_RUN/server/src/` unless
absolute.

## Subsystems

| subsystem | their files (count + key names) | approx LOC | status | evidence |
|---|---|---|---|---|
| Corporation core (records, members, roles, titles, wallet, tax, founding, liquidation) | 27: `corporation/corporationState.js`, `corpRegistryRuntime.js`, `corporationRuntimeState.js`, `corpWalletState.js`, `corporationNotifications.js`, `corpMemberActionState.js` | 18,992 | functional | 92 `Handle_*` methods in `corporation/corpRegistryRuntime.js`; real persistence to tables `corporations`/`corporationRuntime`/`characters` (`corporation/corporationState.js:1363-1408` writes employment history on affiliation change); role bitmask authority at `corporation/corporationRuntimeState.js:1736-1820` |
| Alliances (founding, applications, departure, governance, billing, war inheritance) | 14: `corporation/allianceRegistryRuntime.js`, `allianceDepartureState.js`, `allianceApplicationState.js`, `allianceFoundingState.js`, `allianceMaintenanceBilling.js` | 7,342 | functional | 55 `Handle_*` in `corporation/allianceRegistryRuntime.js`; `alliances` table with `nextCustomAllianceID` meta and `memberCorporationIDs` index (`corporation/corporationState.js:1442-1465`) |
| Wars (declare, negotiate, cost, permits, mutual, peace treaty, statistics) | 12: `corporation/warRuntimeState.js`, `warRegistryService.js`, `warNegotiationRuntimeState.js`, `warNotificationCenter.js`, `warCostState.js` | 4,946 | partial | state lives inside `corporationRuntime.wars` (`corporation/warRuntimeState.js:91-120`), not its own table; 15 handlers in `warRegistryService.js`; no war-state space/crimewatch coupling found in this tree, so whether a war actually changes legality in space is **unverified** |
| Corporation votes | 2: `corporation/voteRuntimeState.js`, `voteManagerService.js` | 1,241 | functional | own table `corporationVotes`; 13 handlers |
| Offices & station/structure rental | 7: `corporation/officeManagerService.js`, `officeRentalBilling.js`, `officeRentalSettlementState.js`, `officeImpoundReleaseState.js`, `corpStationMgrService.js`, `itemLockingService.js` | 2,815 | functional | `officeManagerService.js` requires `inventory/itemStore`, `account/walletState` and `structure/structureState` — offices are real items with rent settlement, not a rowset |
| Loyalty points | 3: `corporation/lpWalletState.js`, `lpService.js` | 1,009 | functional | own table `lpWallets` |
| Faction-warfare enlistment hook | 2: `corporation/fwCharacterEnlistmentMgrService.js`, `contactLabelState.js` | 238 | partial | sends only a `warfactionid` session change (`fwCharacterEnlistmentMgrService.js:41-50`); the FW subsystem itself is not in this tree |
| **Sov: claim, development indices, alliance territory** | 12: `sovereignty/sovState.js` (3,719), `sovStore.js`, `sovStaticData.js`, `sovNotifications.js`, `allianceSovereigntyLifecycle.js`, `sovSpaceInterop.js` | 6,970 | functional | `sovereignty` table; index maths at `sovereignty/sovState.js:63-70` (bonus tables) and `sovConstants.js:19-23` (`DEV_INDEX_LEVEL_WIDTH`, `STRATEGIC_INDEX_DAY_THRESHOLDS`); mirrors into live scenes via `sovSpaceInterop.js:9` (`space/runtime`) |
| **Sov: campaigns, command nodes, entosis capture** | 4: `sovereignty/sovCampaignState.js` (2,526), `sovCaptureState.js`, `sovSuppressionState.js`, `sovCampaignNotifications.js` | 3,957 | functional | entosis warmup/scoring state machine at `sovereignty/sovCaptureState.js:475-600`; driven from space by `space/modules/entosisLinkRuntime.js` (1,219 lines, a real dogma module effect) |
| **Sov: Equinox hub (fuel, upgrades, workforce, colony resources)** | 10: `sovereignty/sovModernState.js` (2,502), `sovHubFuel*.js` ×3, `sovHubUpgradeInstall.js`, `sovHubLossDrops.js`, `sovHubPlacement.js`, `sovUpgradeSupport.js`, `sovGatewayProto.js`, `colonyResourcesMgrService.js` | 3,951 | functional | reached over **HTTPS protobuf**, not machoNet: `_secondary/express/gatewayServices/sovereigntyGatewayService.js:48-70` lists 16+ `eve_public.sovereignty.*` request types and `sovereignty/sovGatewayProto.js:6-23` names the 16 `.proto` entry files |
| Sov: skyhooks | 4: `sovereignty/skyhookRuntime.js`, `skyhookHarvest.js`, `skyhookRaidRuntime.js`, `skyhookRaidRules.js` | 843 | functional | harvest settlement wired into `sovModernState.js:6`; orbital state from `services/planet/planetOrbitalState.js` |
| Sov: mercenary dens | 1: `sovereignty/mercenaryDenState.js` (+ `gatewayServices/mercenaryDenGatewayService.js`) | 719 | functional | own protobuf namespace `eve_public.sovereignty.mercenaryden.*` (`sovGatewayProto.js:17-20`); skill gate `MERCENARY_DEN_MANAGEMENT_SKILL_TYPE_ID = 85233` (`sovConstants.js:30`) |
| Sov: FLEX structures (Ansiblex / Pharolux / Tenebrex) | 1: `sovereignty/sovFlexStructures.js` | 603 | partial | the FLEX *structures* are owned by the structure tree; this file is the sov-upgrade gate on them |
| Sov: player deployment (anchoring from an inventory item) | 1: `sovereignty/sovPlayerDeployment.js` | 1,266 | functional | the single real anchor path, also used by Upwell structures — `structure/structureDeploymentService.js:9-14` imports `deployStructureFromInventoryItem` from it |
| Sov: GM tooling & one-time parity reset | 7: `sovereignty/sovGmState.js`, `sovChatCommands.js`, `sovAuto*.js` ×3, `sovParityReset.js`, `sovResetMarker.js` | 4,287 | functional | 16 documented `/sov …` commands at `sovereignty/sovChatCommands.js:56-71` |
| **Upwell core (records, directory, type presets, space mirror)** | 12: `structure/structureState.js` (4,827), `structureConstants.js`, `structureDirectoryService.js`, `structurePayloads.js`, `structureLocation.js`, `structureSpaceInterop.js` | 8,113 | functional | one `structures` table (`structure/structureState.js:136`); 30 structure presets with hardcoded typeIDs at `structureConstants.js:253-435`; 21 read handlers in `structureDirectoryService.js:362-582` |
| Upwell services & profiles (access lists, service modules, fuel) | 9: `structure/structureServiceModules.js`, `structureProfilesState.js`, `structureFlexFuelRuntime.js`, `structureServiceAuthority.js`, `structureSettingsService.js` | 4,375 | functional | 9 profile CRUD handlers (`structureProfilesService.js:175-290`); service-id table `STRUCTURE_SERVICE_ID` at `structureConstants.js:104-120` |
| Upwell docking, tether, control, guests | 7: `structure/structureDockingService.js`, `structureControlState.js`, `structureTetherRestrictionState.js`, `structureGuestsService.js`, `structureHangarViewState.js` | 2,045 | functional | `Handle_Dock`/`Handle_Undock` (`structureDockingService.js:137,184`); tether-state sets at `structureConstants.js:67-74`; `TakeControl`/`BoardStructure`/`EjectFromStructure` at `structureControlService.js:86-186` |
| Upwell deliveries & cargo delivery | 5: `structure/structureDeliverySaga.js`, `structureDeliveriesService.js`, `structureCargoDeliveryService.js`, `structureDeliveryRuntimeState.js` | 1,123 | functional | a saga with runtime state, not a stub |
| Upwell asset safety | 2: `structure/structureAssetSafetyState.js` (2,748), `structureAssetSafetyService.js` | 3,067 | functional | own table `structureAssetSafety` (`structureState.js:137`); 9 handlers incl. a GM timer skip (`structureAssetSafetyService.js:308`) |
| Upwell jump bridges & cyno beacons | 3: `structure/structureJumpBridgeMgrService.js` (1,933), `structureJumpBridgeLinks.js`, `structureCynoBeaconMgrService.js` | 2,791 | functional | own table `jumpBridgeSettlements`; a real two-phase jump settlement (`structureJumpBridgeMgrService.js:1235-1300`) |
| Upwell lifecycle: vulnerability, reinforcement, destruction, loot, wreck, paint | 9: `structure/structureVulnerabilityService.js`, `structureDestructionLootState.js`, `structureWreckState.js`, `structureSelfDestructRuntime.js`, `structureDeathTestRuntime.js`, `structurePaintworkState.js`, `structureDeployment*.js` | 3,399 | functional | 14 structure states incl. the three reinforcement tiers (`structureConstants.js:3-17`) |
| Moon mining & Metenox | 18: `structure/moonExtractionsService.js` (1,354), `moonCompositionPackGenerator.js`, `moonResourceAuthority.js`, `moonResourceSqliteAdapter.js`, `metenoxHarvest.js`, `autoMoonMiningService.js`, `structureAutoMoonFuelRuntime.js` | 7,734 | functional | own tables `moonExtractions`, `moonMiningFields`; a *separate SQLite adapter* (`moonResourceSqliteAdapter.js`) and a shadow-read path (`moonResourceShadowRead.js`) — a second storage engine inside this subsystem |
| Upwell GM tooling | 3: `structure/structureChatCommands.js`, `structureAuto{Commands,State}.js` | 2,208 | functional | `/upwell seed|anchor|…` at `structureChatCommands.js:174-178` |
| **POS core (anchoring policy, control, access, force field)** | 7: `starbase/starbaseAnchoringPolicy.js`, `starbaseControlRuntime.js`, `starbaseAccessRuntime.js`, `starbaseForceFieldBoundary.js`, `starbaseItemIndex.js`, `services/planet/posMgrService.js`, `services/ship/controlTowerRuntime.js` | 4,084 | functional | **POS has no table of its own** — it lives in the generic `items` table, indexed by `categoryID === 23` / `groupID === 411` (`starbase/starbaseItemIndex.js:25`), exactly the retail model; 23 handlers in `services/planet/posMgrService.js:391-645` |
| POS fuel & resources | 2: `starbase/starbaseFuelRuntime.js` (882), `starbaseResourceRuntime.js` | 1,200 | functional | consumption-factor handler at `posMgrService.js:505` |
| POS modules & inventory | 3: `starbase/starbaseModuleRuntime.js`, `starbaseInventoryRuntime.js`, `starbaseStorageKinds.js` | 908 | functional | called from dogma: `services/dogma/dogmaService/moduleOperation.js:3226,3282,3589` |
| POS combat, defence, reinforcement, repair, resistance | 6: `starbase/starbaseCombatRuntime.js`, `starbaseDefenseController.js`, `starbaseDefenseRuntime.js`, `starbaseReinforcementRuntime.js`, `starbaseRepairRuntime.js`, `starbaseResistanceRuntime.js` | 1,124 | partial | resistances read by `services/dogma/dogmaService.js`; sentry/defence settings handlers exist (`posMgrService.js:391-422`) but the only observable driver is the GM `/pos damage` command (`posChatCommands.js:34`) — **whether a real attacker reinforces a POS is unverified** |
| POS production (moon harvesting / reactions) | 4: `starbase/starbaseProductionRuntime.js`, `starbaseProductionCycles.js`, `starbaseProductionAuthority.js`, `starbaseProcessingInventory.js` | 922 | partial | 5 handlers at `posMgrService.js:586-617`, but `posChatCommands.js:39` labels `/pos production` a "server diagnostic" — their own wording |
| POS cyno jammer / travel | 1: `starbase/starbaseTravelRuntime.js` | 197 | functional | consumed by sov suppression (`sovereignty/sovSuppressionState.js` → `getJammerActivationTimeMs`) |
| POS GM tooling | 2: `starbase/posGmState.js`, `posChatCommands.js` | 1,096 | functional | 13 `/pos …` commands at `posChatCommands.js:27-39` |
| **Fleets** | 10: `fleets/fleetRuntime.js` (3,083), `fleetObjectHandlerService.js`, `fleetPayloads.js`, `fleetWarpFormation.js`, `fleetSetupRuntime.js`, `fleetWarpCommandAssociation.js`, `fleetMgrService.js`, `fleetProxyService.js` | 5,278 | functional | 40 handlers in `fleetObjectHandlerService.js:138-460`; **entirely in-memory** — `fleetRuntime.js:54-56` is three `Map`s and there is no `fleets` table; session push at `fleetRuntime.js:974-1005` |

## Session and affiliation coupling

This is the answer to the coordinator's question, and the short version is:
**affiliation is a derived projection of the character record onto the session,
and EvEJS has three different recomputation paths with three different field
sets.** Getting this wrong is silent.

### What session fields carry corp/alliance identity

The authoritative allow-list is `network/clientSession.js:58-90`
(`SESSION_CHANGE_ALLOWED_KEYS`) — a key not in that set is **dropped with a log
line and never reaches the client** (`clientSession.js:742-749`). Of its 35
keys, these eleven are the affiliation identity:

| field | written at | notes |
|---|---|---|
| `corpid` | `services/character/characterState.js:4418-4419` | also mirrored as `session.corporationID`; defaults to `1000009` when the character record has none |
| `allianceid` | `characterState.js:4420-4421` | `null`, never `0`, when unaffiliated |
| `warfactionid` | `characterState.js:4460` | |
| `corpAccountKey` | `characterState.js:4498-4499` | corp wallet division; defaults 1000 |
| `hqID` / `baseID` | `characterState.js:4477-4478` | `hqID` comes from the *corporation's* `stationID`, `baseID` from the member record |
| `rolesAtAll` / `rolesAtBase` / `rolesAtHQ` / `rolesAtOther` | `characterState.js:4481-4484` | BigInt masks |
| `corprole` | `characterState.js:4480` | **EvEJS sends this explicitly, and that contradicts `docs/reference/session-roles.md`/`service.rs:1516-1521`, which says the client derives `corprole` in `RecalculateDependantAttributesWithChanges` from the `rolesAt*` plus `stationid` vs `hqID`/`baseID`.** EvEJS computes the same derivation server-side at `corporationRuntimeState.js:1802-1808`. One of the two is redundant; resolve which before copying either. |

Every one of those eleven names is **already in eve-offline's
`Registry::character_change`** (`server/crates/eve-server/src/service.rs:1544-1557`)
except `corpAccountKey`, and it already sends them with `rolesAt* = 0`. So the
wire shape is not the gap. The gap is everything behind it.

### Who recomputes them

One function: **`applyCharacterToSession(session, charId, options)`** at
`services/character/characterState.js:4287-4601`. It is 300 lines of
"read the character record, read the corp role state, overwrite ~60 session
fields, diff them against captured `old*` locals, emit one
`SessionChangeNotification`". The corp/alliance half of its input is one call:

```
corporationRuntimeState.getCorporationSessionRoleState(charData.corporationID || 1000009, charId)
```
(`characterState.js:4385-4394`, implementation `corporationRuntimeState.js:1736-1820`).

Three callers wrap it, and **they do not do the same thing**:

1. **`refreshAffiliationSessions(characterIDs)`** — `services/chat/chatCommands.js:872-896`.
   The one the coordinator hit. Takes a **set**, scans the whole live registry
   (`sessionRegistry.getSessions()`), matches on `characterID || charid`, and
   calls `applyCharacterToSession` with `{selectionEvent: false,
   emitNotifications: true, logSelection: false}`.
   **It silently drops every role change.** `buildCharacterSessionNotificationPlan`
   gates the whole `hqID`/`baseID`/`role`/`corprole`/`rolesAt*` block on
   `if (isCharacterSelection || options.includeRoleChanges === true)`
   (`characterState.js:4137`), and `selectionEvent: false` forces
   `isCharacterSelection` false (`characterState.js:4505-4507`).
   `refreshAffiliationSessions` passes neither flag. So after `/corpcreate`
   the server-side session *has* the new CEO's director mask
   (`characterState.js:4481-4484` runs unconditionally) and the client is
   never told. Do not copy this bug.
2. **`refreshCharacterSession(characterID, options)`** — `services/corporation/corporationNotifications.js:788-806`.
   The retail path. **Single** character via
   `findSessionByCharacterID` (`services/chat/sessionRegistry.js:194`), and it
   *does* pass `includeRoleChanges: true`. Called from
   `notifyCorporationMemberChanged` (`corporationNotifications.js:962-965`),
   which is called from `corpRegistryRuntime.js:683,691,710` and
   `corpMemberActionState.js:659` — i.e. every real join/kick/role grant.
3. **`synchronizeCorporationSessionAffiliation(affectedCharacterIDs, nextAllianceID)`**
   — `services/corporation/corporationSessionAffiliationSync.js:88-253`.
   Does **not** call `applyCharacterToSession`. Sets only
   `session.allianceID`/`session.allianceid`, sends a one-key session change
   `{allianceid: [prev, next]}` (`:182-188`), and then does the thing the
   others do not: looks up the session's ship entity in its scene and
   **updates `entity.allianceID` and broadcasts a slim-item change to everyone
   else on the grid** (`:230-253`). Called from inside
   `setCorporationAlliance` (`corporation/corporationState.js:1495-1498`), so
   it runs *before* the chat command's own
   `refreshAffiliationSessions` — `/setalliance` and `/joinalliance` push the
   session twice.

There is a fourth, narrower one: `refreshCorporationHeadquartersSessions`
(`corporationNotifications.js:974-1038`) scans for sessions in a corp and pushes
only `{hqID, corprole}` when the corp moves HQ.

### How the set of affected characters is determined

Not from the session registry. From the **persisted membership index**:

- corp membership change: the set is `[characterID]` — a single element
  (`corporationState.js:1565`, `:1645`).
- alliance membership change: `getCharacterIDsInCorporation(corporationID)`
  (`corporationState.js:756-765`), which reads a cached, dirty-flagged index
  (`ensureCorporationMembersIndex`, invalidated by
  `markCorporationMembersIndexDirty()` at `corporationState.js:1404-1406`).
  `setCorporationAlliance` then loops that set calling `setCharacterAffiliation`
  on each (`corporationState.js:1480-1491`) and **returns early on the first
  failure, after having already written the alliance table and some of the
  character records** — a non-atomic multi-record write. The set is returned to
  the caller as `data.affectedCharacterIDs` (`:1498-1505`).

So: the persisted side determines the set; the session side is a scan filtered
by that set. Note `synchronizeCorporationSessionAffiliation` snapshots
`getSessions()` **once** and dedupes by object identity
(`seenSessions`, `:138-144`) so that duplicate logins of the same character
each compute their own `old` value — which implies EvEJS expects more than one
live session per characterID.

### A character who is offline when it changes

Nothing happens to them at change time, and nothing needs to. Order of
operations in `setCorporationAlliance`:

1. `alliances` table written (`corporationState.js:1467-1470`)
2. `corporations` record written (`:1472-1478`)
3. **`setCharacterAffiliation` per member** (`corporationState.js:1351-1408`) —
   this is the durable part: it writes `corporationID`, `allianceID`,
   `allianceMemberStartDate`, `startDateTime`, and appends an
   `employmentHistory` entry, into the **character record**.
4. only then the best-effort live push (`:1495-1498`).

The live push is explicitly and repeatedly documented as non-authoritative —
`corporationState.js:93-104` swallows every error with the comment
"Corporation/alliance persistence is authoritative. Live transports and space
scenes are best-effort projections and cannot roll that state back", and
`corporationSessionAffiliationSync.js:77-86` says the same. An offline
character's next login runs the ordinary `applyCharacterToSession`, which reads
the already-updated record, so they come back correct. There is deliberately
**no offline queue, no pending-change table and no login reconciliation step**.
There is one real optimisation in there worth copying: the space runtime is not
loaded at all unless a matched session actually has a `_space`
(`corporationSessionAffiliationSync.js:201-218`), so a startup seed that
re-affiliates 500 offline characters does not boot the simulation.

### What the client does when it receives the change

Server-side, `sendSessionChange` (`network/clientSession.js:727-840`) does three
things beyond sending packet type 16: it filters against the allow-list, it
**drops the packet entirely if the filtered change set is empty** unless
`allowEmpty: true` (`:761-767`) — so a no-op affiliation change is invisible
and no handler runs — and it then calls `synchronizeSessionChatState`
(`:826-840`).

`services/chat/sessionChatSync.js:92-162` is the server's own list of what a
corp/alliance change means: `corpid` and `allianceid` are in
`SCOPED_CHAT_SESSION_KEYS` (`:7-12`), so the session is re-joined to scoped
chat rooms (`syncSessionScopedRoomMembership` with
`autoJoinKinds: ['corp'|'alliance']`, `:47-64`), and both are also in
`CHAT_PRESENCE_SESSION_KEYS` along with `role` and `corprole` (`:17-21`), so
chat presence is recomputed. **Leaving the old corp channel and joining the new
one is part of the session change, not a separate step.**

Client-side, eve-offline already has the authoritative notes and they match:
`service/corp.rs:26-27` (`war_cso.OnSessionChanged` fires on any `corpid`),
`:45-50` (`crimewatchSvc.ProcessSessionChange` has a *second* `if 'corpid' in
change:` that calls `corpRegistry.GetAggressionSettings`, and an unanswered
call never releases the session-change write lock), `:257-262`
(`corp.OnSessionChanged` → `CorporationMembersO.__PrimeEveOwners` →
`corpRegistry.GetEveOwners`). Add from this side: `allianceid` reaching the
client with no `allianceRegistry` answer behind it is a dead alliance window,
and `service/inventory.rs:783` already notes the client's own `{}` fallback
when `session.allianceid` is unset.

### What eve-offline's identity split must support

Prescriptively, and in this order:

1. **A session registry keyed by characterID, returning a `Vec<&mut Session>`,
   not an `Option`.** EvEJS's is 239 lines
   (`services/chat/sessionRegistry.js`) with `getSessions()`,
   `findSessionByCharacterID()`, `indexCharacterSession()` and a
   `isPreferredCharacterSession` tie-break for duplicate logins. This is the
   single prerequisite that unblocks everything else in this inventory, and it
   is what the three `user_id: 1` sites
   (`session.rs:39`, `handshake.rs:75`, `service.rs:218`) and the ~15
   `characters.first()` sites are standing in for. Note the two layers are
   *different*: `user_id` is the **account**, `characterID` is the character,
   and a corp change is keyed on the latter. EvEJS keeps both
   (`clientSession.js` has `this.userid` separate from `this.characterID`).
2. **Character records as the durable store of affiliation, with the session as
   a pure projection.** `Registry::character_change` must read
   `(corporation_id, alliance_id, roles)` from a mutable character record, not
   from `config.characters`. The one-way-ness matters: a write goes to the
   record, then a projection pass pushes sessions. Never the reverse.
3. **A membership index: `corporation_id -> Vec<character_id>` and
   `alliance_id -> Vec<corporation_id>`**, with an explicit dirty flag. This is
   what makes the affected set computable. Do it as a derived index rebuilt on
   write (EvEJS: `ensureCorporationMembersIndex`,
   `corporationState.js:756-765`), not as a stored list that can skew.
4. **One `apply_character_to_session(&mut Session, character_id, flags)`
   function, with the role block always included.** EvEJS's split between
   `selectionEvent` and `includeRoleChanges` is the source of its own bug; make
   roles unconditional and you cannot reproduce it. Have it capture the old
   values, diff, and emit one `SessionChangeNotification`.
5. **An affiliation refresh that takes a set**:
   `refresh_affiliation(&[CharacterId])` → for each live session whose
   character is in the set, re-project and push. Offline characters are a
   no-op by construction.
6. **Chat-room resync as part of the session change**, not as a caller's
   responsibility. Mirror `sessionChatSync.js`'s key sets exactly.
7. **The empty-change trap**: if the diff is empty, EvEJS sends nothing
   (`clientSession.js:761`) and the client's handlers do not run. eve-offline's
   own `service.rs:1517-1519` already documents the client's half of this ("a
   pair whose new value equals the client's current one is dropped, and if that
   empties the map then no handler runs at all, silently"). Both halves must
   agree or you get a change that persisted but did nothing.
8. **Space-entity affiliation is a separate push.** If a ship is on a grid, its
   entity's `allianceID` is a *different copy* that other clients read, and it
   needs its own broadcast
   (`corporationSessionAffiliationSync.js:230-253`). This can be deferred
   until there is a ship in space, but the design must leave room for it:
   affiliation has two consumers, the session and the grid.

## The fifteen structure services

**One feature with sixteen entry points, not sixteen subsystems** — and the
real count is sixteen, not fifteen: `structure`, `structureDirectory`,
`structureVulnerability`, `structureCargoDelivery`, `structureDeliveries`,
`structureCynoBeaconMgr`, `structureJumpBridgeMgr`, `structureDeployment`,
`structureProfiles`, `structureControl`, `structureDocking`,
`structureHangarViewMgr`, `structureGuests`, `structureAssetSafety`,
`structureSettings`, `structureCompressionMgr` (all advertised in
`services/machoNet/machoNetService.js:208-247`, except `structureGuests` which
is at `:291` and `structureCompressionMgr` at `:242`). Evidence:

1. **One table, one record type.** `structures` (`structure/structureState.js:136`),
   normalised by one function `normalizeStructureRecord`
   (`structureState.js:800-900+`) producing one ~40-field record. Every piece
   of state the sixteen services expose — `state`, `upkeepState`,
   `serviceStates`, `profileID`, `reinforceWeekday/Hour`, `moonMiningBinding`,
   `hasQuantumCore` — is a field on that one record.
2. **Thirteen of the sixteen service files `require` `./structureState`
   directly** (measured: all but `structureDeploymentService`,
   `structureProfilesService`, `structureGuestsService`).
3. **The three that don't, delegate.** `structureDeploymentService.js` is 66
   lines and four one-line handlers forwarding to
   `sovereignty/sovPlayerDeployment.js` (`:9-14`). `structureGuestsService.js`
   is 53 lines wrapping `services/_shared/guestLists.js`.
   `structureProfilesService.js` wraps `structureProfilesState.js`.
4. **The thin ones are query facades, not subsystems.**
   `structureHangarViewMgr` = 53 lines / 1 handler;
   `structureGuests` = 53 / 1; `structureSettings` = 90 / 5;
   `structureDocking` = 242 / 2; `structureVulnerability` = 264 / **1**
   (`Handle_UpdateReinforceTimesFromClient`). 6 of the 16 expose exactly one or
   two methods.
5. **The service-name split follows the client's UI panels, not the server's
   data model.** `structureControl` (TakeControl/Board/Eject) and
   `structureDocking` (Dock/Undock) are two services doing the same thing to
   the same record from two different client windows.

The honest unit of work is the **seven structure subsystems** in the table
above (core/directory, services+profiles, docking+tether+control, deliveries,
asset safety, jump bridges+cyno, lifecycle+destruction) plus moon mining, which
is genuinely separate (its own tables and its own SQLite adapter). Counting
machoNet names here would overstate the work by roughly 2×; counting LOC
understates the *dispatch* work, because sixteen names still have to be routed.
Mirror image of the corporation tree, where **four** service names
(`corpRegistry`, `corpmgr`, `corporationSvc`, `corpRecProxy`) hide 132 handlers
and 19,000 lines.

## Sovereignty: which era?

**One era: Equinox (2024). They implement no other.** `sovState.js` and
`sovModernState.js` are not two eras; they are two layers of the same one, and
they cross-require each other.

Evidence:

- Their own statement, in the reset tool's header
  (`sovereignty/sovParityReset.js:3-13`): *"The server supports only today's
  Tranquility rules: the Sovereignty Hub is the only sovereignty structure and
  a claimed Hub is the claim. Sovereignty data from earlier releases (TCU
  claims, empty claims, display-only campaigns) is not repaired; the reset
  removes it once and records a marker."*
- `sovState.js` — the "claim and index" layer — **contains no reference to
  `TYPE_TERRITORIAL_CLAIM_UNIT` at all** (grep: zero hits) and exports
  `TYPE_INFRASTRUCTURE_HUB` only. Its logic is development indices
  (`sovConstants.js:16-18`, attributes 1583/1584/1615), the strategic index day
  thresholds `[7,21,35,65,100]` (`:20`), prime-hour and capital-system
  transition delays (`:22-23`), and entosis campaign state. That is Aegis
  (2015) *plus* Equinox, with the Equinox reading: `getInfrastructureHubClaim`,
  not `getTCUClaim`.
- `sovModernState.js` — the "hub contents" layer — is Equinox-only: power
  states, hub fuel, hub upgrades, workforce, skyhooks, colony resources.
- They coexist by design, not by accident. `sovState.js` lazily requires
  `sovModernState` at six sites (`:1753, 1811, 2577, 2904, 3255, 3649`), each
  with the comment *"at bootstrap sovModernState is half-loaded (require cycle)
  and has no cache yet"*; `sovModernState.js:119` calls back into
  `sovState.getStrategicIndexLevel`. 11 modules require both.
- The TCU type ID 32226 survives in exactly two places: the reset's deletion
  set (`sovParityReset.js:36,45`) and a deployment *refusal*
  (`sovPlayerDeployment.js:519-521`). It is a migration constant, not a feature.
- Skyhooks (81080) and mercenary dens (85230 / 85980) are the same era, and
  the hub/skyhook/den API is served over the **Equinox HTTPS protobuf
  gateway** (16 `.proto` entry files, `sovGatewayProto.js:6-23`), which is the
  strongest single piece of evidence for the era: those `.proto` files did not
  exist before Equinox.

**Porting answer: one thing, two layers.** Port `sovState` (claim + index +
campaign) and `sovModernState` (hub fuel/upgrades/workforce) as one subsystem
with an internal seam, and do not budget for a Dominion or pre-Aegis path —
there isn't one to port. The 42-file count is misleading: 4,287 of the 22,596
lines are GM tooling and a one-time migration.

## Dependency order

Inside the area:

```
session registry + character records + membership index   <-- THE gate
          |
          +-> corporation core (records, members, roles, wallet)
          |        |
          |        +-> alliances ---------+
          |        +-> votes             |
          |        +-> wars  <-----------+   (wars are declared by corp OR alliance)
          |        +-> LP
          |        +-> offices  <-- needs: item location model, wallet
          |
          +-> fleets   (needs nothing else in this area)

item location model + inventory
          |
          +-> Upwell core (structures table)
                   |
                   +-> services & profiles  <-- needs corp roles (access lists)
                   +-> docking / tether / control  <-- needs ship in space
                   +-> deliveries, asset safety
                   +-> jump bridges / cyno  <-- needs ship in space + jump mechanic
                   +-> lifecycle / destruction  <-- needs combat
                   +-> moon mining  <-- needs industry + mining
                   |
                   +-> sovereignty (sovState + sovModernState)
                             |
                             +-> campaigns / entosis  <-- needs dogma modules + space
                             +-> skyhooks  <-- needs planet orbital state
                             +-> mercenary dens
                             +-> FLEX structures
                             +-> Equinox protobuf gateway  <-- needs an HTTPS transport

items + dogma + space
          |
          +-> POS / starbase
```

Each edge, with its evidence:

- **Everything in the corporation tree ← session registry + character records.**
  Already argued above. `corporationState.setCharacterAffiliation` writes the
  *character* record (`corporationState.js:1363`); there is nowhere else for
  membership to live.
- **Alliances ← corporations.** `createCustomAllianceForCorporation` refuses on
  `corporationRecord.isNPC` (`corporationState.js:1673-1678`); an alliance's
  members are corporations (`memberCorporationIDs`, `:1453-1463`).
- **Wars ← alliances.** The war owner is `allianceID || corporationID`
  (`peaceTreatyMgrService.js:67-71`), and `allianceWarInheritance.js` (444
  lines) exists solely to move wars when a corp joins or leaves an alliance.
- **Offices ← the item location model and the wallet.**
  `officeManagerService.js` requires `inventory/itemStore` and
  `account/walletState`. An office is an item with rent, not a row.
- **Structures ← the item location model (hard).** `structureState.js:30-39`
  requires `inventory/itemStore` (`removeInventoryItem`,
  `updateInventoryItem`) and `inventory/simulationInventoryProjection`
  (`listContainerItems`); `:43-46` requires `fitting/liveFittingState`
  (`getTypeAttributeValue`). An anchor consumes an inventory item
  (`deployStructureFromInventoryItem`, `sovPlayerDeployment.js`). A structure
  is a container with hangars and an asset-safety destination; without items
  there is nothing to put in it.
- **Sovereignty ← structures (hard).** `sovSpaceInterop.js:10` and
  `sovModernState.js:28-30` both require `structure/structureState`. A
  sovereignty hub **is a row in the `structures` table** — there is no separate
  sov-structure store. Port order is fixed: structures first, always.
- **Sovereignty ← Destiny/space (hard, for capture).** Entosis capture is
  driven by a dogma module effect in the space tree
  (`space/modules/entosisLinkRuntime.js`, 1,219 lines, requires
  `services/fitting/liveFittingState`), and `sovSpaceInterop.js:9` requires
  `space/runtime` to mirror hubs into scenes.
  `sovHubPlacement.js:10-13` positions a hub off the **sun's warp-in point**
  via `space/destiny/simulation/warpInPointParity` — so even *anchoring* a hub
  needs the Destiny warp-in geometry.
- **Sovereignty (Equinox hub UI) ← an HTTPS protobuf transport.** The hub,
  upgrade, fuel, workforce, skyhook and mercenary-den APIs are **not machoNet**.
  They are protobuf over the express gateway
  (`_secondary/express/gatewayServices/sovereigntyGatewayService.js`, 1,107
  lines; `https.createServer` at `_secondary/express/server.js:540`). 8 of the
  33 gateway services are in this area (sovereignty, mercenary den, structure
  paintwork, corp goals, corp colors, corp skill plan ×2). **eve-offline has no
  such transport.** This is a whole second server, and it gates the Equinox sov
  UI entirely.
- **Skyhooks ← planet orbital state.** `sovModernState.js:9` requires
  `services/planet/planetOrbitalState` for `ORBITAL_STATE` and
  `getDeployedSkyhookRevision`.
- **Moon mining ← industry/mining and a second storage engine.**
  `structure/moonResourceSqliteAdapter.js` (424 lines) plus
  `moonResourceShadowRead.js` — this subsystem has its own SQLite path
  alongside the JSON store. Also `structureIndustryJobs.js`.
- **POS ← items + dogma + space (hard, all three).** POS state is the `items`
  table filtered on `categoryID === 23` (`starbaseItemIndex.js:25`); module
  online/offline runs through `services/dogma/dogmaService/moduleOperation.js:3226,3282,3589`;
  anchoring restrictions are a JSON policy file read by
  `starbaseAnchoringPolicy.js`; force-field boundary is geometry
  (`starbaseForceFieldBoundary.js`). **POS does not need industry** — its
  production (`starbaseProductionCycles.js`) is self-contained moon
  harvesting/reactions and their own chat command calls it a diagnostic.
- **POS ← sovereignty, weakly, one direction only.**
  `starbaseAnchoringPolicy.js` requires both `sovState` and `sovModernState`
  (anchoring is gated on sov), and `sovSuppressionState.js` reads
  `starbaseTravelRuntime.getJammerActivationTimeMs`. A cycle at the file level
  but not at the subsystem level: sov can be ported without POS.
- **Fleets ← almost nothing.** `fleetRuntime.js` requires only
  `character/characterState`, `network/protocolProfile`,
  `_shared/serviceHelpers`, `space/runtime` and the logger. All state is three
  in-memory `Map`s (`:54-56`). It touches the space runtime at exactly **two**
  call sites (`:2662-2663`, `getSceneForSession`/`getBubbleForSession`, for
  broadcast scoping). Fleet *warp* is implemented on the other side of the
  seam, in `space/runtime/scene/fleetWarp.js` (293 lines), not in this tree.

**Cannot be done before eve-offline has a ship in space:**

- Entosis capture, sov campaigns, command nodes (a module effect on a grid).
- Structure docking / undocking / tether / `TakeControl` / `BoardStructure`
  (`structureControlService.js:133-134` compares the structure's system to the
  session's).
- Jump bridges and cyno beacons (they move a ship between systems).
- All structure and POS combat, reinforcement and destruction.
- Hub anchoring (needs the sun warp-in point) and the slim-item half of an
  alliance change.
- Fleet warp and fleet broadcasts with grid scoping.

**Can be done fully docked, today, after the identity split:**

- Corporation core, alliances, votes, wars (declaration and bookkeeping),
  LP, offices.
- Fleet composition, wings, squads, invites, MOTD, roles — 40 of
  `fleetObjectHandler`'s handlers are pure bookkeeping.
- The structure *directory* (21 read handlers), profiles, settings, guests,
  asset safety, deliveries — i.e. GM-seeded structures you can query and
  configure but not fly to.
- Sov claim/index read-side (`sovMgrService`, 10 handlers) against GM-seeded
  state.

## Effort to port, per subsystem

Effort assumes the session registry and item location model already exist; add
them separately. "Risk" is specifically what could be silently wrong on
build 3619879 (EvEJS targets 3396210 — roughly V24.01-era by their own
comments, e.g. `corporationRuntimeState.js:62-64`, `:92`).

| subsystem | effort | why | risk |
|---|---|---|---|
| **Session registry + identity split** | M | ~250 lines of registry (EvEJS's is 239) plus threading a character id through ~15 call sites; no protocol work | Low protocol risk, high blast radius. The real risk is doing it half-way: one `characters.first()` left behind becomes a cross-character data leak that no test catches. |
| Corporation core | XL | 19,000 lines, 92 handlers, role bitmask authority, wallet divisions, titles, applications, employment history | **Role bitmasks.** `ALL_CORPORATION_ROLE_MASK` is 55 explicit bits (`corporationRuntimeState.js:54-106`) sourced from "V24.01 appConst.py". If 3619879 added or moved a bit, `FULL_ADMIN_ROLE_MASK` is wrong and directors silently lose permissions. Also `CORP_ROLE_ACCOUNT_CAN_TAKE_BY_DIVISION` (7 bits, wallet keys 1000-1006, `:67-70`) is order-sensitive. |
| Alliances | L | 7,300 lines but the model is thin (a record + a member-corp list); most of the volume is applications/departure/governance UI surface | Alliance ID range: `CUSTOM_ALLIANCE_ID_START` / `nextCustomAllianceID` are server-chosen. Pick a range the client's `idCheckers` agrees is an alliance or every owner lookup mis-types. |
| Wars | L | 5,000 lines, a negotiation state machine, cost model, treaties, inheritance on alliance change | War legality has to reach crimewatch and the grid to mean anything, and that coupling is **not in this tree** — port it and you may get a war that changes no behaviour. Also unverified in EvEJS itself. |
| Votes | S | 1,241 lines, own table, 13 CRUD handlers, no space coupling | Vote type enums are client-facing constants; a wrong one is an empty dialog. |
| Offices | M | 2,800 lines; the hard part is rent settlement against the wallet and impound | Office type ID `26` (`TYPE_OFFICE_RENTAL`) and the office item flag. Rent amounts are station-attribute driven. |
| LP | S | 1,000 lines, own table | LP store offer shapes. |
| Fleets | **L (and do it early)** | 5,278 lines, 40+6+8 handlers, **in-memory only — no persistence to write**, 2 lines of space coupling, needs only `characterState` | Lowest-risk large item in this inventory. The `fleetObjectHandler` is a **bound object** (`Handle_MachoBindObject`, `fleetObjectHandlerService.js:142`) with `GetInitState` — get the bind/init handshake wrong and the fleet window never populates. Fleet warp is a separate, later job. |
| Upwell core + directory | L | 8,100 lines but mostly one record type and read handlers | **30 hardcoded structure type IDs** with per-type `structureFamily`/`structureSize`/service lists (`structureConstants.js:253-435`). If 3619879 changed a preset's attributes the structure anchors with the wrong size and the client's fitting/service UI disagrees. Also 14 `STRUCTURE_STATE` values (`:3-17`) which are wire-visible enums. |
| Upwell services & profiles | M | 4,375 lines; access-list evaluation against corp roles and standings | `STRUCTURE_SERVICE_ID` (`structureConstants.js:104-120+`) is a client-facing enum; `FLEX_SERVICE_IDS_BY_TYPE_ID` (`:508`) hardcodes which FLEX type offers what. |
| Upwell docking/tether/control | M | 2,045 lines; needs a ship in space | Tether rules are a set of 7 states (`structureConstants.js:67-74`); getting the set wrong makes ships invulnerable or not, with no error either way. |
| Upwell deliveries | M | 1,123 lines, a saga | |
| Upwell asset safety | L | 3,067 lines, mostly one 2,748-line state file; timers, wraps, per-owner views | Asset-safety delay constants and the wrap item type. A wrong delay is invisible until a structure dies. |
| Jump bridges & cyno | L | 2,791 lines, two-phase settlement, own table | Ansiblex 35841 / Pharolux 35840 / Tenebrex 37534, plus `TYPE_LIQUID_OZONE = 16273` and `TYPE_JUMP_PORTAL_GENERATOR = 23953`. Jump range/fuel formulae are attribute-driven and build-sensitive. |
| Upwell lifecycle & destruction | L | 3,399 lines; needs combat | Reinforcement timer maths and `DEFAULT_STRUCTURE_REINFORCE_HOUR = 20` (`corporationRuntimeState.js:49`). Silently wrong timers are the classic structure bug. |
| Moon mining & Metenox | XL | 7,700 lines, **two storage engines** (JSON + `moonResourceSqliteAdapter.js`), composition generation, extraction rules, Metenox automation | Metenox is recent (81826 / 81920 / 82941). `TYPE_MOON_MINING_BEACON = 46329`, `TYPE_STANDUP_MOON_DRILL = 45009`. Moon composition is generated, not from SDE — the generator's output must match what the client's moon-mining UI expects. |
| **Sov: claim + index + campaign** | XL | 11,000 lines across `sovState`/`sovCampaignState`/`sovCaptureState`, a cross-require cycle, entosis state machine | **Sov constants.** Dev-index attributes 1583/1584/1615, `DEV_INDEX_LEVEL_WIDTH = 100000 * 96`, `STRATEGIC_INDEX_DAY_THRESHOLDS = [7,21,35,65,100]`, `VULNERABILITY_WINDOW_BASE_HOURS = 18`, `PRIME_TIME_CHANGE_DELAY_MS = 48h`, `CAPITAL_SYSTEM_CHANGE_DELAY_MS = 7d`, and the five bonus tables at `sovState.js:63-70`. **Every one of these is a balance number CCP tunes.** Equinox has been re-tuned repeatedly between 3396210 and 3619879; indices that are slightly wrong produce a sov system that looks right and plays wrong, with nothing to compare against. |
| **Sov: Equinox hub (fuel/upgrades/workforce)** | XL | 4,000 lines *plus a protobuf transport eve-offline does not have* | The whole subsystem is gated on 16 `.proto` schemas (`sovGatewayProto.js:6-23`) served over HTTPS. **Schema drift between 3396210 and 3619879 is the single largest silent-failure risk in this document** — protobuf decodes a changed message without complaint and hands you wrong field values. Their `.proto` bundle is a 3396210 artifact; do not trust it for 3619879 without a capture. |
| Sov: skyhooks | M | 843 lines, but depends on planet orbital state | 81080; reagent type IDs (`TYPE_COLONY_REAGENT_LAVA = 81143` and siblings); harvest rates. |
| Sov: mercenary dens | M | 719 lines + its own gateway service | 85230 / 85980, skill 85233, `DEFAULT_MERCENARY_DEN_ACTIVITY_CAPACITY = 3` / `ABSOLUTE_MAXIMUM = 5` (`sovConstants.js:31-32`). Dens are the newest feature here and the most likely to have changed. |
| Sov: FLEX structures | S | 603 lines, a gate on three type IDs | |
| **POS / starbase** | XL | 6,900 lines in-tree + 677 (`posMgrService`) + 4,100 of `controlTowerRuntime`/policy, across items, dogma and space; and it is a **legacy** system | `TYPE_COMPRESSION_ARRAY = 12239`, `TYPE_ANCHORING = 11584`, `TYPE_GENERATOR = 27673`, `TYPE_JAMMER = 27674`, `TYPE_BRIDGE/JUMP_BRIDGE = 27897`, `TYPE_NITROGEN_FUEL_BLOCK = 4051`, `categoryStarbase = 23`, force-field `groupID = 411`, plus a whole `starbaseAnchoringRestrictions.json`. CCP has been removing POS; by 3619879 some of these may be unpublished, and an unpublished type that still has a typeID anchors fine and then behaves like nothing. **Recommend deferring POS entirely** — it is the only XL item here with no end-goal payoff (friends do not want a 2008 POS). |

Rough total if everything were ported: on the order of **4-6 months of
single-lane work**, of which the identity split is days and unblocks the first
third.

## Build-delta hazards

Collected, because these are the things that compile and run and are wrong.

**Role bitmasks** — the specific thing asked about:

- eve-offline's sandbox default is `0x6168005200E41000`
  (`docs/archive/session-flow.md:133`, `roles.rs`). EvEJS's equivalent baseline is
  `SESSION_BASE_ROLE_MASK = 0x6000000080000000`
  (`services/account/accountRoleProfiles.js:137`), OR'd into **every** session
  role by `composeSessionRoleMask(accountRole, chatRole)`
  (`accountRoleProfiles.js:174-180`), called at
  `network/clientSession.js:305`, `characterState.js:4501`,
  `network/tcp/index.js:289,588`.
- Arithmetic (computed): eve-offline's mask **contains** EvEJS's
  `ROLE_LOGIN|ROLE_PLAYER` (bits 62, 61 = `0x6000000000000000`) but is
  **missing bit 31, `0x80000000`**, which EvEJS sets unconditionally. Bit 31 is
  `ROLE_IGB` per eve-offline's own table (`docs/reference/session-roles.md:319`, reader
  count 0). So this is almost certainly harmless — but it means EvEJS and
  eve-offline do **not** agree on the baseline, and any EvEJS code copied that
  assumes "the base mask is always present" is making an assumption
  eve-offline's mask does not satisfy. Decide deliberately rather than by
  accident.
- Conversely eve-offline sets bit 36 (`ROLE_TOURNAMENT`) which EvEJS's
  `MAX_ACCOUNT_ROLE` (`accountRoleProfiles.js:76-95`) does not include.
- EvEJS splits the account role into three stored parts —
  `session.accountRole`, `session.chatRole`, and the composed
  `session.role` — and a non-GM account is `PLAYER_ACCOUNT_ROLE = 0n`
  (`accountRoleProfiles.js:30`) plus the base mask. eve-offline has one flat
  `role`. If eve-offline ever ports the chat-colour feature
  (`CHAT_ROLE_PROFILES`, `:98-134`) it needs the split.
- **Corporation roles are a completely separate, 55-bit space** and must not be
  confused with session roles. `corporationRuntimeState.js:51-106`. The two
  largest bits, `CORP_ROLE_PROJECT_MANAGER = 1152921504606846976n` (bit 60)
  and the unnamed `4611686018427387904n` (bit 62), overlap numerically with
  session `ROLE_SERVICE` and `ROLE_LOGIN` — different namespaces, same
  integers. `corprole`/`rolesAt*` are BigInt and must marshal as longs.
- The `corprole` derivation is location-dependent:
  `corprole = rolesAtAll | (stationID === hqID ? rolesAtHQ : stationID === baseID ? rolesAtBase : rolesAtOther)`
  (`corporationRuntimeState.js:1802-1808`). eve-offline currently sends
  `rolesAt* = 0` to avoid this (`service.rs:1535-1537`); as soon as a real corp
  exists, the derivation has to be right or role checks fail at exactly one
  station.

**Structure / sov type IDs** (all hardcoded as `const`, none from SDE lookup):

- Upwell presets, 30 type IDs: `structureConstants.js:253-435` —
  35825/35826/35827 (Raitaru/Azbel/Sotiyo), 35832/35833/35834 (Astrahus/Fortizar/Keepstar),
  35835/35836 (Athanor/Tatara), 40340, 47512-47516 (faction Fortizars), 81826 (Metenox).
- Sov: 32226 TCU, 32458 IHub, 34832 command node, 35840 Pharolux, 35841
  Ansiblex, 37534 Tenebrex, 81080 Skyhook, 85230/85980 Mercenary Den
  (`sovConstants.js:9-12`, `sovParityReset.js:35-43`).
- Sov hub upgrades: 81615/81619/81621 (`sovUpgradeSupport.js`).
- Moon: 45009, 46329, 81826, 81920, 82941.
- POS: 11584, 12239, 23953, 27673, 27674, 27897, 4051, 16273.
- Other: `TYPE_CHARACTER = 1373`, `TYPE_CORPORATION = 2`, `TYPE_ALLIANCE = 16159`,
  `TYPE_OFFICE_RENTAL = 26`, `OWNER_TYPE_CHARACTER = 1373`
  (`corporationState.js:40`).
- **Policy**: do not copy any of these as literals. Put them in one table that
  can be validated against eve-offline's 3619879 SDE at startup, and fail loud
  on an unpublished or missing type. A wrong structure typeID anchors
  successfully and then has the wrong size, services and attributes.

**Sov balance constants** — `sovConstants.js:16-32` and `sovState.js:63-70`,
listed in the effort table. These are the ones with no wire-visible symptom.

**ID range allocation**: `CUSTOM_CORPORATION_ID_START` /
`CUSTOM_ALLIANCE_ID_START` (`corporationState.js` exports),
`NEXT_STRUCTURE_ID_START` (`structureState.js:2189`). The client's
`idCheckers` types an owner by its numeric range; pick wrong and
`IsPlayerCorporation` / `IsNPC` flip, which eve-offline already depends on in
two places (`service/character.rs:118,1113`).

**Non-ID hazards**:

- `FILETIME_EPOCH_OFFSET = 116444736000000000n` / `TICKS_PER_MS = 10000n`
  appear independently in `corporationState.js:41-42` and `sovConstants.js:24-25`.
  Affiliation records store filetimes.
- Sov hub position derives from the Destiny sun warp-in point
  (`sovHubPlacement.js:10-13`); if eve-offline's warp-in geometry differs the
  hub is anchored somewhere the client does not expect to warp to.
- `STRUCTURE_STATE`, `STRUCTURE_UPKEEP_STATE`, `STRUCTURE_SERVICE_ID`,
  `POWER_STATE` (`sovModernState.js:35-41`) are all wire-visible enums.
- The 16 sovereignty `.proto` schemas. See the effort table.

## Notes for the coordinator

**The three surprises.**

1. **The Equinox sovereignty UI is not machoNet.** Hub fuel, hub upgrades,
   workforce, colony resources, skyhooks and mercenary dens are protobuf over
   HTTPS through `_secondary/express/` — a second server with 33 gateway
   services, 8 of them in my area. eve-offline has no such transport. This is
   not a detail of the sov port; it *is* most of the sov port, and it also
   gates corp goals, corp colours and corp skill plans. **If sov is on the
   roadmap, schedule the gateway transport as its own item, before sov.**
2. **`refreshAffiliationSessions` has a bug and it is the function the
   coordinator built on.** It never pushes `corprole`/`rolesAt*`/`hqID`/`baseID`
   to the client, because it passes `selectionEvent: false` and does not pass
   `includeRoleChanges: true` (`chatCommands.js:886-890` vs
   `characterState.js:4137`). The retail path
   (`corporationNotifications.js:788-806`) does pass it, but is single-character.
   **eve-offline should implement the set-based version *with* roles — i.e.
   neither of theirs exactly.**
3. **POS lives in the items table.** No starbase table exists; a POS is
   `categoryID === 23` items with a `spaceState.controlTowerID`
   (`starbaseItemIndex.js:25`). That is the retail model and it is the right
   model, but it means POS cannot be ported as an isolated subsystem at all —
   it is a view over inventory plus dogma plus space.

**Count corrections to the brief.** Sovereignty is 40 `.js` files (+1 JSON
bundle), not 42. Starbase is 23 `.js` (+1 JSON), not 24. The structure services
are **sixteen**, not fifteen (`structureCompressionMgr` was in the brief's prose
but `structureSettings` and `structureDirectory` round it out; `structureDirectory`
was missing from the brief's list and is the biggest read surface of the lot).
`fleetMgr`/`fleetProxy`/`fleetObjectHandler` are 3 of 10 files — the brief's
list omitted `fleetRuntime.js`, which is 3,083 of the 5,278 lines and where all
the work is.

**What I did not verify.** No tests exist, nothing was executed, and the
following are *file-exists* evidence only, flagged as such: POS combat and
reinforcement under a real attacker (only GM `/pos damage` is observable); POS
production (their own chat command calls it a diagnostic); wars actually
changing legality in space (the coupling is outside my trees); whether the
`.proto` bundle matches any live client. I also did not read
`corpRegistryRuntime.js` (4,017 lines) or `structureState.js` (4,827 lines) in
full — I sampled their persistence, handler counts and normalisers.

**What to look at next, in priority order.**

1. The identity split, as specified in §"What eve-offline's identity split must
   support". It is M-sized, it is the gate on every other item here, and it is
   worth doing before anything in this inventory is ported.
2. **Fleets.** Best ratio in the whole area: 5,278 lines, zero persistence,
   two lines of space coupling, immediately visible to friends playing
   together, and it needs nothing from corps. If something from this inventory
   ships next, it should be this. The one hard part is the bound-object
   `MachoBindObject`/`GetInitState` handshake
   (`fleetObjectHandlerService.js:138-180`).
3. A **type-ID table with startup validation** against eve-offline's 3619879
   SDE, before porting any structure or sov code. ~80 literals are in play and
   every one of them fails silently.
4. Resolve the **`corprole` contradiction**: EvEJS sends it
   (`clientSession.js:85`, `characterState.js:4164-4168`), eve-offline's docs
   say the client derives it and it must not be sent
   (`service.rs:1516-1521`). A capture settles it; until then assume
   eve-offline's note is right (it was read off the client) and treat EvEJS's
   send as redundant.
5. Corporation core, as the first XL item — but scope it to *one* corp with
   *one* CEO and real roles, not the 92-handler surface. Offices, votes,
   wars and LP can all wait.
6. Defer POS indefinitely and sov until the gateway transport exists.
