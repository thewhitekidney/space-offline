# R-FW: factional warfare, an implementation spec

Research lane R-FW of W12 (`docs/content/plan.md` §6.2). It is read-only:
no cargo was run and nothing outside this file was edited. It feeds CP53a
(`facWarMgr` and enlistment) and CP53b (VP, occupation and FW sites), and
it settles the open question in C2 §5 of where the warzone system list
comes from.

**Method.**
- Bytecode was read with `tools/dis.py` against `out/pyc` (build
  3619879). **Every `:N` is a `co_lnotab` line.** Paths drop `out/pyc/`
  and `.pyc`. `facWarSvc`, `fwWarzoneSvc`, `fwVictoryPointSvc` and
  `fwEnlistmentSvc` are under `eve/client/script/ui/services/`.
- Arity was counted from the `CALL_FUNCTION` operand that consumes the
  method's `LOAD_ATTR`, using a stack walk. Return shapes come from
  every consumer that reads the value.
- Static data was read with `tools/fsd.py` `rows()` against the
  `tq-offline` install (`start.ini` build 3619879).

**Marks:**
- **[V]** read from this build;
- **[V-data]** counted from this build's static data;
- **[I]** inferred, with the reason given;
- **[A]** an assumption we would ship, with what would falsify it;
- **unknown**.

---

## 0. Findings that change the plan

1. **The warzone system list is client static data** [V]:
   `res:/staticdata/warzonespositiondata.fsdbinary`. It is read by
   `fwwarzone/client/data/warzoneMapPositionsDataLoader:13`, and its
   schema is already in `findings/pyd-schemas.tsv`. Its keys are the
   warzone ids `1` and `2`, which confirms B1's [I] pairing (§2).
   CP53b's `fw_systems` seeds from it, so nothing has to be authored.
2. **B1's `GetAllWarzonesOccupationStates → {}` is a breaker** on the FW
   dashboard map. `fwwarzone/client/dashboard/mapData.GetWarzoneSystems:59`
   does `self.occupationStates[warzoneId]` without a guard, and `:123`
   does the same, so `{}` raises `KeyError`. The minimum fix is
   `{1: {}, 2: {}}`. The real fix is §3.
3. **B1 answers the wrong method name.** The client calls
   `fwCharacterEnlistmentMgr.GetMyDirectEnlistmentCooldownTimestamp`
   (`fwEnlistmentSvc:135`), not `GetEnlistmentCooldownTimestamp`, so
   that call is still unanswered. Its consumer is
   `facWarSvc.IsCharacterInFwCooldown:406-407`.
4. **B1's `GetLocalOccupationState` uses the wrong system id.** It
   answers with `config.characters.first().solar_system_id`, but the
   client compares the id against **its own** `session.solarsystemid2`
   (`fwWarzoneSvc:74-79`) and raises `RuntimeError` on a mismatch. With
   two characters, or after the first character jumps, that breaks. The
   id must be the calling session's `solarsystemid2`.
5. **Enlistment is a session change.** Both join paths poll
   `session.warfactionid` (`enlistCont.JoinMilitia_thread:165-175` and
   `EnlistDirectly:180`). Today the server hard-codes it to `None`
   (`service.rs:2658`), so enlistment needs a `warfactionid` session
   change. `jump::session_change` (`jump.rs:393`) is the pattern to copy.
6. **"Advantage" travels over the gateway, not macho.**
   `fwAdvantageSvc` calls `publicGatewaySvc` (`activityTracker
   get_solar_system_scores`, `:108`). It is out of scope here, and its
   panel section degrades through the gateway timeout (C2 §1.1).

---

## 1. `facWarMgr`: the methods

**Transport [V].** `facWarSvc.facWarMgr` (`:80-84`) is
`sm.RemoteSvc('facWarMgr')`, cached on `self.remoteFacWarMgr`. It is an
**unbound service call**, so it dispatches as an ordinary service named
`facWarMgr`. The calls are made through the attribute, which is why the
census missed them (C2 §5).

**Client-side caching [V].** `_RefreshCache:283-288` and the
join/leave methods invalidate `('facWarMgr', 'GetMyCharacterRankInfo' |
'GetMyCharacterRankOverview' | 'GetCorpFactionalWarStatus', ())` through
`objectCaching`. That means CCP served those three as cached method
calls. A plain answer works, because the invalidation of a missing entry
does nothing [I], so **answer plainly**.

