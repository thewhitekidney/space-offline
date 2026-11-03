# What EvEJS implements that we do not

A **scope map**, not a design document. Established 2026-10-20 from the
archive's *file listing alone* — `unzip -l`, nothing extracted, nothing read,
nothing executed. The operating constraints of
`docs/archive/evejs-261018-reference.md` §0 apply here in full: this file names
subsystems, never code. Where a row looks interesting, the next step is the
**client's** bytecode, not their source.

> **Re-measured 2026-10-21** against the tree at `139c0ad`, by the same recipe
> (see the Appendix, and its new §A.2 on where the recipe lies). Their side is
> byte-identical — same archive, same listing — so every number that moved,
> moved on our side. **Three subsystem lanes and an industry round landed in
> between**, and the rows they changed are in §6. The structure, the §1
> framing and the §3/§4 findings are the original document's; the "our state"
> column and the counts are this pass's.

Why this exists: the reference doc answers "does their reading of the protocol
agree with ours". It never asked **"how much game did they build"**. That is a
different and more useful question, because the answer is a roadmap of things
the real client already has UI for.

Contents: 1 the headline, 1a what the denominator means, 2 service-surface
diff, 3 the subsystem inventory, 4 the three rows that matter now, 5 what this
does not tell us, 6 what moved since 2026-10-20.

---

## 1. The headline

```
their machoNet service entry points   206      (from *Service.js basenames)
ours                                   87      (service/*.rs SERVICES, unioned)
in both                                73
theirs, absent from ours              133
ours, absent from theirs               14      (mostly naming variants:
                                                stationSvc/station, LPSvc/lp,
                                                dogmaIM/dogma, invbroker/invBroker)
```

2026-10-20 read `206 / 84 / 71 / 135 / 13`. **Their 206 is unchanged** — it is
the same archive and the same `unzip -l` output, re-run for this pass **[V]**
(`$DOWNLOADS/EveJS-261018.1.zip`). Ours went `84 → 87`: three
new services, `fleetObjectHandler`, `planetMgr` and `reprocessingSvc`, and
nothing was removed **[V]** (diffed against `git show e7a5d43:` for the
thirteen `service/*.rs` that existed then).

Two of those three close a row of the 135: `fleetObjectHandler` and
`planetMgr` are both on their list, so 135 → 133 and 71 → 73.
`reprocessingSvc` closes **nothing**, because theirs is spelled
`reprocessing` — it lands in the ours-only column instead, taking 13 → 14.
That is a measurement artefact, not a gap: §2's Combat-support row is served.

So **our service surface is still well under half of theirs**, and the overlap
is almost exactly our whole list. We have not taken a different slice of the
game — we have taken the docked prefix of the same slice, and the three lanes
deepened it rather than widening it.

Their subsystem count, by directory: **76 service families**
(`server/src/services/*/` is 78 directories, of which `_shared` and `_other`
are not families) **[V]**, plus a `server/src/space/` tree of its own with
destiny, combat, modules and NPC under it. The 2026-10-20 figure of 76 is
confirmed exactly.

---

## 1a. What the denominator means, and what it does not

`87 / 206` is **not 42% of a port.** The project has since measured the error
in both directions, and a reader who treats either number as progress is
misled twice over. Four things to hold at once:

1. **Service names overstate their side by ~2×, and understate it by ~2×.**
   `docs/port-tracker.md` §10.5: their sixteen `structure*` services are
   **one** feature — 13 of 16 `require ./structureState`, one table, one
   ~40-field record — and the honest unit of work is seven subsystems plus
   moon mining. The mirror image is corp, where **4 service names hide 132
   handlers and 19,000 lines**. Neither a row of §2 nor a name in the 206 is a
   unit of anything.

2. **The 133 is inflated by naming.** Crediting the two halves of our
   `skillMgr2::skillHandler` takes it to 131, and **12 more are a different
   spelling of something we answer** — `char`/`charMgr`, `dogma`/`dogmaIM`,
   `FastCheckout`/`FastCheckoutService`, `insurance`/`insuranceSvc`,
   `jumpClone`/`jumpCloneSvc`, `loginCampaign`/`loginCampaignManager`,
   `lp`/`LPSvc`, `market`/`marketProxy`, `ping`/`pingService`,
   `repackaging`/`repackagingSvc`, `reprocessing`/`reprocessingSvc`,
   `user`/`userSvc` — plus `calendarMgr`/`calendarProxy`, which the
   prefix test misses because neither suffix contains the other **[V]**
   (scripted against both lists). **~118 of the 206 name something we do not
   answer at all**; the other 15 of the 133 are bookkeeping.

