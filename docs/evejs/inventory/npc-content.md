# Inventory: NPCs, dungeons, missions and sites (EvEJS)

This is the whole PvE content stack of EvEJS: the NPC entity runtime (spawn,
behave, shoot, die, drop), the dungeon/site instance layer that holds a pocket
of space and advances it through rooms and objectives, the agent/mission layer
that offers and settles missions, the exploration layer that makes sites
findable with probes, and the Abyssal engine, which is a self-contained
re-implementation of filament runs sitting on top of all of it. Measured:
**116 JavaScript files / 90,562 LOC** in the four named trees
(`space/npc` 54 files / 32,610; `services/dungeon` 19 / 26,230;
`services/agent` 16 / 17,533; `services/exploration` 27 / 14,189), plus
**295 files / 128,072 LOC** under `abyssal/` — **~219k LOC total**, roughly a
third of the server. Alongside the code they ship **~110 MB of JSON content
data** as pre-baked static tables (`tools/DatabaseCreator/staticTables/`:
`dungeonAuthority` 71 MB, `missionAuthority` 15 MB, `agentAuthority` 11 MB,
`npcBehaviorProfiles` 6.9 MB, `explorationAuthority` 6.5 MB, `npcProfiles`
5.4 MB, `dungeonClientContent` 4.7 MB, `npcSpawnPools` 0.8 MB,
`npcLootTables` 66 KB). There are **zero tests** in the release; the
`__testing` exports in `npcBehaviorLoop.js:5782` and friends are hooks for a
test suite that is not shipped. Every status below is inferred from code shape,
config gates and the authors' own comments, and is flagged accordingly.

## Subsystems

| subsystem | their files (count + key names) | approx LOC | status | evidence |
|---|---|---|---|---|
| NPC behaviour engine (target/move/shoot tick) | 1 + support: `npcBehaviorLoop.js`, `npcRegistry.js`, `npcRuntime.js`, `npcControlState.js` | 7,188 | functional | Complete decomposition present and wired: `npcBehaviorLoop.js:5591` `tickScene` → `:5061` `tickController` → `:5086` `tickControllerBehavior`, which calls movement, propulsion, weapons, hostile/assist/self modules in order (`:5323`–`:5556`). Comments cite reproduction against live client logs (`npcRuntime.js:27-31` names `client/crime7.txt`, `client/crime9.txt`; `npcBehaviorLoop.js:4082` names `client/jolt9.txt`), which is the strongest evidence of real runtime exercise anywhere in this area. |
| NPC catalogue + spawn selection | `npcData.js`, `npcSelection.js`, `npcHullDamageability.js`, `npcHardwareCatalog.js`, `npcEquipment.js`, `pirateHullProfiles.js`, `pirateFactionPolicy.js`, 6 `*NpcCatalog.js` | ~9,100 | functional | 5,620 profiles over 5,606 distinct `shipTypeID`s, 83 spawn pools, 11 spawn groups measured in `staticTables/npcProfiles/data.json` and `npcSpawnPools/data.json`; weighted pick at `npcSelection.js:78`, plan resolution at `:493`. `npcHullDamageability.js:5-21` documents a real failure mode they found and fixed (41 SDE entity types with no usable HP would make a wave unclearable). |
| NPC loot | `npcLoot.js` + `npcLootTables` table | 886 + 66 KB | **partial** | The rule engine is complete (`npcLoot.js:509` `getRuleCandidatePool`, `:680` `rollRuleLootEntries`, `:842` `seedNpcShipLoot`), but **33 of 61 loot tables are explicitly empty placeholders** and **4,027 of 5,620 profiles (72%) point at one**, e.g. `parity_irregular_entities_pending_loot`: `"Explicitly empty placeholder … until loot parity is modeled separately"`. Belt rats, deadspace pirates and haulers drop; mission, overseer, homefront and abyssal NPCs drop nothing. |
| NPC wrecks + bounties | `nativeNpcWreckService.js`, `npcPresentation.js`, `pendingNpcBounties` table | 1,417 | functional | `npcPresentation.js:198` and `:243` read SDE attribute 481 `entityKillBounty` with the authored profile bounty taking priority. |
| Capital NPCs | `capitals/` 10 files | 1,714 | partial | Full sub-engine (`capitalNpcTargeting.js`, `capitalNpcMovement.js`, `capitalNpcDoctrine.js`, `capitalNpcEngagement.js`) dispatched from `npcBehaviorLoop.js:3254`; driven by a 346 KB `capitalNpcAuthority` static table. No gate in `config/npc.json` beyond `beltRats.asteroidBeltNpcRatCapitalEnabled`. |
| Drifter / Triglavian NPCs | `trigDrifter/` 2 files, plus ~1,200 lines inside `npcBehaviorLoop.js` | ~3,500 | functional | The most elaborate AI path: scored targeting (`npcBehaviorLoop.js:2428` `scoreDrifterTarget`), pack regroup, pursuit warp, reinforcement calls (`:808`), turbo-shield state machine (`:614`), superweapon cycle (`:540`). 19 drifter site families in `dungeonAuthority/data.json` `meta.universeSpawnFamilies`. |
| Belt rats | `beltRatRuntime.js` | 1,412 | functional | Enabled by default, all four flags true in `config/npc.json`. |
| CONCORD / empire security | `npcDefaultConcordRules.js`, `empireSecurity/empireSecurityNpcCatalog.js`, `npcSentryPolicy.js` | 760 | partial | `config/npc.json` has `npcDefaultConcordStartupEnabled: true` but `npcDefaultConcordGateAutoAggroNpcsEnabled: false`, `npcDefaultConcordStationScreensEnabled: false`, `crimewatchConcordPodKillEnabled: false` — three sub-features shipped off. |
| NPC dormancy / scale control | `npcCombatDormancy.js`, `npcAmbientMaterialization.js`, `npcAnchorRelevance.js`, `legacySyntheticNpcCleanup.js` | 1,306 | functional | Materialise/dematerialise by player relevance; `npcBehaviorLoop.js:146-147` caps the tick at 3 controllers and 2 ms per scene beat. |
| Dungeon instance runtime | `dungeonRuntime.js`, `dungeonRuntimeState.js`, `dungeonTrackingRuntime.js`, `dungeonVisibilityPolicy.js`, `deadspaceWarpPolicy.js` | 4,916 | functional | `dungeonRuntime.js:2178` exports `createInstance`/`activateRoom`/`advanceObjective`/`completeRoom` plus persistence and per-session visibility. |
| Dungeon content authority | `dungeonAuthority.js`, `dungeonAuthorityPayload.js`, `dungeonSiteAdapter.js` | 991 | functional **but thin content** | 5,009 templates indexed (`dungeonAuthority/data.json` `counts.templateCount`), of which **4,613 from the client and 396 from the EVE University wiki** (`indexes.templateIDsBySource`). Only **26** carry real room geometry — see Data provenance. |
| Universe site spawning | `dungeonUniverseSiteService.js`, `dungeonUniverseRuntime.js`, `dungeonSiteSpawnPolicy.js`, `globalSiteSchedulerRuntime.js`, `homefrontPopulationPolicy.js` | 17,869 | functional | Largest single file in the area (10,331 LOC). Policy v3 at `dungeonSiteSpawnPolicy.js:19`; per-family site counts are operator config (`config/world.json` `cosmicSites.universeSiteCountsByFamily`). |
| Dungeon client protocol | `keeperService.js`, `dungeonService.js`, `dungeonInstanceCacheMgrService.js`, `dungeonExplorationMgrService.js` | 2,180 | partial | `keeperService.js:1015` `GetCurrentDungeonForCharacter`, `:1020` `ActivateAccelerationGate`, `:1031`/`:1042` operation spawnpoints. `:1025` `GetLevelEditor` is an explicit refusal stub (`"level editor runtime is not enabled"`). `dungeonService.js:172-210` serves the in-client dungeon *editor* tables (`DEGetDungeons`, `DEGetRooms`, `DEGetRoomObjectPaletteData`). |
| Agent + mission runtime | `agentMissionRuntime.js` (8,458), `agentMgrService.js`, `missionAuthority.js`, `missionRuntimeState.js`, `agentAuthority.js` | 12,818 | **partial — 13 missions live** | 2,878 mission records shipped, 10,941 agents. But `isMissionOfferAllowedForAgent` (`missionAuthority.js:758-779`) refuses any mission from an ordinary security agent that is not in `goldenSecurityMissions`, and that list in `server/src/config/productionMissionPolicy.json:3` has **13 entries** plus 1 chain. Everything else is data on disk, not offerable content. |
| Mission reward settlement | `missionRewardSettlementState.js`, `*Jobs.js`, `*Recovery.js` | 2,216 | functional | Durable job + reconciliation/recovery lane; this is a lot of machinery for crash-safe payouts. |
| Research agents | `researchRuntime.js`, `researchAuthority.js`, `researchDialogue.js`, `researchRuntimeState.js` | 1,836 | functional | 651 research agents in `agentAuthority/data.json` `counts`. `researchDialogue.js:157` says `"Verified against a live client"`. |
| Epic arcs / storyline | `epicArcStatusService.js`, `storylineAgentSelector.js` | 522 | partial | 267 epic-arc mission records with `epicArcID`/`nextMissionIDs` chains measured in `missionAuthority/data.json`; the selector is only ~258 LOC, so arc *progression* is thin relative to the data. |
| Probe scanning | `scanMgrService.js`, `probes/` 5 files | 6,489 | functional | Full `scanMgr` surface (`scanMgrService.js:2325`–`:2895`): register, full state, probe destination, range step, cone scan, `RequestScans`, recover, destroy, plus QA overrides. |
| Signatures / anomalies | `signatures/` 10 files | 1,509 | functional | Provider pattern over scene sites, anomalies, wormholes, structures; placement at `signaturePlacement.js:138`. |
| Wormholes | `wormholes/` 8 files | 5,049 | functional | 8,490 systems, 99 hole code types, 3,015 static slots, 42 wandering profiles (`explorationWormholeStatic/data.json` `counts`). Enabled in `config/world.json`. |
| Hacking minigame | `hackingMgrService.js`, `depotHackingBoard.js` | 1,009 | functional | Complete handler set (`hackingMgrService.js:700`–`:874`): start, click tile, use utility, quit. |
| Abyssal deadspace | 295 files under `abyssal/` + `space/npc/abyssal/` | 128,072 | partial | All 35 filaments defined (7 tiers × 5 weathers) in `catalog/content/abyssalFilamentDefinitions.js`. But room *geometry* exists only for tier-0/tier-1 captures (10 captures / 30 rooms in `retailAuthoredContent.generated.json` `expectedCounts`), abyssal NPCs have an empty loot table (`parity_abyssal_entities_pending_loot`), and they self-declare `NOT_IMPLEMENTED_IN_EVEJS` statuses (`abyssal/runtime/abyssalVisibilityRestrictions.js:61,184`) and "localized cloud mechanics are not implemented" (`abyssal/catalog/abyssalEnvironmentalCatalog.js:279`). |