The table lists 29 call sites: 28 through `self.facWarMgr`, plus
`GetControlBunkerIDInSystem` through `sm.GetService('facwar').facWarMgr`.
"Arity" counts the arguments after `self`. Shapes in `KeyVal(...)` are
`pyutil::keyval`.

| # | Method | Arity [V] | Call site | Return shape | Mark | Consumer that pins it |
|---|---|---|---|---|---|---|
| 1 | `GetCorporationWarFactionID(corpID)` | 1 | `facWarSvc:147` | `factionID` or `None`; falsy becomes `None` (`:148-150`) | [V] | NPC corps short-circuit client-side (`:140-144`), so only player corps reach us. 13 UI files go through the wrapper |
| 2 | `GetFactionCorporations(factionID)` | 1 | `:157` | list of corpIDs | [I] | the wrapper is called by no UI file (grep), so any iterable will do |
| 3 | `GetAllianceWarFactionID(allianceID)` | 1 | `:160` | `factionID` or `None` | [I] | as #1, for alliances |
| 4 | `GetFactionMilitiaCorporation(factionID)` | 1 | `:210` | corpID or `None` | [V] | `cfg.eveowners.Get(x).name` (`fwWarzoneDashboard:340`); `GetStanding(x, charid)` (`progression:121-122`, `overview:25-26`); LP wallet (`infrastructureHub:383-384`). **`None` breaks `DonateLPsToSolarSystem:668-669`** (`RuntimeError`) |
| 5 | `JoinFactionAsCharacter(factionID)` | 1 | `:278`, through `sessionMgr.PerformSessionChange('corp.joinmilitia', fn, factionID)` | ignored | [V] | session change: `warfactionid` must become `factionID` (§4) |
| 6 | `JoinFactionAsAlliance(factionID)` | 1 | `:303` | ignored (`POP_TOP`) | [V] | then `ScatterEvent('OnJoinMilitia')` |
| 7 | `JoinFactionAsCorporation(factionID)` | 1 | `:322` | ignored | [V] | as #6 |
| 8 | `LeaveFactionAsAlliance(factionID)` | 1 | `:331` | ignored | [V] | `enlistmentUtil:132` |
| 9 | `LeaveFactionAsCorporation(factionID)` | 1 | `:339` | ignored | [V] | `enlistmentUtil:143` |
| 10 | `WithdrawJoinFactionAsAlliance(factionID)` | 1 | `:347` | ignored | [V] | `enlistmentUtil:193` |
| 11 | `WithdrawJoinFactionAsCorporation(factionID)` | 1 | `:355` | ignored | [V] | `enlistmentUtil:195` |
| 12 | `WithdrawLeaveFactionAsAlliance(factionID)` | 1 | `:363` | ignored | [V] | `enlistmentUtil:170` |
| 13 | `WithdrawLeaveFactionAsCorporation(factionID)` | 1 | `:371` | ignored | [V] | `enlistmentUtil:172` |
| 14 | `GetCorpFactionalWarStatus()` | 0 | `:382` | `KeyVal(status, factionID, startDate)`; see the note below | [V] attributes | `.status` vs `facwarCorporation{Joining=0,Active=1,Leaving=2}` (`enlistmentChecks:27,45`, `enlistCont:117-118`); `.factionID` (`enlistmentUtil:126,137,163,185`); `.startDate` FILETIME (`fwWarzoneDashboard:735`); truthiness (`fwWarzoneDashboard:355`) |
| 15 | `GetMyCharacterRankInfo()` | 0 | `:390` | `KeyVal(currentRank, factionID)` or `None` | [V] | `.currentRank` (`personal:148`, `overview:30`, `fwWarzoneDashboard:727`); falsy is "not in FW" (`progression:125`); `getattr(rank,'currentRank',1)` and `getattr(rank,'factionID',None)` (`infosvc.GetRankEntry:635-636`) |
| 16 | `GetCharacterRankInfo(charID)` | 1 | `:392` | as #15, for another character | [V] | `infosvc:1423-1425` |
| 17 | `GetMyCharacterRankOverview()` | 0 | `:402` | list of `KeyVal(factionID, currentRank)`, one per faction ever served | [V] iterated, `.factionID` read | `infosvc:2803`, `decorationsPanel:207-208` (passed to `GetRankEntry`) |
| 18 | `RefreshCorps()` | 0 | `:421` | `None` | [I] | the wrapper has no UI caller |
| 19 | `CheckForRankChange()` | 0 | `:433` | new rank as an int, or `None` | [V] | `:434`: `is not None and != oldrank` leads to `DoOnRankChange`. Ranks are 0..9 (`:432`, `GetRankLabel:637`) |
| 20 | `GetStats_FactionInfo()` | 0 | `:460` | `{factionID: KeyVal(totalMembersCount, systemsCount, militiaMembersCount, topMemberCount)}` | [V] | `statisticsController.ChangeFormat:162-163` does `getattr(v, name, 0)`, then `:94-95,117-118,127,135` |
| 21 | `GetStats_TopAndAllKillsAndVPs()` | 0 | `:477`, `:505`, `:528` | `(top, all)`, each `{groupID: {statKey: n}}` for `groupCharacter`, `groupCorporation` and `groupAlliance` | [V] | `:480-481` `topStats[0|1][const.groupX][k]` |
| 22 | `GetStats_Character()` | 0 | `:488` | `{statKey: n}`, all six keys **required** | [V] | `personal:174` `stats['killsLW']`, `:187` `['vpLW']`, with no `.get` |
| 23 | `GetStats_Corp()` | 0 | `:509` | `{statKey: n}` | [V] | `.items()` at `:509` |
| 24 | `GetStats_Alliance()` | 0 | `:532` | `{statKey: n}` | [V] | `.items()` at `:532` |
| 25 | `GetStats_Militia()` | 0 | `:558` | `{'header': [factionID, ...], 'data': {statKey: {factionID: n}}}` | [I] from consumers | `statisticsController:73-74` `.get('header')`/`.get('data')`; `GetLine:225` `data.get(each, 0)` per header item; `GetMilitiaHeader:240-245` maps the header items to race or owner names |
| 26 | `GetStats_CorpPilots()` | 0 | `:562` | int | [I] | the wrapper has no UI caller |
| 27 | `GetSolarSystemLPs(solarSystemID)` | 1 | `:660` | int, the iHub LP pool (0..`facwarSolarSystemMaxLPPool`=300000) | [V] | `:676` adds it to an int; `factionalWarfareSystemContentPiece:95` compares it |
| 28 | `DonateLPsToSolarSystem(pointsDonated, pointsToIhub)` | 2 | `:680` | returned to the caller and ignored | [I] | `infrastructureHub`. The client pre-checks with `UserError` `FacWarCantDonateSoMuch`/`FacWarPoolOverloaded` |
| 29 | `GetControlBunkerIDInSystem()` | 0 | `factionWarfareSystemInfoContainer:52` | itemID or `None` | [V] | `ballpark.GetInvItem(id)`, then falsy means return (`:53-55`), so `None` is safe |