3. **A landed subsystem can move zero rows.** The industry round
   (`e69ee89`, `4ce4694`) shipped 3,771 lines and 54 tests and closed **none**
   of the 133, because all four of its service names —
   `blueprintManager`, `facilityManager`, `industryManager`,
   `industryMonitor` — were already in the 71-name overlap on 2026-10-20.
   Reprocessing is the same mismatch from the other side: a real engine that
   added a name to *our* list without removing one from theirs. Two rounds,
   two different ways the row count fails to notice work.

4. **Three of our four landed subsystems were not ported from EvEJS.**
   `docs/port-tracker.md` §15.1 is the rule — check `out/pyc/eve/common/`
   first — and it holds for three of the four:
   - **PI**: CCP's own colony simulation,
     `out/pyc/eve/common/script/planet/baseColony.pyc`, whose
     `GetTypeAttribute` raises *"must be specifically implemented on the
     client and server"* — the proof it is shared code. EvEJS's 11,586-line
     `planetRuntimeStore.js` is a reimplementation of what we already had.
   - **Industry**: CCP's `out/pyc/industry/`, 91 KB across fourteen modules
     plus `activities/`, shipped in the client bundle
     (`server/crates/eve-server/src/industry.rs` header, **[V]** on the
     `boot.role == 'client'` branch that proves it runs under both).
   - **Reprocessing**: five static tables read out of **our** build's FSD, two
     of which only our build ships (`reprocess.rs` header).
   - **Fleets** is the one exception: EvEJS's `services/fleets/` supplied the
     decomposition, and every contract was then decided against our client.

   Across the project that is **eighteen disagreements with EvEJS, eighteen
   decided our way** (§15.3). So "EvEJS has 206 services and we have 87" is
   not a statement about how much of our server came from them. For the
   subsystems with a presence in `eve/common/` or `industry/`, **the client
   contains the specification** and EvEJS is a third party's reading of it.

The one thing `87 / 206` *is* good for is the §1 framing: a **breadth**
comparison of entry points at one moment, useful because the overlap tells us
we are on the same slice of the game. It is not a completion figure in either
direction.

---

## 2. The 133, grouped

Not a to-do list. Grouped so the shape is visible — notice how much of it is
**one feature with fifteen service names**. The "our state" column was
re-measured against the tree, not against the changelog, and carries the
tracker's taxonomy (`functional` / `partial` / `stub` / `none` / `unknown`)
applied to **our** side.