## Data provenance

EvEJS uses **five** distinct provenance routes, and — usefully for us — they
tag them. `abyssal/catalog/abyssalContentAuthority.js:3-9` defines the exact
taxonomy this section needs:

```
CAPTURED_RETAIL  OBSERVED_EMPIRICAL  PROJECT_AUTHORED  PROVISIONAL  DEBUG_PLACEHOLDER
```
plus a separate validation authority `SDE_RUNTIME_AUDITED` (`:24-26`). **I
recommend we adopt this taxonomy verbatim** — it is the single most
transferable idea in this area and it is pure design, not data.

| data kind | where theirs comes from | evidence | can we use it? |
|---|---|---|---|
| NPC combat stats (HP, resists, damage, ranges) | **(a) CCP's published SDE** — downloaded as JSONL from developers.eveonline.com | `tools/DatabaseCreator/database-creator.js:34-35`: `DEFAULT_SDE_URL = "https://developers.eveonline.com/static-data/tranquility/eve-online-static-data-3396210-jsonl.zip"`. `npcHullDamageability.js:5-6`: *"NPC hit points come from the ship type's dogma and from nowhere else: no NPC profile carries a health override."* | **Code: yes. Data: no, and we don't need it.** We read the same numbers out of `types.fsdbinary`/`typedogma.fsdbinary` ourselves, for the right build. Their route is *easier* than ours (a public JSONL download) but pinned to 3396210. |
| NPC weapons / modules | **(a) SDE dogma**, via effects, not authored loadouts | `npcData.js:123-130` defines `retail_empty_npc_entity_loadout` — *"combat stats and effects live on the NPC type dogma record"* — and **5,592 of 5,620 profiles use it**. `npcSelection.js:511-520` explains weapons resolve from the `missileLaunchingForEntity` effect and the type's `entityMissileTypeID`. | **Code: yes, and this is the right design.** Data: ours already. |
| NPC AI tuning (target switching, chase) | **(a) SDE dogma, field-renamed** | `npcBehaviorProfiles/data.json`: 5,639 rows whose fields are CCP's `AI_*`/`entityChase*` attributes under local names — `allowTargetSwitching` (←`AI_ShouldUseTargetSwitching`), `targetSwitchChanceToNotSwitch` (←`AI_ChanceToNotTargetSwitch`), `chaseMaxVelocityMetersPerSecond`, `chaseMaxDistanceMeters`, `chaseMaxDelayMs`, `chaseMaxDelayChance`, `chaseMaxDurationMs`, `chaseMaxDurationChance`, `useSdeChaseVelocity`. Row descriptions say *"SDE-derived asteroid Angel Raider behavior using type 10016 entity range attributes."* Note they never reference the attribute names in code (`grep AI_Should` across `server/` returns nothing) — the mapping is baked into the unshipped generator. | **Code: yes. Data: no — but we have the source.** We hold these attributes directly. **Their field→attribute mapping is a guess I made from the names and must be verified against our `typedogma` dump before relying on it.** |
| NPC profile wrapper (bounty, faction, spawn distance, target mode) | **(c) invented**, keyed to SDE type IDs; bounty falls back to SDE | `npcProfiles/data.json` rows carry `bounty: 15000`, `spawnDistanceMeters: 8000`, `preferredTargetMode: "invoker"`, `lootTableID` — none of which exist in CCP's data. `npcPresentation.js:243` falls back to SDE attribute 481 `entityKillBounty` when the profile has none. | **Code: yes. Data: no — it's their design, and the SDE already gives us bounty properly.** Use attribute 481 directly rather than copying authored numbers. |
| **NPC loot** | **(c) wholly invented — a generative rule engine over the SDE type table** | `npcLoot.js:18-137` is hand-written sets of *group names*: `RULE_AMMO_GROUP_NAMES` (`"projectile ammo"`, `"frequency crystal"`…), `RULE_MODULE_GROUP_NAMES`, `RULE_UTILITY_GROUP_NAMES`, `RULE_TRASH_GROUP_NAMES`, plus `SPECIAL_GRADE_NAME_PATTERNS`. A table is `{mode:"rule", selectors:[{kind,tier,sizeBand,weight,quantityMin,quantityMax}], emptyChance}` (`npcLootTables/data.json`, 60 of 61 tables `mode:"rule"`). Nothing reads a CCP loot table, because there isn't one. | **Code: YES — take it.** This independently confirms our confirmed negative in `docs/re/dungeon-model.md` §6, and it is a working answer to it. Data: n/a (there is no CCP data here to avoid). Caveat: 72% of their profiles are wired to empty placeholders, so the *engine* is portable but the *coverage* is not there. |
| Mission definitions (text, rewards, objectives) | **(b) compiled out of an EVE client** | `missionAuthority/data.json` `source` names `res:/staticdata/missions.fsdbinary` (hash `0a4a7688…`, 1,248,072 bytes), `res:/staticdata/epicarcs.fsdbinary`, with `hashedPath` entries into the client ResFiles cache; produced by `"provider": "Repo DataSync"`. 2,878 missions with 17,164 localised messages. | **Code: yes. Data: ABSOLUTELY NOT.** This is CCP's prose verbatim, shipped as JSON. We must keep doing what `mission.rs:12-18` already does: send `messageID` integers and let the client render CCP's words. Their counts (2,878 / 1,456 distinct kill dungeons) are a useful cross-check against our 2,892. |
| Mission → agent assignment (who offers what) | **(c) invented** | `database-creator.js:1502-1503`: *"Mission template pools are local EvEJS authority data and are not present in public SDE JSONL."* Then gated down to 13: `server/src/config/productionMissionPolicy.json:3` `goldenSecurityMissions`, enforced at `missionAuthority.js:767-775`. | **Code: yes (the gate pattern is good practice). Data: no — their 13 hand-mapped `{missionID, dungeonID, agentLevel}` triples are their curation, and we can derive the link ourselves: `missionAuthority/data.json` shows `killMission.dungeonID` present on 1,656 missions, consumed at `missionAuthority.js:807`.** |
| Agents | **(a) SDE** `npcCharacters.jsonl`, plus a client patch | `agentAuthority/data.json` `source.npcCharacters = "data/eve-online-static-data-3294658-jsonl/npcCharacters.jsonl"`, plus `careerAgentPatch` from `"tools/ClientSDE build-3396210 npccharacters.jsonl"` adding 62 rows. 10,941 agents. | Code yes; data no (ours to extract). Note the mixed-build seam they admit to: `publicSdeLimitations` says *"seeded from an older npcCharacters snapshot plus local patches; public build counts can differ."* |
| Dungeon headers (5,009 templates) | **(b) client FSD**, decoded by a PowerShell + Python pipeline that is **not shipped** | `dungeonAuthority/data.json` `source`: `exportScript: "scripts/DataSync/Export-ClientDungeonData.ps1"`, `staticFsdDecoder: "scripts/DataSync/decode-static-fsd.py"`. **Neither path exists in the release** (`ls scripts` → only `Diagnostics`). Fields present: `dungeonNameID`, `archetypeID`, `difficulty`, `factionID`, `entryObjectTypeID`, `connections[].fromObjectID`. | Code: n/a (not shipped). **Data: no.** Their decoder would have been the valuable part and it is absent. |
| **Dungeon rooms and 3D object layouts** | **mostly MISSING.** 4,983 of 5,009 templates have **no rooms and no positions**; 26 do, from a *third-party* SDE | `dungeonAuthority/data.json` contains no `"rooms"`, `"dungeonRoomID"` or `"dunObjectID"` key at all (verified by grep); its 23,672 `"position"` hits are all 2-D node-graph *editor* coordinates. Its one room per template is synthetic: `siteSceneProfile.roomProfiles[0] = {roomKey:"room:entry", source:"derived_entry"}`. Real geometry lives only in the separate 4.7 MB `dungeonClientContent/data.json`: `"provider": "ElysianSDE", "clientBuild": 3475087`, sourcing a **55 MB `dungeons.jsonl`**, with `templateCount: 26`, `roomCount: 103`, `sourceObjectCount: 4097`, `guardCommandCount: 496`, `entityAnchorCount: 146`, `triggerCount: 131`, and per-template `provenance: {kind:"client_static", geometry:"exact", sourceFields:"lossless"}`. The manifest wanted 100 (`targetManifest.targetCount: 100`). | **Data: no. But this is the headline for the coordinator: we are AHEAD of them here.** We have 8,483 rooms / 260,242 objects / 30,610 spawn entities decoded (`docs/re/dungeon-model.md` §1); they have 103 rooms. Their 26 records do, however, **independently confirm our field names** — their `guardCommand.sourceFields` are exactly `aggressionRange`, `orbitRange`, `entityDelaySpawn`, `entitySpawnOnce`, `entityMin/MaxRespawnDelay`, `objectMin/MaxRespawnDelay`, `objectTriggerSpawn`, `objectDelaySpawn`. Treat that as a free **[V]** on `dungeon-model.md` §5. |
| Dungeon wave composition (how many rats, which, when) | **(c) invented** | `dungeonAuthority` templates carry `populationHints.encounters = [{key:"combat_wave_1", amount:7, spawnQuery:"blood_deadspace_pve", trigger:"on_load"}, …wave_4 amount 10, trigger:"wave_cleared"]` with `"source": "combat_wave_derived"`. `pirateHullProfiles.js:5-11` is explicit: *"Content packs built from the client extraction know which pirate faction a site belongs to and, by reconstruction, which hull classes a wave carries, but they never assert exact ship types. The engine owns that decision."* And **nothing in the server reads `guardCommand` or `entityAnchors`** (grep across all of `server/src` returns nothing) — CCP's own spawn parameters are shipped and then ignored. | **Code: yes, as a fallback. Data: no.** We don't need the fallback for the 5,516 dungeons we decoded: we have the real spawn entities. This is the clearest case where their *design* is a workaround for data we already hold. |
| Scenery object identity in dungeons | **(c) invented, by name guessing** | `populationHints.environmentProps[].typeNameCandidates: ["Blood Bunker","Blood Raider Bunker","Bloodraider Bunker","Fortified Angel Bunker",…]` and `gateProfiles[].typeNameCandidates: ["Acceleration Gate"]`. They search the type table by name because they don't have the object's `typeID`. | **Neither.** We have exact `typeID` per object. Do not import this pattern. |
| Site spawn placement (which systems get which sites) | **(e) reconstructed from public documentation and community wikis** | `dungeonSiteSpawnPolicy.js:22-60` `SOURCE_REFS` cites CCP support articles (Sovereignty Hub, Combat Anomalies, Activity Defense Multiplier) and EVE University wiki pages (Wormhole sites, Cosmic Anomalies, Asteroids and Ore). `dungeonAuthority/data.json` `spawnProfiles.source = "repo-authored"`, with the note *"HOW MANY sites each eligible system receives is operator configuration in config/world.json … not authored here."* 396 templates come wholly from `"eve-university"`. | **Code: yes. Data: their reconstruction, so it is their design, not CCP's truth — and the wiki text itself is third-party.** Cite-as-reconstruction if we adopt the numbers. |
| **Abyssal loot** | **(d′) harvested third-party run telemetry** — not ESI, not SDE, not the client | `abyssal/catalog/abyssalLootTables.generated.js:1-3`: `// GENERATED FILE … Regenerate with: node tools/AbyssalData/scripts/analyze-abyssal-loot.js --input abyssal-runs.csv`, `"sourcePolicy": "non-empty high-confidence rows pooled by tier"`. Each tier file: `"lootTableID": "abyssal-space:empirical:tier-1:high-confidence-v1"`, `"sourceEligibleRuns": 839`, entries with `dropChanceBps` and empirical `quantityBuckets`. `catalog/content/abyssalRewardTables.generated.json` names `"historical Abyssal.Space complete-run loot telemetry"` with per-file SHA-256s. **`tools/AbyssalData/` is not in the release.** | **Code: yes (the bps + quantity-bucket shape is a good model). Data: no — it is a third-party dataset of unknown licence, scraped from someone's run tracker.** This is a *sixth* category the brief didn't name and the coordinator should add to the vocabulary: *empirically observed from live Tranquility play*. |
| Abyssal spawn tables | **(d′) scraped community pages + SDE validation** | `catalog/content/qsnaAbyssalSpawnTables.generated.json`: `source.upstream = "QSNA pages derived from Abyssal.Space telemetry"`, `source.directory = "tools/AbyssalData/scraper/output"`, source CSVs `npc-by-combination.csv`, `valid-spawn-tables.csv`. Its `authority` block is exemplary: `familyEligibility/npcAppearanceChance/npcQuantityBounds/familyEnemyCountBounds: "observed_empirical"`, `typeIdentity: "sde_runtime_audited"`, `familySelectionPolicy/quantitySelectionPolicy/countReconciliationPolicy: "project_authored"`. | **Code: yes. Data: no.** The per-field authority tagging is the pattern to copy. |
| Abyssal room geometry | **(f) captured from a live client session on Tranquility** | `catalog/content/retailAuthoredContent.generated.json` `generatedBy`: `sourceArchiveName: "authoredContent.zip"` + SHA-256, `activeGenerationID: "tq-3436472-v1-…"`, `clientBuild: 3436472`, `expectedCounts: {captures: 10, rooms: 30}`. `catalog/content/baseRoomGeometryFactsV1.json` rows carry `capturedRetailOwnerID: 500021`, `retailBallID`, `dunObjectID`, `exactRelativePosition`, `sourceFile: "TQCalmAbysaal2.txt"`, `sourceEvidence.positionEvidence: "slim_dunPosition"` — i.e. ball positions read out of a live TQ session's traffic. | **Data: NO — this is live-server capture of CCP content and the most legally exposed thing in the tree.** Code yes. Note `environmentFactsV1.json` `source` is admirably honest: *"tier and room assignments deliberately excluded. Runtime selection and placement are EveJS policy, not measured retail probabilities."* |
| Probe/scan contracts | **(b) decompiled client Python** | `explorationAuthority/data.json` `source.clientContracts = ["_local/codeccpFULL2/code/probescanning/const.py", "_local/codeccpFULL2/code/probescanning/formations.py"]`. The extractor **is** shipped: `tools/ClientCodeGrabber/extract.py:3-5` — *"code.ccp (ZIP) → .pyj (zlib) → .pyc (Python 2.7) → .py (decompiled)"* via uncompyle6. `_local/` itself contains only `certs`. | **Code: yes. Data: no.** The extractor is a useful cross-check for our own client RE but targets Python 2.7 bytecode in `code.ccp`; whether build 3619879 still ships that layout is **unverified**. |
| Wormhole statics | **(c) invented / community spec** | `explorationWormholeStatic/data.json` `source.staticSpec = "local:scripts\\DataSync\\spec\\explorationWormholeStatics.json"` (not shipped), `wanderingSpec` likewise. 8,490 systems, 3,015 static slots. | Code yes; data no — and provenance is unverifiable since the spec files are absent. |
| NPC standings | **(a) SDE** | `npcStandingsAuthority/data.json` `source.authority = "eve-online-static-data-3294658-jsonl"`, tables `npcCorporations.jsonl`, `factions.jsonl`. | Code yes; data ours to extract. |