**`statKey` [V]**: `killsY`, `killsLW`, `killsTotal`, `vpY`, `vpLW`,
`vpTotal` (`facWarSvc:469-474`, `statisticsController:78-88`).

**#14, the "not in FW" value.**
- [I] Return `None` for a character in an NPC corp. The dashboard's
  `:355` falsy branch shows "time served" for a direct enlistee.
- [A] Return `KeyVal(status=None, factionID=None, startDate=None)` for a
  player corp that is not in FW. `enlistCont:114-117` and
  `enlistmentChecks:24-27,42-45` read `.status` unguarded whenever the
  caller is a director. *Falsified by* a traceback on the dashboard for
  a directly enlisted player-corp member.

**Server-side only, not ours to answer [V]:**
- `facWarMgr.GetCharacterEnrolledFaction` (`evestations/specialStationRules:66,79`);
- `GetSolarSystemLPsEx` (`facwarCommon:243`).

Both use `sm.GetService`, the server half of shared code.

**Sibling remotes on the same feature [V]:**

| Service.method | Arity | Site | Shape |
|---|---|---|---|
| `fwCharacterEnlistmentMgr.GetMyEnlistment` | 0 | `fwEnlistmentSvc:80` | `(factionID, directFactionID, corpFactionID)`, unpacked 3 (B1 answers `(None,)*3`) |
| `.CreateMyDirectEnlistment(factionID)` | 1 | `:98` | ignored |
| `.RemoveMyDirectEnlistment()` | 0 | `:108` | ignored |
| `.GetCorpAllowedEnlistmentFactions(corpID)` | 1 | `:114` | list of factionIDs (`in` at `enlistmentChecks:74`; `sorted` at `corp_ui_home:605`, `infosvc:1538`) |
| `.SetMyCorpAllowedEnlistmentFactions(ids)` | 1 | `:121` | returned to the caller and ignored (`corp_ui_direct_enlistment_mgt:158`) |
| `.GetMyDirectEnlistmentCooldownTimestamp()` | 0 | `:135` | FILETIME or `None` (`None > now` is `False` in py2, so it means "no cooldown") |
| `fwVictoryPointMgr.GetLocalVictoryPointState()` | 0 | `fwVictoryPointSvc:48,51` | `(session.solarsystemid2, VPState or None)`, unpacked 2. A mismatched id is retried once, then stays `STATE_UNKNOWN` |
| `fwVictoryPointMgr.GetAllVictoryPointStates()` | 0 | `:59` | `{ssid: VPState}` (`.get`) |
| `fwWarzoneSolarsystem.*` | 0 | `fwWarzoneSvc:74,105,113,221,225` | B1's five arms; see §0 items 2 and 4 |
| `dungeonInstanceCacheMgr.GetFactionWarfareBattlefieldInstances()` | 0 | `fwWarzoneSvc:186` | `{ssid: [row]}` (C2 §5) |