| cluster | their service names | our state |
|---|---|---|
| **Upwell structures** | `structure`, `structureDocking`, `structureGuests`, `structureVulnerability`, `structureDeployment`, `structureAssetSafety`, `structureJumpBridgeMgr`, `structureCynoBeaconMgr`, `structureProfiles`, `structureSettings`, `structureControl`, `structureDeliveries`, `structureCargoDelivery`, `structureCompressionMgr`, `structureHangarViewManager`, `structureDirectory` (**16**, not 15 — §10.5) | **stub.** `structureDirectory` only, 3 arms, all empty shapes (`service/world.rs`) |
| **Sovereignty** | `sovMgr`, `facWarMgr`, `fwVictoryPointMgr`, `colonyResourcesMgr`, plus a 42-file `services/sovereignty/` tree (skyhooks, hubs, campaigns, entosis) | **stub**, and "none" was too strong in October: none of their four **[V]**, but `fwCharacterEnlistmentMgr`, `fwWarzoneSolarsystem` and `insurgencySolarsystem` answer 6 reads between them, all `None`/empty (`service/space.rs`) |
| **Starbases (POS)** | `posMgr`, `moonExtractions`, `autoMoonMining`, plus 24 files (`starbaseCombatRuntime`, `starbaseFuelRuntime`, `starbaseProductionCycles`, `starbaseReinforcementRuntime`…) | **none** **[V]** (no arm, no name, nothing) |
| **Mining** | `miningCommand`, `miningScanMgr`, `miningResourceSite`, `characterMiningLedger`, `corpMiningLedger`, `inSpaceCompressionMgr` + 25 files | **none on the wire** **[V]** — but **its static layer landed**: `reprocess/tables.rs` (3,916 lines) carries `typematerials` and the 212-pair compressed-type map, which is exactly what mining's compression needs (`reprocess.rs` header; `b5d96b7`) |
| **Exploration** | `scanMgr`*, `hackingMgr`, `wormholeMgr`, + probe/signature/wormhole trees (28 files) | **stub, deliberately.** `scanMgr.SignalTrackerRegister` → `None`, and the bound `scanMgr::systemScanMgr.GetFullState` answers four empty `CRowset`s so the overlay draws an empty system instead of live-locking on a cached `None` (`service/space.rs`) |
| **Planetary industry** | `planetMgr`, `planetOrbitalRegistryBroker` | **partial.** 7 read arms over a ported **CCP** colony simulation — `planet.rs`, 5,541 lines, 64 tests, all 13 `UserUpdateNetwork` opcodes modelled. The write path (15 methods) is unanswered **on purpose**, pinned by `the_write_path_is_left_unanswered_on_purpose`. `planetOrbitalRegistryBroker` none |
| **Fleets** | `fleetMgr`, `fleetProxy`, `fleetObjectHandler` | **functional.** `fleetObjectHandler`: 33 arms (`CreateFleet` plus 32 object methods), `fleet.rs` 3,420 lines, 74 tests, invite proven across two sockets and then across two real clients (`139c0ad`). Session keys now sent (`95e2e41`). `fleetMgr`/`fleetProxy` are their names for the same feature |
| **Contracts & trade** | `contractMgr`, `tradeMgr`, `itemLocking`, `crate` | **stub.** `contractProxy` has 13 arms, every read an empty shape and every mutator a `CustomNotify` *"This server does not have contracts yet."* — more than "registered only" and no nearer a contract |
| **Market depth** | `market`, `lpStoreMgr`, `lp`, `essMgr`, `billMgr` | **stub**, and October's "browse + orders" overstated it: `marketProxy`'s 26 arms answer **descriptor-only empty rowsets**, and `PlaceBuyOrder`/`ModifyCharOrder`/`CancelCharOrder`/both multi-sell forms are no-op `None` **[V]**. No order can exist. `LPSvc` answers two balance calls; no LP store, no ESS, no `billMgr` |
| **Skills** | `skillHandler`, `skillMgr`, `skillMgr2`, `certificateMgr`, `alphaInjectorMgr` | **partial.** `skill::SKILLS` is **44** rows, not 23 — +18 reprocessing (`07a58d9`), +3 industry (`e69ee89`) — and the **ranks are real**, attribute 275 from `typedogma` **[V]**, which also retired the "every rank here is 1" stopgap. Still no training queue: `GetSkillQueueAndFreePoints` → `([], 0)`. No `skillMgr`, no `certificateMgr`, no `alphaInjectorMgr` |
| **Corp/alliance** | `corp`, `corpmgr`, `allianceRegistry`, `allianceFittingMgr`, `voteManager`, `ownerGroupManager`, `peaceTreatyMgr`, `mutualWarInviteMgr`, `warsInfoMgr`, `warStatisticMgr`, `killRightMgr` | **stub** — "shells" still exactly right. `corpRegistry`, `warRegistry`, `officeManager`, `corpStationMgr`: 9 arms, every one an empty map/list/`None`. Theirs: 4 names, 132 handlers, 19,000 lines (§10.5) |
| **Content/sites** | `dungeon`, `dungeonUniverseSite`, `keeper`, `abyssalMgr`, `abyssalContentManager`, `pvpFilamentMgr`, `voidSpaceMgr`, `epicArcStatus`, `missionTrackerMgr`, `customAgencyProvider` | **partial for missions, none for sites.** Still one hand-built mission, 4832 "Slave Shipment" (`mission.rs`), now per-character (`b2df04f`). `dungeonInstanceCacheMgr`'s 8 site-instance arms answer empty dicts — stub |
| **Combat support** | `entity`, `eject`, `repair`, `reprocessing`, `repackaging`, `insurance`, `superWeaponMgr`, `shipStanceMgr`, `warpVectorMgr`, `randomJumpMgr`, `solarsystemInterferenceMgr` | **split now.** `reprocessing` → our `reprocessingSvc` is **functional**: `GetQuotes` + `Reprocess` over `reprocess.rs` (1,773) + `tables.rs` (3,916), 41 tests — *the quoted tax is still not charged* (`service/reprocess.rs:427`, §15.6). `repackaging` → `repackagingSvc.RepackageItems` served; `insurance` → `insuranceSvc`, 6 arms. The remaining eight are the undocked ones: **none** |
| **Session/infra** | `sessionMgr`, `objCache`, `cache`, `loadService`, `tcpRawProxy`, `publicGateway`, `serviceGatewayMgr`, `sharedSettingsMgr`, `authentication`, `user`, `char`, `base` | **none of their names**, and the function is served elsewhere. *"`sessionMgr` is our Step C blocker" is retired*: Round 0.3 landed the identity split with no `sessionMgr` service at all (`session.rs`, `session_table.rs`, tracker §13), and "Step C" now has no other referent anywhere in `docs/` **[V]**. See §4.1 |
| **Live/meta** | `achievementTrackerMgr`, `activityMgr`, `rewardMgr`, `loginCampaign`, `loginRewardFacilities`, `raffleMgr`, `raffleProxy`, `tourneyMgr`, `communityTrackMgr`, `contextualOfferMgr`, `nesIntro`, `tutorialSvc`, `dailyGoals` | **stub.** 10 services, 14 canned arms, 236 lines in `service/live.rs`. Unchanged |
| **Chat** | `lsc`, `xmppChatMgr`, `chatAuthenticationService`, `bannedwords` | **stub.** `XmppChatMgr` has two arms, both `None`. `lsc` is also their **second slash-command entry point** (§10.9), so this row gates more than chat |
| **Misc** | `bookmark`, `overviewPresetMgr`, `nodeGraph`, `photoUpload`, `dynamicItem`, `encodedItems`, `dynamicResourceCacheMgr`, `infoGatheringMgr`, `ingamereport`, `eveguardReport`, `devIndexManager`, `externalQueueMgr`, `kiringMgr`, `rwManager`, `corpRecProxy`, `browserLockdown`, `characterEnergyMgr`, `shipCaster*` (4) | **bookmarks are not none**, and were not in October either: `accessGroupBookmarkMgr` answers `GetMyActiveBookmarks`, `AddFolder`, `UpdateKnownFolderState` (`service/world.rs`) — **partial**. Everything else in the row: **none** **[V]** |