**Nothing in this area comes from ESI.** I grepped for ESI/everef/fuzzwork across
the DatabaseCreator and found no hits. The "harvested" route here is *run
telemetry scraped from community sites*, which is a different and legally
murkier thing than ESI.

## The AI engine

`npcSelection.js` is **not** a target selector — it is spawn-composition
selection (which profiles make up a wave). Targeting lives entirely in
`npcBehaviorLoop.js`. The decomposition, in the order the tick runs it:

**What drives the tick.** `tickScene(scene, now)` (`:5591`) walks a cached,
`entityID`-sorted controller list for the scene (`npcRegistry.js:6-11`
documents why it's cached: the loop runs 10 Hz per scene). It is a
**round-robin with a cursor and a double budget**: stop after
`NPC_BEHAVIOR_MAX_CONTROLLERS_PER_TICK = 3` or `NPC_BEHAVIOR_TICK_BUDGET_MS = 2`
(`:146-147`), resume at the controller that was *not* ticked — there is a
comment at `:5619-5621` explaining that advancing past it would systematically
starve 1-in-(budget+1) due controllers. Each controller carries its own
`nextThinkAtMs`; `scheduleNextThink` (`:4899`) sets it to
`now + behaviorProfile.thinkIntervalMs` (default 250 ms, floor 50 ms, and 160 ms
on the SDE-derived retail profiles), with a `forcedAtMs` clamp downward and a
`notBeforeMs` clamp upward. `tickControllersByEntityID` (`:5648`) is the
event-driven path — something happened to these NPCs, think now.
`runtimeKind === "nativeAmbient"` controllers are skipped entirely (`:5629`):
ambient decoration never thinks.

**What decides targeting.** A four-layer funnel.
1. `resolveDesiredTarget` (`:3369`) — manual-order override first
   (`attack`/`orbit`/`follow`/`stop`/`returnHome`/`holdFire` from GM or scripted
   commands), else fall through.
2. `resolveBehaviorTarget` (`:3142`) — branches on
   `behaviorProfile.autoAggro`, then dispatches to a specialist
   (`resolveCapitalBehaviorTarget` for `entity.capitalNpc`,
   `resolveDrifterBehaviorTarget` at `:2578` for drifters) or runs the generic
   path.
3. `resolveCurrentBehaviorTargetGate` (`:2285`) — **target stickiness**. If the
   current target is still valid, keep it unless a switch is both *due* and
   *rolled*: `targetSwitchIntervalMs` (default 60 s, `:134`) and
   `targetSwitchChanceToNotSwitch`. `resolveNpcTargetSwitchPolicy` (`:1970`)
   deliberately distinguishes *absent* from *false* via `hasOwnField` (`:1962`):
   a profile with no `allowTargetSwitching` key returns `handled: false` and the
   gate is bypassed, which is **not** the same as switching disabled. This is
   where CCP's `AI_ShouldUseTargetSwitching` / `AI_ChanceToNotTargetSwitch`
   land.
4. Priority order once the gate declines to handle it: **retaliation →
   preferred target → nearest valid**. `resolveRetaliationTarget` (`:3101`)
   returns `lastAggressorID` if it is within `NPC_RETALIATION_MEMORY_MS = 60_000`
   (`:148`) and passes `isValidBehaviorCombatTarget`;
   `controller.preferredTargetID` is the scripted/dungeon-assigned victim; then
   `findNearestCombatTarget` (`:1912`) within the acquisition range.

   Validity is layered: `isValidCombatTarget` (`:1856`) composes
   `isFriendlyCombatTarget` (`:1578`), `isEntityInActiveWarp` (`:1833`),
   `isEntityCloakedForCombatTargeting` (`:1841`),
   `isTargetWithinEntityVisibilityScope` (`:1852`), `isCapsuleEntity` (`:1825`,
   gated by `allowPodKill`), `isIgnoredInvulnerablePlayer` (`:1569`), and
   `ignoreDronesBelowSignatureRadius`. Class filtering is separate:
   `resolveCombatActorClass` (`:1517`) returns `player`/`drone`/`npc`/… and
   `resolveAutoAggroTargetClasses` (`:1652`) / `resolveProximityAggroTargetClasses`
   (`:1691`) say which classes this profile will shoot.

   **Range.** `resolveNpcAcquisitionRangeMeters` (`:160`) floors the profile's
   `aggressionRangeMeters` at `NPC_MIN_ACQUISITION_RANGE_METERS = 50_000`
   (`:158`) — so a rat with a 30 km SDE aggression range still acquires at 50 km.
   That is an invented override, not CCP behaviour, and is worth flagging.

   Drifters are the only NPCs with real **threat scoring**:
   `scoreDrifterTarget` (`:2428`) + `selectBestScoredCombatTarget` (`:2562`),
   with weights at `:136-139` (`DRIFTER_GUARD_PRIORITY_BASE_SCORE` 900,
   `..._BONUS_SCORE` 900, `..._ENTOSIS_PRIORITY_SCORE` 1500,
   `DRIFTER_TARGET_SWITCH_SCORE_MARGIN` 250 — a new candidate must beat the
   incumbent by 250 to steal it) and a 60 s aggression memory (`:135`).
   **For everything else, threat is just "who hit me last", single-slot.** There
   is no damage-weighted threat table anywhere in this file.

**What decides movement.** Separate and downstream of targeting.
`resolveMovementDirective` (`:3956`) turns the manual order + profile
`movementMode` into `{movementMode, rangeMeters}` (`orbit` / `follow` / `hold` /
`stop`). `syncNpcMovement` (`:4015`) issues the scene command only when the
intent changed (`logNpcMovementCommand` at `:3998`). Orthogonally:
- `syncNpcChaseVelocity` (`:2164`) + `resolveNpcChaseVelocityPolicy` (`:2044`) +
  `applyNpcChaseVelocityMode` (`:2151`) implement CCP's chase mechanic: cruise
  at `cruiseSpeedMetersPerSecond` until the target is beyond
  `chaseMaxDistanceMeters`, then burst to `chaseMaxVelocityMetersPerSecond`,
  with `chaseMaxDelayMs`/`chaseMaxDelayChance` and
  `chaseMaxDurationMs`/`chaseMaxDurationChance` rolled via `rollChance` (`:2089`).
  `npcChaseSpeedModifiers.js:16` applies fitting modifiers to attribute 37
  `maxVelocity` for the burst ceiling.
- `syncNpcPropulsion` (`:4246`) / `activateNpcSyntheticPropulsion` (`:4174`) fake
  an AB/MWD effect for the client when the hull has no real module
  (`NPC_SYNTHETIC_PROPULSION_DURATION_MS = 60_000`, `:120`).
- Leash: `isBeyondLeash` (`:4856`) against `leashRangeMeters` (90–150 km in the
  data) drops combat; `syncNpcReturnHome` (`:4684`),
  `syncNpcIdleAnchorOrbit` (`:4764`) and `tryNpcIdleAnchorWarp` (`:4805`,
  min 150 km, 15 s cooldown, `:144-145`) take over when idle.
- Abyssal containment is a *pre-pass*, not part of the directive:
  `tickController` (`:5061`) asks `resolveAbyssalNpcBoundaryDirection` first and,
  if the NPC is near the pocket wall, overrides with a raw `scene.gotoDirection`
  and sets `avoidingBoundary`, which suppresses normal movement at `:5335`.

**What fires the guns.** Four independent maintenance lanes, each with its own
cadence and jitter so they don't align: `syncNpcWeapons` (`:4478`,
`NPC_WEAPON_MAINTENANCE_MS = 2_500`), `syncNpcHostileModules` (`:3476`, EWAR,
3_000 ms), `syncNpcAssistanceModules` (`:3648`, remote rep —
`findNearestNpcAssistanceTarget` at `:2979` and
`resolveNpcAssistanceNeedRatio` at `:2825` pick the hurt friend, 2_000 ms), and
`syncNpcSelfModules` (`:3831`, hardeners/boosters, 5_000 ms).
`isNpcMaintenanceDue` (`:238`) + `getNpcMaintenanceJitterMs` (`:225`) +
`getNpcInitialModuleCycleDelayMs` (`:201`,
`NPC_INITIAL_MODULE_CYCLE_SPREAD_MS = 5000`) spread cycles across a wave so 20
rats don't all volley on the same frame. Locking is explicit and asynchronous:
the loop inspects `scene.getTargetsForEntity` and
`scene.getSortedPendingTargetLocks` (`:5386-5387`) and only fires once
`hasDesiredLock` (`:5508`).