---

## 2. Where the warzone system list comes from

**Source [V]:** `warzonespositiondata.fsdbinary`, loaded by
`warzoneMapPositionsDataLoader` (`__resBuiltFile__` `:13`,
`get_frontlines_map_data:18`). The row layout is:

```
{warzoneID: {systems: {ssid: {solarSystemId, xpos, ypos, isVisible?, territorySprite?}},
             mapWidth, mapHeight}}
```

**How the client splits it [V]** (`mapData.GetWarzoneSystems:59-92`):
- rows with `isVisible` falsy are skipped (`:68`);
- a row that has a `territorySprite` **and** whose ssid is in
  `occupationStates[warzoneId]` is a warzone system (`:81-87`);
- every other row is drawn as a "navigation system" (`:89`).

**Counts [V-data]:**

| warzone | rows | visible | visible with `territorySprite` | their `systems.static.factionID` |
|---|---:|---:|---:|---|
| 1 | 103 | 100 | **70** | Minmatar 42, Amarr 28 |
| 2 | 149 | 132 | **93** | Caldari 54, Gallente 39 |

Warzone 2 also has 8 hidden rows with a sprite.

**What this settles:**
- **[V-data] Warzone 1 is Amarr/Minmatar and warzone 2 is
  Caldari/Gallente.** B1's [I] is confirmed.
- **[V-data] B1's four HQ systems** (Arzad, Amamake, Kedama, Ouelletta)
  are all territory systems of the right warzone. Whether they are this
  build's HQs is still [A].

**One sample row**, warzone 1:

```
30002056 {'territorySprite': {'territoryIconFile': 'res:\\UI\\Texture\\classes\\frontlines\\territories\\1\\30002056.png', 'imageHeight': 550, 'imageWidth': 567, 'xOffset': 4603, 'yOffset': 1466}, 'solarSystemId': 30002056, 'xpos': 4878, 'ypos': 1732, 'isVisible': 1}
```

**Spec for CP53b:**
- **[I] membership.** `fw_systems` = the visible rows that carry a
  `territorySprite`, 163 systems. The reason: those are exactly the rows
  the client textures as territory.
- **Extraction.** Extract with a `tools/content.py` reader over
  `fsd.rows(client, 'warzonespositiondata')`; never commit the rows.
- **[A] seed occupier** = the system's `systems.static.factionID`.
  *Falsified by* nothing that matters: TQ's live front is not
  reproducible anyway.