`*` registered on our side but answering nothing of substance.

A note on the taxonomy, because it decides how to read the table: a **stub**
here is usually *chosen*. The empty `CRowset`, the `None`, the empty dict are
in most cases the one answer the client's own branch survives — several of
them are documented as having been a live lock or an `AttributeError` when
they were absent or wrongly typed. "Stub" means no engine behind it, not
"unfinished and broken".

---

## 3. `server/src/space/` — the part we have none of

Their space tree is a separate hierarchy from services, and it is where the
undocked game lives. File counts from the listing:

```
destiny/simulation    19   activeWarpTick, warpPhase, warpRamp, warpState,
                           warpInterdiction, warpInPointParity, motionState,
                           movement, nativeSubwarp, pilotWarpPhase
destiny/commands      19   warpCommands, subwarpCommands, docking, teleport,
                           drone, fighter, missile, mining, probe, tractorBeam,
                           superweapon, npc, npcWarpCompletion, starbaseForceField
destiny/delivery      17   sessionStamps, transitionStamps, sync, coordinateFrame,
                           sameSceneShipHandoff, postWarpDemotion, removeBallsSessionStamp
destiny/visibility    11   bubbleUpdater, acquisition, delta, membershipState,
                           warpHandoff, warpLanding, sceneTickCoordinator
destiny/presentation  10   addBalls, addBallsTimeline, removeBalls, shipPrime,
                           shipBoarding, freshAcquire, siteEnvironment, specialFxPayloads
destiny/dispatch       8   destinyDispatch, ticker, sceneRefresh,
                           observerReconciliation, watcherCorrections
destiny/stream         7   ballEncoding, ballConfig, freeBallHeader, statePayloads,
                           staticBallTail, primitives, actions
destiny/authority      7   destinyAuthority, destinySessionState, destinyContracts,
                           scenePositionAuthority, sceneRangeDecision
destiny/protocol       4   payloads, payloadIdentity, payloadSummary, direction
destiny/batching       4   parkUpdateBatcher, packagedAction, systemHistory
destiny/projection     3   entityProjection, kinematics, range
destiny/identity       2   entityID, interactionScope
destiny/bootstrap      2   initialBallpark, staticBootstrap
space/combat          11   damage, weaponDogma, laserTurrets, precursorTurrets,
                           vortonProjectors, missileSolver, missileReloads,
                           killmailTracker, breacherPods
space/modules         38   localCycleRuntime, moduleAttributeDiff, jammerModuleRuntime,
                           smartbombRuntime, reactiveArmorHardenerRuntime,
                           entosisLinkRuntime, commandBurstRuntime, microJumpDriveRuntime,
                           pointDefenseRuntime, salvagerRuntime, tractorBeamRuntime,
                           projectedWeaponModifiers, moduleClientParityAuthority
space/npc             ~50  npcBehaviorLoop, npcSelection, npcRuntime, npcRegistry,
                           npcEquipment, npcLoot, npcHullDamageability,
                           npcChaseSpeedModifiers, npcCombatDormancy, npcSentryPolicy,
                           beltRatRuntime, pirateFactionPolicy, capitalNpc* (10),
                           abyssalNpc*, trigDrifterNpc*, empireSecurityNpcCatalog
space/runtime         ~40  targeting, weaponCycle, weaponSnapshot, visibility,
                           timedInvulnerability, vortonArcChain, universeSiteAttach
```