**How aggression is tracked.** `noteIncomingAggression(entityID, attackerEntityID, now)`
(`:5742`) is the single entry point, called from the damage path outside this
file. It writes `controller.lastAggressorID` / `lastAggressedAtMs` and wakes the
controller. That is the entire threat model for non-drifters: one slot, 60 s
decay. Drifters additionally propagate aggression to pack members
(`propagateDrifterReinforcementRequestState`, `:788`) and can call in
reinforcements (`maybeRequestDrifterReinforcements`, `:808`).

**How respawn works.** **Not in this file at all** — and this is the gap. The
behaviour loop only unregisters a controller when its entity vanishes
(`:5064-5067`). Respawn is a *site*-level concern:
`dungeonSiteSpawnPolicy.js:1306` `getCompletionRespawnRule` and
`COMPLETION_RESPAWN_RULES`, driven by `globalSiteSchedulerRuntime.js:76`, which
reconciles whole site instances on a durable scheduler. Wave-to-wave spawning
inside a site is `populationHints.encounters[].trigger` (`"on_load"` /
`"wave_cleared"`) evaluated by `dungeonUniverseSiteService.js` (spawn calls at
`:10072`, `:10115`, `:10189`). CCP's per-entity `entityMinRespawnDelay` /
`entityMaxRespawnDelay` / `entitySpawnOnce` are **shipped and unread**.