- **`pirateFactionID`** per warzone: 1 is Angel Cartel `500011`, 2 is
  Guristas `500010` ([I], B1's `pirateinsurgency/client/dashboard/const:19-21`).
- **[A] adjacency.**
  - `FL` = a stargate neighbour is held by the other militia;
  - `SL` = a neighbour is `FL`;
  - everything else is `BL`.

  The constants are [V] (`factionwarfare/const:1-3`); the rule is public
  knowledge. *Falsified by* the dashboard's front lines disagreeing with
  the occupation on screen.

---

## 3. Wire shapes of the state objects [V]

Both classes are new-style `0x23` objects, so use `pyutil::new_style`.
Both class names are in the rowset whitelist (`whitelist.rs:183-184`),
and `__init__` never runs (C2 §1.2), so **the server must uphold these
invariants**.

**`factionwarfare.occupationState.OccupationState`** (`occupationState:10-27`):

| key | type | invariant |
|---|---|---|
| `_occupierID` | factionID | not `None` |
| `_attackerID` | factionID | the other militia (`potentialOccupierIDs` is `[occupier, attacker]`, `:59`) |
| `_adjacencyState` | `'FL'`, `'SL'` or `'BL'` | in `ADJACENCY_STATES` |
| `_pirateFactionID` | factionID | not `None` |
| `_dateTaken` | FILETIME or `None` | `mapData:127` sorts on it |

**`factionwarfare.victoryPointState.VictoryPointState`** (`victoryPointState:5-15`):
- `_vpScore` is an int ≥ 0, and `_vpThreshold` is an int > 0.
- `isVulnerable` = `score >= threshold` (`:41`).
- `isContested` = `score > 0` (`:46`).
- `contestedFraction` = `min(score/threshold, 1.0)` (`:51`).

**`GetAllWarzonesOccupationStates`** and the `Uncached` variant return
`{1: {ssid: OccupationState}, 2: {...}}`. Both warzone keys must be
present (§0 item 2), and every §2 system must be in its warzone's dict.

---

## 4. Enlistment

**Constants [V]** (`factionwarfare/const`):
- `ENLISTMENT_COOLDOWN_SECONDS = 86400` (`:36`);
- `facwarCorporationJoining = 0`, `Active = 1`, `Leaving = 2`
  (`appConst:1305-1307`).

**Militia corps [V-data]** (`npccorporations` `factionID`):

| corp | ticker | faction |
|---|---|---|
| 1000179 | IC24 | Amarr |
| 1000180 | SPROT | Caldari |
| 1000181 | FEDEF | Gallente |
| 1000182 | TLIB | Minmatar |
| 1000436 | MALZT | Angel |
| 1000437 | CMDGR | Guristas |

**Persisted state, a new delta row:**
`enlist(charID, directFactionID, since, cooldownUntil)`, plus
`corp_fw(corpID, factionID, status, startDate)` for corporations.

**`GetMyEnlistment`** returns
`(directFactionID or corpFactionID, directFactionID, corpFactionID)`.
The first element is "my faction". [I] from `fwEnlistmentSvc:59-77`:
`GetMyFaction` reads the first, and `IsEnlistedDirectly` and
`IsEnlistedViaCorp` read the other two `is not None`.

**Direct enlistment, `CreateMyDirectEnlistment(f)` and its legacy twin
`JoinFactionAsCharacter(f)`:**
1. Refuse if already enlisted, or if `now < cooldownUntil`. Use a
   `UserError('CustomNotify', {'notify': ...})`, the shape the client
   itself raises (`fwEnlistmentSvc:95`) [I].
2. Write the row.
3. Queue a session change `warfactionid: (None, f)` [V need, §0 item 5].
4. Push `OnEnlistmentStateUpdated(f, f, None)` [V arity,
   `fwEnlistmentSvc:50`] to the character.

[Superseded by FW-2] `JoinFactionAsCharacter` from an NPC corp **does**
move the character into the militia corp (the client's `corp.joinmilitia`,
`_GetRetireBtns`, `IsFwNpcCorp`, employment-record cooldown need it); one
session change carries `corpid` and `warfactionid`, and Retire is
`corp.KickOut`. Only the `corpid` move itself stays [I]. A player-corp pilot
and `CreateMyDirectEnlistment` (the bots) still enlist directly.

**`RemoveMyDirectEnlistment()`:**
1. Clear the row and set `cooldownUntil = now + 86400 s`.
2. Session change `warfactionid: (f, None)`.
3. Push `OnEnlistmentStateUpdated(None, None, None)`.
4. When the cooldown expires, push `OnEnlistmentCooldownCleared()`
   (arity 0, `:138`); the client then re-fetches the timestamp.

**Corporation and alliance paths (#6–#13):**
- Join sets `corp_fw.status = Joining`; [A] it promotes to `Active`
  after a short wait, then pushes `OnEnlistmentStateUpdated` to every
  online member.
- Leave sets `Leaving`.
- Withdraw reverts to the previous status.
- These matter only for player corps; they can ship after the direct
  path.

**`GetCorpAllowedEnlistmentFactions`.** [A] It returns all four empire
militias unless a director has called `SetMyCorpAllowedEnlistmentFactions`.
*Falsified by* the enlist window warning that the corp disallows a
faction the director never restricted.

**Rank** (#15–#17, #19):
- [I] `currentRank = clamp(int(standing toward the militia corp), 0, 9)`.
  `OnNPCStandingChange:430-432` recomputes it exactly this way, using
  the standing toward `GetFactionMilitiaCorporation(warfactionid)`.
- `CheckForRankChange` returns the new rank whenever it differs from the
  stored one, and stores it.

---

## 5. Victory points and the push rules

**Constants [V]** (`factionwarfare/const:19-21`):
- `VICTORY_POINTS_THRESHOLD = 75000`;
- `MAX_VICTORY_POINTS_OVER_THRESHOLD = 500`;
- `PIRATE_WIN_VICTORY_POINTS_OVER_THRESHOLD = 100`;
- the LP multipliers by adjacency are FL 1.5, SL 1.0 and BL 0.01
  (`:14-16`).

**Rules:**
1. **[I] The threshold is per system and constant.** Every
   `VictoryPointState` carries `_vpThreshold = 75000`.
2. **[A] A completed FW complex adds VP** to its system, in favour of the
   completing militia. If that militia is the attacker the score rises;
   if it is the occupier, the score falls, floored at 0.
   - Points by archetype: 33 Novice 2,500; 34 Small 5,000; 35 Medium
     7,500; 36 Large 10,000; 68 Battlefield 15,000.
   - All five values are [A]; a game knob tuned for a few players.
   - *Falsified by* nothing in the client: the per-site VP is
     server-only (unknown).
3. **[V] Vulnerability.** At `score >= threshold` the system is
   vulnerable (`victoryPointState:41`). The score is capped at
   `threshold + MAX_VICTORY_POINTS_OVER_THRESHOLD` [I from the name].
4. **[A] The flip.** A vulnerable system flips at the next daily
   rollover (`population::on_rollover`):
   - `occupierID` and `attackerID` swap, `dateTaken = now`, and the
     score resets to 0;
   - then adjacency is recomputed for the system and its neighbours.

   *Falsified by* the fanfare not playing for players who are in the
   system.

**Pushes.** Each one is a type in `src/notify.rs` (C2 §1.4); "system"
means a narrowcast to the characters whose `solarsystemid2` is that
system.

| Push | Arity [V] | When | To | Client guard |
|---|---|---|---|---|
| `OnVictoryPointStateUpdated(ssid, VPState)` | 2 (`fwVictoryPointSvc:62`) | the score changes | system | ignored unless `ssid == solarsystemid2` (`:65`) or when equal to the cache (`:68`); otherwise it refreshes the info panel |
| `OnFacWarBunkerVulnerable(attackerID)` | 1 (`:78`) | the score crosses ≥ threshold | system | dialog |
| `OnFacWarBunkerNonVulnerable(attackerID)` | 1 (`:81`) | the score drops below threshold | system | dialog |
| `OnWarzoneOccupationStateUpdated(ssid, OccupationState)` | 2 (`fwWarzoneSvc:115`) | a flip, or an adjacency change | system | an `occupierID` change plays the flip fanfare (`:130-131`); an adjacency change plays `ShowChangedAdjacency` (`:132-133`) |
| `OnFacWarDungeonCompleted(wasConquered, factionID)` | 2 (`:145`) | a complex is completed | [A] the completing characters | video |
| `OnFwObjectiveCompletedInSolarSystem(ssid, targetFactionID, objectiveMode)` | 3 (`:135`) | later: objectives (`OBJECTIVE_MODE_*` strings, `const:24-28`) | system | video |
| `OnFwScoreboardWon(ssid, scoreboardItemID, winnerFactionID)` | 3 (`:140`) | later: battlefields | system | video |
| `OnSolarSystemLPChange(oldpoints, newpoints)` | 2 (`facWarSvc:87`) | an iHub LP donation | system | it writes `solarSystemLPs[session.solarsystemid2]` |
| `OnEnlistmentStateUpdated(f, direct, corp)` | 3 | §4 | the character | |
| `OnEnlistmentCooldownCleared()` | 0 | §4 | the character | |

**Session-change invalidation [V].**
- On a `solarsystemid2` change the client resets its VP cache
  (`fwVictoryPointSvc:34-37`) and re-asks `GetLocalVictoryPointState`.
- `GetAllWarzonesOccupationStates` is not memoized (B1's note), so the
  dashboard refreshes by itself.
- Neither push needs to be sent on a jump.

---

## 6. FW sites: `dunArchetypesFactionalWarfare`

**[V]** `appConst:614` defines `dunArchetypesFactionalWarfare =
(33, 34, 35, 36, 68)`:
- 33 `ComplexNovice`, 34 `ComplexSmall`, 35 `ComplexMedium` and
  36 `ComplexLarge` (`:569-572`);
- 68 `dunArchetypeFactionaWarfareBattlefieldSites` (`:600`, CCP's typo).

**Dungeon rows [V-data].** `fsd.rows(client, 'dungeons')` has 5,516 rows,
177 of them in these archetypes. Entry types: 28356 *Cosmic Anomaly*,
19728 *Deadspace Signature*, 10645 *Celestial Beacon*, and 73262/73263
*Strategic … Cluster* (group 310).

| arch | dungeons | per empire faction (anomaly + signature) | difficulty | other |
|---|---:|---|---|---|
| 33 Novice | 40 | 9 + 1 each | 4 (38), `None` (2) | — |
| 34 Small | 60 | 14 + 1 each | 4 | — |
| 35 Medium | 32 | 8 anomaly each | 4 | — |
| 36 Large | 36 | 8 anomaly each, plus 1 for Gallente | 4 (34), 2 (2) | 2 cluster-entry rows; 1 Angel (`500011`) row |
| 68 Battlefield | 9 | 1 beacon + 1 anomaly each (Amarr has 2 beacons) | 4 | — |

**One sample row**, with dict and list fields summarised:

```
2774 {archetypeID: 33, difficulty: 4, dungeonID: 2774, dungeonNameID: 111116, entryObjectID: 892829, entryTypeID: 28356, factionID: 500003, rooms: dict(2), connections: list(1), contentTags: None, ...}
```

**Spec for CP53b and C3's generator:**
- **[I] a site belongs to its owner.** A row's `factionID` is the militia
  that owns the complex, so the site sits in a system of that faction's
  warzone. Completing it is a capture *against* the owner.
- **Spawning.**
  - Spawn archetypes 33–36 as anomalies (entry 28356) in warzone
    systems. The `population.tsv` rows are keyed by archetype, with
    `faction` drawn from the system's `occupierID` (or its `attackerID`).
  - [A] Per system: 1 Novice, 1 Small, 1 Medium and 0–1 Large.
  - Leave 68 (battlefields) for later. They also feed
    `GetFactionWarfareBattlefieldInstances` and the `BattleFieldAdded`
    scatter (`fwWarzoneSvc:172`, arity 3: `itemID, typeID, score`).
- **"Completed"** means the pocket is cleared, the liveness pattern in
  plan §6.2 CP53b. `facwar::tick` pulls completed slots and applies §5
  rule 2.
- **Unknown.** The client holds no data on capture timers or on the
  ship-size restriction gates; both are server-only. [A] Rely on the
  dungeon's own `connections` and rooms as they are.

---

## 7. Unknown, and what would settle it

- The per-complex VP values and the flip timing (§5 rules 2 and 4): these
  are server-only, and no client artifact holds them.
- Whether `isVisible = 0` rows carrying a `territorySprite` (8 in warzone
  2) are contestable: a TQ capture would settle it.
- The CCP default for `GetCorpAllowedEnlistmentFactions`.
- The HQ systems: still B1's [A], which is now only weakly supported
  (§2).
- Advantage (`fwAdvantageSvc`) needs the gateway lane (P6.2).