Three of those filenames were our exact blocker on 2026-10-20:
`destiny/bootstrap/initialBallpark.js`, `destiny/presentation/addBalls.js`,
`destiny/stream/ballEncoding.js`. **The first of those three is no longer a
blocker.** Round 0.2 closed on 2026-10-21 (`c758c10`, tracker §17): the
Destiny state blob was dumped from our own client while docked and
`ReadFullStateFromStream` **accepted it back unchanged** — so the format is a
reference produced and consumed by the code that will parse ours, not a
reading of a format. The remaining two name what is left: **an encoded ball
stream, and `AddBall`** (now 17 arguments, all but one named, §17.2).

Do not read those files. Their *existence and naming* is the finding — it
corroborates the decomposition we had already inferred from the client, which
is exactly the permitted use (§0.3).

Our state for this whole section: **none**, and that is unchanged. Every row
in §2 that landed since the last pass is a docked subsystem.

---

## 4. The three rows that matter now

Everything above is scope. These three changed what we did next — and one of
them is now spent.

**4.1 `sessionMgr` was on their list and not on ours — and the thing it
blocked is done.** We identified the identity split and `station.GetGuests`
as the first multiplayer observable independently, from three hardcoded
`user_id: 1`s. An independent implementation carrying a `sessionMgr` service
was a second vote for the same ordering, and the ordering held: Round 0.3
shipped the split, `bf31907` made a push reach a character who is not the
caller, and on 2026-10-21 **two real clients** saw each other in one station
lobby with the guest list updating by push (`139c0ad`). We did it without a
`sessionMgr` service — the session lives in `session.rs`/`session_table.rs`
and the change rides the dispatcher — so the row stays open in the diff and
closed in fact. **Keep this entry as the worked example** of a §2 row whose
service name was never the unit of work.

**4.2 `space/npc/npcBehaviorLoop.js` + `npcSelection.js` exist.** I told the
owner the AI *engine* is the piece nobody shipped and that we would have to
write it over CCP's `AI_*` dogma attributes. That remains true of the
**client** — but it is no longer true that nobody has written one. We still
write ours from the attributes, and we now know the decomposition one other
project arrived at: a behaviour loop separate from target selection, separate
from a capability registry. (Partially verified since, §11.8: the chase
attributes exist, and CCP shipped *more* AI data than EvEJS reads.)

**4.3 `space/npc/npcLoot.js` exists, and the client ships no loot tables.**
This is the one place the two corpora disagree in an interesting direction.
`docs/re/dungeon-model.md` records loot as a **solid negative** — eight fields
across five tables reference a loot table with no table of loot tables to point
at. Yet they have a loot module. The inference available without reading it:
loot is **not** recoverable from the client, so theirs is either invented or
ESI/SDE-derived (the archive carries 9.3 MB of harvested ESI, §0.2). That
strengthens our negative rather than weakening it, and it tells us loot is a
**design** task for us, not an RE task. Worth saying out loud, because it is
the only subsystem on this page where "go look at the client" is not the
answer. Now **triple-confirmed**, §11.3.

---

## 5. What this does not tell us