**For us:** the valuable, legitimately adoptable parts are (1) the
budgeted round-robin + per-controller `nextThinkAtMs` scheduler, (2) the
four-layer target funnel with the explicit absent-vs-false distinction in the
switch gate, (3) the separation of *target* from *movement directive* from
*four independently-cadenced module lanes*, and (4) the jitter/spread discipline.
All four are design, not data. What we should **not** copy: the 50 km
acquisition floor, the single-slot threat model, and ignoring `guardCommand`.

## Dependency order

Inside the area:

1. **NPC catalogue** (`npcData`, profiles/pools/groups) → **NPC spawn
   selection** (`npcSelection`). Selection resolves a query string to profile
   IDs by reading the catalogue (`npcSelection.js:50` `getCachedNpcDefinition`).
2. **NPC spawn selection** → **NPC behaviour engine**. A controller is only
   registered for an entity that was spawned from a definition; `npcRuntime.js`
   `spawnBatchInSystem` → `npcRegistry.registerController`. The behaviour loop
   resolves `behaviorProfileID` off the profile (`resolveEffectiveBehaviorProfile`,
   `npcBehaviorLoop.js:1818`).
3. **NPC hull damageability** → **spawn selection**. `npcHullDamageability.js:21`:
   *"Every path that turns a spawn pool into a chosen profile asks this first."*
   Without it a wave can roll an unkillable rat and the site never clears. Port
   this gate *with* the selector, not after.