- **Nothing about quality.** 206 service names is a count of entry points, not
  of working features. The release ships **no test directory and no
  `*.test.js` anywhere**; the only tests in it are **15 Rust `#[test]`s in
  their market daemon** (§10.8 — a correction to this section's original
  "zero tests"; 290 in the earlier public snapshot, stripped). Worse for
  assessment: `TODO`/`FIXME`/`NOT_IMPLEMENTED` over 150k lines of space and
  106k of corp/sov returns **zero**, so there is no textual signal separating
  finished code from aspirational code. We ship **787** `#[test]`s **[V]**
  (499 at the last pass). A name in §2 may be a stub exactly as several of
  ours are — and §2 now says which of ours those are.
- **Nothing about correctness.** Their build target is client **3396210**;
  ours is **3619879**, and `docs/re/evejs-crosscheck.md` §6 already found the
  build difference invalidating specific claims. Scope transfers across a
  build difference; wire details do not. Since the last pass the score is
  **eighteen disagreements, eighteen decided our way** (§15.3), including one
  field — `isLootLogging` — that does not exist in our build at all.
- **Nothing we may copy.** AGPL-3.0-only, with a marshal codec itself ported
  from EVEmu. Scope is a fact about the game; their code is not ours to have.
  (Our tree is under its own licence, `LICENSE.md`, since 2026-11-03, and
  holds no EvEJS code: porting is not allowed.)
- **No ordering.** This page deliberately does not rank the 133. Ranking them
  is a decision about what game we want (`eve-offline end goal`), and the owner
  makes it, not this file. §10.6's fleets call was made in the tracker, not
  here, and that is the right place for it.

---

## 6. What moved since 2026-10-20

Short by design: this section is what makes the document's staleness visible
next time instead of silent. Every row below was checked against the tree at
`139c0ad`, not against the commit message.

| §2 row | was | is | commit(s) |
|---|---|---|---|
| **Fleets** | none | **functional** — 33 arms, 74 tests, two real clients | `f31a375`, `bf31907`, `95e2e41`, `139c0ad` |
| **Planetary industry** | none | **partial** — 7 reads over CCP's own colony sim; write path deliberately unserved | `5e853cd`, `d6a750c` |
| **Combat support** (`reprocessing`) | none undocked | **functional** — `GetQuotes` + `Reprocess`, 41 tests; tax quoted not charged | `b5d96b7`, `212ae02` |
| **Mining** | none | **none** still, but the ore→mineral + 212-pair compression static layer landed under reprocessing | `b5d96b7` |
| **Skills** | 23 real skills, no queue | **44** skills, ranks real from `typedogma` 275; still no queue | `07a58d9`, `e69ee89` |
| **Market depth** | "browse + orders" | **stub** — a measurement correction, not a regression: the reads were always descriptor-only empties and the mutators always no-ops | — |
| **Session/infra** | "`sessionMgr` is our Step C blocker" | **retired** — Round 0.3 shipped the split without one; "Step C" has no referent left in `docs/` | `dfbf80a`…`de7ba3b`, `139c0ad` |
| **Contracts & trade** | `contractProxy` registered only | **stub** — 13 arms, empty shapes, mutators notify "no contracts yet" | pre-dates this pass |
| **Sovereignty** | none | **stub** — three FW/insurgency read services answer empties; "none" was too strong in October | pre-dates this pass |
| **Misc** (`bookmark`) | none | **partial** — `accessGroupBookmarkMgr`, 3 arms; also wrong in October | pre-dates this pass |
| **Exploration** | "answers nothing real" | **stub, deliberately** — the empty `GetFullState` is what keeps the overlay out of a live lock | pre-dates this pass |
| **§3 space tree** | three filenames are our blocker | **two** — Round 0.2 closed `initialBallpark` | `c758c10` |
| *(no row)* | — | **Industry** landed — 13 arms, 23 blueprints, 54 tests, ported from CCP's `out/pyc/industry/` — and moved **no** row of the 133, because all four of its service names were already in the overlap | `e69ee89`, `4ce4694` |

Four of the twelve corrections above are **measurement errors in the
2026-10-20 pass**, not changes in the tree (Market depth, Sovereignty, Misc,
Exploration). That is the useful lesson for the next refresh: the "our state"
column was written from memory of what had been built, and it drifted in both
directions — it over-credited the market and under-credited bookmarks and FW.
Re-derive it from `service/*.rs`, arm by arm.