4. **NPC behaviour engine** → **NPC loot** → **NPC wrecks**. Loot is rolled at
   death and seeded into a wreck's cargo (`npcLoot.js:842` `seedNpcShipLoot`).
5. **Dungeon content authority** → **dungeon instance runtime** → **universe
   site spawning**. `dungeonAuthorityPayload.js:140` merges `dungeonAuthority` +
   `dungeonClientContent` into one payload; `dungeonRuntime.createInstance`
   needs a template; the universe service decides *where and when* instances
   exist.
6. **Dungeon instance runtime** → **NPC spawning**. The site service calls
   `nativeNpcService.spawnNativeNpcEntityInContext` with an anchor entity
   (`dungeonUniverseSiteService.js:10072`) and resolves wave hull classes via
   `pirateHullProfiles.resolveSpawnEntryProfileQuery` (`:1909`).
7. **Dungeon instance runtime** → **exploration signatures**. Sites become
   scannable through providers that read live instances
   (`signatures/providers/sceneAnomalySiteProvider.js`,
   `sceneSignatureSiteProvider.js`) → **probe scanning** →
   **`keeperService.ActivateAccelerationGate`** to actually enter.
8. **Mission runtime** → **dungeon instance runtime**. A kill mission creates a
   private instance (`agentMissionRuntime.js:2971` `dungeonRuntime.createInstance`,
   with `findInstanceBySiteKey`/`purgeInstance` around it) and grants access
   (`dungeonRuntime.grantCharacterMissionAccess`). Mission completion is
   detected from instance/objective state.
9. **Agent authority** → **mission authority** → **mission runtime**. And
   `productionMissionPolicy.json` gates the whole thing: without a golden entry,
   an ordinary agent offers nothing (`missionAuthority.js:767`).
10. **Everything above** → **Abyssal**. The abyssal engine builds its NPC
    definitions out of `space/npc/npcData` (`abyssal/runtime/content/abyssalEncounterNpcCatalog.js:3-6`),
    its rooms out of the dungeon layer, and its own filament/room/weather
    catalogues on top.

Dependencies **outside** this area — these are the real schedule risk:

- **NPC combat requires their Destiny ball simulation, hard.** `space/npc`
  reaches into `space/destiny` in at least seven places:
  `destiny/commands/npc`, `destiny/commands/npcWarpCompletion`,
  `destiny/delivery/michelleContract` (4 call sites),
  `destiny/delivery/stamps`, `destiny/identity/entityID`,
  `destiny/identity/interactionScope` (5 call sites),
  `destiny/simulation/warpPhase`. The behaviour loop also calls the scene
  directly: `scene.gotoDirection`, `scene.getTargetsForEntity`,
  `scene.getSortedPendingTargetLocks`, `scene.getCurrentSimTimeMs`,
  `scene.getEntityByID`. **There is no NPC without a working ball/Destiny layer
  and target-locking.** This is the single biggest prerequisite and it is
  entirely outside this lane.
- **Weapons and HP require the dogma/fitting engine.**
  `services/fitting/liveFittingState` is the most-required external module in
  `space/npc` (10 call sites) — `getAttributeIDByNames`,
  `getTypeAttributeValue`, `applyPassiveShipAttributeModifiers`. Also
  `space/combat/weaponDogma` (3 sites) and `space/modules/liveModuleAttributes`.
- **Loot and mission completion require inventory.** `npcLoot.js:9-12` imports
  `services/inventory/itemStore` (`grantItemToCharacterLocation`, `ITEM_FLAGS`);
  `itemTypeRegistry` is used in 8 places. Courier and trade missions are
  inventory operations end to end. **Mission completion cannot be ported before
  inventory.**
- **CONCORD response requires crimewatch.** `space/npc` imports
  `services/security/crimewatchState`.
- **Site persistence requires their durable job scheduler and `gameStore`.**
  `globalSiteSchedulerRuntime.js` and `missionRewardSettlementJobs.js` both bind
  a durable scheduler; the NPC/dungeon/mission runtimes all persist to SQLite
  tables (`npcRuntimeState`, `dungeonRuntimeState`, `missionRuntimeState`).
- **Everything client-facing needs the `machoNet` service dispatch.**
  `keeper`, `agentMgr`, `missionTrackerMgr`, `scanMgr`, `hackingMgr`,
  `dungeonInstanceCacheMgr`, `dungeon`, `epicArcStatus`, `wormholeMgr`.

## Effort to port, per subsystem

| subsystem | effort | why | risk |
|---|---|---|---|
| NPC behaviour engine | **XL** | 5,810 LOC in one file with no tests, and ~1,200 of those lines are drifter-specific. The *generic* engine is maybe 2,500 LOC and is the part worth porting; untangling it from the drifter and capital branches is most of the work. Hard dependency on Destiny + locking + fitting. | Their invented 50 km acquisition floor (`:158`) and 60 s switch interval (`:134`) are not CCP values and will make NPCs feel wrong if copied blindly. Their `scene.*` contract may not match ours. Build skew: none here — the engine reads dogma by *name*, which is build-stable. |
| NPC catalogue + spawn selection | **M** | The selector is 623 LOC and simple (weighted pick, pool/profile/group resolution). | **The 5,620 profiles are data we cannot ship.** We must generate our own profile set from the 7,418 category-11 types in our build. Their profile→type mapping is their curation and their "parity_*" naming encodes design decisions we may not want. |
| NPC AI tuning from dogma | **S** | Reading `AI_*` and `entityChase*` off `typedogma` is a lookup table; we already have the attributes. | **My field→attribute mapping above is inferred from names and unverified.** Attribute *IDs* may differ between 3396210 and 3619879; resolve by name as `liveFittingState.getAttributeIDByNames` does, never by hardcoded ID. |
| NPC loot rule engine | **M** | 886 LOC, self-contained apart from inventory. The group-name sets (`npcLoot.js:22-87`) are the real content and are short. | Group *names* are localisation-adjacent and change between builds; `"missile launcher rapid light"` etc. must be re-verified against our `groups.fsdbinary`. Silently wrong if a group was renamed — the selector just returns an empty pool and rats drop nothing, which is exactly the failure mode they already have on 72% of profiles. |
| NPC wrecks + bounties | **S** | Bounty is SDE attribute 481; wrecks are an inventory/entity concern. | Low. |
| Capital NPCs | **L** | 1,714 LOC plus a 346 KB authority table we cannot ship. | Needs capital-specific movement (siege, no-warp) in Destiny first. Defer. |
| Drifter / Triglavian | **XL** | ~3,500 LOC, the most invented content in the tree, and it needs warp-while-in-combat, pack coordination and superweapons. | Almost entirely their design. Lowest fidelity payoff per line. Defer hard. |
| Belt rats | **S–M** | 1,412 LOC, needs only asteroid belts + the behaviour engine. | **Best first NPC target.** Risk is low and visible. |
| CONCORD | **M** | 760 LOC but requires crimewatch and security status. | Three sub-features are shipped *off* in their own config, so their own confidence is low. |
| NPC dormancy / scale | **M** | Needed for a populated universe, not for a demo. | Defer until NPC count is a problem. |
| Dungeon instance runtime | **L** | 4,916 LOC covering lifecycle, persistence, per-session visibility and deadspace warp restriction. Unavoidable: nothing PvE works without instances. | Their instance model assumes one synthetic `room:entry`. **Ours must be multi-room from day one** because our data is — do not inherit their single-room shape. |
| Dungeon content authority | **S (for us)** | We don't port theirs. We write our own loader over our already-decoded 5,516 dungeons / 8,483 rooms / 260,242 objects. | The risk is the *opposite* of theirs: we have the data and no runtime. Their template schema (`siteFamily`/`siteKind`/`archetypeID`/`difficulty`) is a reasonable shape to borrow. |
| Universe site spawning | **L** | 17,869 LOC, but most of that is mining/ore/gas specialisation that belongs to another lane. The scheduler + per-family counts core is maybe 2,000 LOC. | Their *which systems get what* is reconstructed from wikis and CCP support articles — citeable, but it is a reconstruction and will not match Tranquility. |
| Dungeon client protocol (`keeper`) | **S** | Six handlers, one of them a refusal. `ActivateAccelerationGate` is the load-bearing one. | **Build skew is a real risk here**: the `keeper` call signatures for 3619879 are unverified against 3396210. Also `keeperService.js:905` notes acceleration gates are a short *warp* on TQ, not a teleport — easy to get subtly wrong. |
| Agent + mission runtime | **L** | `agentMissionRuntime.js` alone is 8,458 LOC. Offer, accept, decline, quit, expire, modify, objective tracking, standings, rewards. | **Hardest-to-see risk: we would be porting a shell.** Only 13 missions actually work on their side; 2,865 records are inert. Porting the runtime does not get us missions, it gets us the ability to curate missions one at a time, which is what `mission.rs` already does for one. Also: mission text is CCP's and must stay as `messageID` integers. |
| Mission reward settlement | **M** | 2,216 LOC of durable-job plumbing. | Only needed once rewards are real; a non-durable version is fine for co-op. |
| Research agents | **M** | 1,836 LOC, mostly dialogue; `researchDialogue.js:157` says it was verified against a live client. | Self-contained, low risk, low value for a co-op/PvP goal. Defer. |
| Epic arcs | **M** | Data is there (267 missions with chain links); the selector is thin. | Depends on the whole mission runtime first. |
| Probe scanning | **L** | 6,489 LOC and a `scanMgr` protocol surface of ~15 handlers. | **Highest build-skew risk in the area**: their scan contracts come from *decompiled 3396210 client Python* (`probescanning/const.py`). Probe formations, scan strengths and result shapes for 3619879 are unverified. A wrong constant here fails silently as "nothing ever resolves". |
| Signatures / anomalies | **M** | 1,509 LOC, clean provider pattern worth copying. | Needs the dungeon instance layer underneath. |
| Wormholes | **L** | 5,049 LOC + 8,490-system static data whose source spec is not shipped. | We'd have to author our own statics. Their provenance is unverifiable. |
| Hacking minigame | **M** | 1,009 LOC, self-contained, fun, visible. | Board generation is theirs (invented); the client protocol is the risk. |
| Abyssal | **XL, do not attempt** | 128k LOC, and its content is third-party run telemetry + live-TQ packet captures we cannot ship. | **The data problem is disqualifying, not just the size.** Even the code is tightly coupled to their generated catalogues. |