**Still open and named elsewhere**, so this file does not duplicate it:
`docs/port-tracker.md` §15.6 is the open list from the three lanes. Two of its
five live entries have since closed — the 18 reprocessing skills with real
ranks (`07a58d9`) and the fleet session keys (`95e2e41`) — on top of the
`tools/fsd.py` entry it had already struck (§16). `UserUpdateNetwork`, the
reprocess tax and the `findings/client-calls.tsv` defects are still open **[V]**.

**What would settle the rows this pass could not.** Two kinds of `unknown`
remain, and neither is resolvable from the file listing:

- **Their quality, per row.** §5 says why: zero textual signal, 15 tests in
  the whole release. Only pass 2 — driving their server from the GM console
  (§10.9's 176 commands) — can separate a finished service from a named one.
  Until then every `stub`/`functional` judgement in §2's *their* column is
  `unknown`, which is why §2 does not have one.
- **Whether a row is one feature or fifteen.** §10.5 settled it for
  `structure*` and corp by reading their source, which this document may not
  do. For the other fourteen clusters the honest unit of work is `unknown`,
  and the thing that settles it is a per-cluster inventory in
  `docs/evejs/inventory/`, not a re-run of the diff.

---

## Appendix — reproducing the diff

### A.1 The recipe

Unchanged from 2026-10-20, so the numbers are comparable:

```sh
unzip -l /path/to/EveJS-261018.1.zip > listing.txt      # no extraction
grep -o 'server/src/services/[^ ]*Service\.js' listing.txt \
  | sed 's|.*/||; s|Service\.js$||' | sort -u > theirs.txt

for f in server/crates/eve-server/src/service/*.rs; do
  awk '/pub\(crate\) const SERVICES/{f=1} f{print} f&&/\];/{exit}' "$f" \
    | grep -o '"[a-zA-Z0-9_.]*"' | tr -d '"'
done | sort -u > ours.txt

# case and underscores differ between the two naming conventions
for x in theirs ours; do tr 'A-Z' 'a-z' < $x.txt | tr -d '_' \
  | LC_ALL=C sort -u > $x.norm; done
comm -23 theirs.norm ours.norm      # 133
```

Two environment notes for whoever re-runs it. `grep` here is a ugrep wrapper
that always passes `-I`, which is harmless on a text listing and **fatal on
`.pyc`** — use `/usr/bin/grep` for anything touching the client, with a
positive control, because "returned nothing" and "cannot see this file type"
look identical. And `comm` needs `LC_ALL=C` on *both* inputs or it reports
"not in sorted order" and silently gives a wrong answer.

### A.2 Three places the recipe lies, and by how much

The recipe is kept as-is for comparability, but it is not a clean count. All
three were present in the 2026-10-20 run too — the `"Repackage"` comment
landed in `5674ba6`, an ancestor of this document's own commit `e7a5d43`
**[V]** — so `84 → 87` is apples to apples even though neither number is the
true one.

1. **It picks up a string from a code comment.** `service/inventory.rs`'s
   `SERVICES` list contains the line *"`"Repackage"` in the hangar did nothing
   at all"*, and `grep -o '"…"'` cannot tell a comment from an element.
   **+1 phantom service.**
2. **It drops our three synthetic object names.** `skillMgr2::skillHandler`,
   `invbroker::inventory` and `scanMgr::systemScanMgr` are bound-object
   targets, not machoNet services, and `:` is not in the character class. That
   is the **right** behaviour for a machoNet entry-point diff — but it means
   `skillMgr2` and `skillHandler` sit in the 133 as services we "lack" when we
   answer twelve methods on that object. **−3 entries, 2 false absences.**
3. **The normalisation is too weak for their naming.** It folds case and
   underscores and nothing else, so `Svc`/`Service`/`Mgr`/`Proxy` suffix
   differences survive as separate names. See §1a.2 for the thirteen.

Counted exactly, with comments stripped and `::` names kept, our sixteen
`SERVICES` lists hold **89 entries: 86 machoNet service names and 3 synthetic
object names** **[V]**. Against that, the honest reading of §1 is: **86 real
service names on our side, ~118 of their 206 naming something we do not answer
at all.**

A future refresh should fix (1) by stripping `//` comments before the grep,
and leave (2) and (3) alone — but say so in §1, as this pass does. Changing
the recipe silently is how two passes stop being comparable.