## Notes for the coordinator

**The one thing to take away.** On NPC *stats* and *AI tuning* they are ahead of
us (they just read the SDE); on dungeon *geometry* **we are far ahead of them**.
They ship 5,009 dungeon templates of which 4,983 have no rooms and no object
positions, and invented 4-wave encounters to fill the hole
(`populationHints.source: "combat_wave_derived"`). We have 8,483 rooms, 260,242
positioned objects and 30,610 spawn entities with per-entity guard parameters.
Their `pirateHullProfiles.js:5-11` says in plain words that their content packs
"never assert exact ship types" — we can. **So the port is asymmetric: take
their engine, keep our data.** Do not let anyone import their dungeon templates.

**Free verification of our own work.** Their 26 `dungeonClientContent`
templates, sourced from a third-party "ElysianSDE" decode of client build
3475087, contain `guardCommand.sourceFields` with *exactly* the field names in
`docs/re/dungeon-model.md` §5 — `aggressionRange`, `orbitRange`,
`entityDelaySpawn`, `entitySpawnOnce`, `entity{Min,Max}RespawnDelay`,
`object{Min,Max}RespawnDelay`, `objectTriggerSpawn`, `objectDelaySpawn` — plus
exact `dunObjectID`, `roomID`, `position` and `dunRotation`. That is an
independent, third-party confirmation of our struct at a *different* build.
Someone should upgrade the relevant **[I]** claims in `dungeon-model.md` §5 to
**[V]** and cite it.

**Loot: our confirmed negative is now double-confirmed, and they have the
answer.** They built a generative rule engine over the SDE type table
(`npcLoot.js`) precisely because there is no CCP loot table to read. That is the
strongest single endorsement of `dungeon-model.md` §6's conclusion. Take the
engine. But know the honest state of their coverage: **4,027 of 5,620 profiles
(72%) point at an explicitly empty placeholder table**, 33 of 61 tables are
empty-by-design, and mission/overseer/homefront/abyssal rats drop nothing at
all. Their loot works for belt rats and deadspace pirates and nowhere else.

**"13 missions" is the number to quote, not "2,878".**
`server/src/config/productionMissionPolicy.json` enables 13
`goldenSecurityMissions` plus one chain, enforced at `missionAuthority.js:767`.
Everything else is inert JSON. We have 1. The gap on *mission data* is zero —
we both have CCP's table, they just shipped theirs as JSON and we read ours from
the client. The gap on *mission runtime* is 12,818 LOC. Judge the port on the
runtime, not on the content count.

**A sixth provenance category the brief didn't anticipate.** Alongside
SDE / client / invented / ESI, EvEJS uses two more that matter legally and
epistemically: **(e) reconstructed from public documentation and community
wikis** (EVE University supplies 396 whole dungeon templates and the
sites-per-system numbers in `dungeonSiteSpawnPolicy.js`'s `SOURCE_REFS`), and
**(f) harvested from live Tranquility play** — community run-tracker telemetry
(`abyssal-runs.csv`, 839 runs; "Abyssal.Space telemetry"; scraped "QSNA" pages)
and actual in-session ball captures (`authoredContent.zip`, client build
3436472, `capturedRetailOwnerID`, `retailBallID`, `sourceFile:
"TQCalmAbysaal2.txt"`). Category (f) is the most legally exposed material in
their tree and we should not touch any of it. **ESI is not used anywhere in this
area** — I grepped and found nothing.

**Adopt their provenance taxonomy.** `abyssal/catalog/abyssalContentAuthority.js:3-34`
defines `captured_retail` / `observed_empirical` / `project_authored` /
`provisional` / `debug_placeholder`, a validation authority
`sde_runtime_audited`, and a five-level completeness ladder
(`INCOMPLETE` → `PLAYABLE` → `BASELINE_COMPLETE` → `OBSERVED_COMPLETE` →
`CAPTURED_RETAIL_COMPLETE`). `qsnaAbyssalSpawnTables.generated.json`'s
`authority` block tags provenance **per field**. This is better discipline than
most of the rest of their codebase and it is pure design — we can copy it
outright, and it maps cleanly onto our existing **[V]**/**[I]** convention.

**Surprises.**
- `space/npc/npcSelection.js` is not what the brief assumed. It selects *which
  NPCs to spawn*, not which target to shoot. Targeting is 100% inside
  `npcBehaviorLoop.js`. Re-scope any plan written against the brief's wording.
- **Nothing in the entire server reads `guardCommand` or `entityAnchors`.** They
  ship CCP's per-entity spawn and respawn parameters in
  `dungeonClientContent/data.json` and never consult them. Their respawn is
  site-level only (`dungeonSiteSpawnPolicy.js:1306`). We should not inherit that
  blind spot — our 30,610 spawn entities *are* the spawn rule.
- There is no threat table. Non-drifter NPCs track exactly one aggressor with a
  60 s decay (`npcBehaviorLoop.js:5742`, `:148`). If we want aggro that feels
  like EVE we will have to invent more than they did.
- Their mission export for build 3396210 shows `nodeGraphID` populated on only
  123 of 2,878 missions, while `killMission.dungeonID` is on 1,656. Both routes
  exist; the dungeonID one is the usable one. Worth confirming the same split
  holds in 3619879.
- The high-value extraction tooling is **absent from the release**:
  `scripts/DataSync/` (the FSD decoder and the PowerShell dungeon exporter),
  `tools/AbyssalData/` (the loot analyser and the scraper),
  `tools/SignalAtlas/`. Only `tools/ClientCodeGrabber/` ships
  (`code.ccp` → decompiled Python 2.7). Do not plan on reusing their decoders.
- `tools/ClientCodeGrabber/extract.py` is worth 30 minutes of someone's time
  purely as a cross-check on our own client RE — decompiled `probescanning`
  constants are how they got their scan contracts, and we may be able to get the
  same for build 3619879.

**What to look at next, in priority order.**
1. `space/destiny/` — not in this lane, but **nothing here works without it**.
   Whoever owns that lane should know that NPC combat is their most demanding
   consumer (locking, `michelleContract`, `warpPhase`, `gotoDirection`).
2. `npcBehaviorLoop.js:5086-5591` (`tickControllerBehavior`) read end to end —
   it is 500 lines and it is the whole engine's control flow.
3. `services/fitting/liveFittingState` — the dogma/fitting engine, required by
   every weapon and every HP number in this area.
4. Verify the behaviour-profile field → `AI_*`/`entityChase*` dogma attribute
   mapping against our own `typedogma` dump. I inferred it from field names and
   it is the load-bearing assumption of the whole "we already have the AI data"
   claim.
5. `dungeonUniverseSiteService.js` — 10,331 LOC, by far the largest file, and I
   only characterised it. Someone should establish whether its core is ~2,000
   portable lines or 10,000 entangled ones.
