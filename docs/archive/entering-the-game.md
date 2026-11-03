# From the character click to a station hangar

> **Demoted 2026-10-24** — its status header is a 2026-10-19 snapshot. Current truth: the code. Kept for history; do not brief a lane from this file.

Reconnaissance, done by decompiling this build before implementing any of it.
It is the map for the step after M1: what the client does once a character is
selected, in order, and what each call has to return.

**Status: implemented up to and including the session change.** The
pre-check (§0), the notification itself (§1) and `crimewatch.GetClientStates`
(§2.2) are built; `dogmaIM.GetAllInfo` (§2.1) is the next gate and is **not**,
nor is anything in §2.3. `[V]` is read out of this build's bytecode, `[I]` is
reasoned from it.

Read `docs/reference/bound-objects.md` for the bind protocol itself and `docs/reference/login-path.md` §8
for the calls up to the select screen.

---

## 0. The pre-check, before anything changes

Two plain calls and one bound one, all in `characterSelection`:

| call | answer | note |
|---|---|---|
| `charUnboundMgr.GetCharacterLockType(charID)` | `None` | anything else raises `UserError` and refuses the character |
| `populationCap` bind → `GetCharacterLoadSlot(charID)` | `{}` | implemented; see `docs/reference/login-path.md` |
| `charUnboundMgr.SelectCharacterID(charID, None, False)` | **return discarded** | implemented; the *point* of it is the session change |

`jumpQueue.PrepareQueueForCharID` is purely local — no `RemoteSvc` or
`Moniker` anywhere in it. **[V]**

## 1. The `SessionChangeNotification` (command **16**)

**Not 19** — an earlier draft of this document said 19 and that was wrong.
Every id in the table is defined in the client's own `cluster/__init__.pyc`,
and `MACHONETMSG_TYPE_SESSIONCHANGENOTIFICATION` is **16**. Our
`MachoNetMsg` table matches it on all fifteen entries. **[V]**

`params` are `['sid', 'change', 'nodesOfInterest?']`, and the server's own
builder is explicit about the shape:

```python
# SessionChangeGPCS.SessionChanged
packet = machoNetPacket.SessionChangeNotification(
    destination=machoNetAddress.MachoAddress(clientID=clientID),
    sid=sid, change=(0, change), nodesOfInterest=nodesOfInterest)
```

so `change = (0, {attr: (old, new)})`, handled by
`SessionChangeGPCS.NotifyUp` → `ApplyRemoteAttributeChanges(change[1],
initialState=False)`. The leading `0` is never read by the client — only
`change[1]` is — but it is what the real server sends; the proxy's own
node-to-node forwarding puts `1` in that slot. `nodesOfInterest` is read
only in proxy mode, through `getattr(..., None)`, so `None` is right for a
client-bound change. **[V]**

Three things about it that are easy to get wrong:

1. **The `old` values are discarded.** `ComputeNotifySets` rebuilds every pair
   as `(the client's current value, new)`. **[V]**
2. **Attributes whose new value equals the current one are dropped.** If that
   empties the set, *nothing happens at all, silently* — no handlers run. A
   session change that looks correct and does nothing is this. **[V]**
3. **`locationid` and `corprole` must not be sent.** They are derived on the
   client by `RecalculateDependantAttributesWithChanges`:
   `locationid = stationid or solarsystemid`, and `corprole` from the
   `rolesAt*` set according to whether `stationid` matches `hqID`/`baseID`.
   **[V]**

The authoritative field list is `SessionMgr.GetSessionValuesFromRowset` in
`eve/common/script/sys/eveSessions.pyc` — the real server's own
session-building code, which ships in the client tree: `allianceid`,
`warfactionid`, `corpid`, `hqID`, `baseID`, `rolesAtAll`, `rolesAtHQ`,
`rolesAtBase`, `rolesAtOther`, `fleetid`, `fleetrole`, `wingid`, `squadid`,
`shipid`, `stationid`, `structureid`, `solarsystemid`, `regionid`,
`constellationid`, `genderID`, `bloodlineID`, `raceID`, `corpAccountKey`, plus
`charid`. When docked, `stationid` is set and `solarsystemid` stays `None`
while `solarsystemid2` carries the system. **[V]**

`gameui.OnSessionChanged` routes on the **live session**, not the change dict:
`charid is None` → charsel, `elif stationid is not None` → station, `elif
structureid`, `elif solarsystemid`, else a "Lame Teardown" warning and nothing.
But `_HandleSessionChangeInStation` then acts only `if 'stationid' in change`.
So the change must carry at least **`charid` and `stationid`**. **[V]**

**Keep every `rolesAt*` at 0.** Then `corprole` is not in the derived change,
which suppresses a blocking `officeManager.GetMyCorporationsOffices()` in the
critical path (§2.2). **[V]**

**`shipid` is now sent, and the character is in a capsule.** The hangar draws,
so the condition the paragraph below was waiting on has been met. What follows
is kept because its *reasoning* is still the reasoning — it is the argument
for why a ship is a deliberate step with a cost, not a one-line session field,
and that argument is what the step actually turned out to require: an item row
in the hangar listing, `GetAllInfo` growing `shipInfo` and a real 4-tuple
`shipState`, a per-container `GetSelfInvItem`, two new draw-path calls, and a
location row for `cfg.evelocations`. See §2.5.

The original note, for the record:

**`shipid` was deliberately absent, and was to stay that way until the hangar
drew.** This was re-examined properly after the first run reached the station
and logged `Unexpectedly got shipID = None`, and the conclusion got stronger
rather than weaker.

`shipid` is a perfectly legitimate session attribute — it is in
`GetSessionValuesFromRowset` as `si.shipID`, it is const slot 23 on
`CoreSession` and a member of `__characterDependantAttributes__`, and
`eveSessions` registers it in `sessionsByAttribute` so the client maintains its
own index for it. Nothing derives it, so sending it is the only way the client
can have it, and nothing type-checks it: it is an item id. **[V]**

We leave it out anyway, because **a docked character does not need a ship for
the hangar to draw**, and three independent guards say so explicitly:

1. `hangarView.SetUpModels` → `GetShipItemFromHangar(session.shipid)`, which
   scans the hangar and falls off the end returning `None`.
   `GetActiveShipTypeID()` then returns `None` by its own guard.
2. `hangarRegistry.GetHangarType` takes that `None` safely
   (`if shipTypeID and ShipNeedsACapitalHangar(shipTypeID)`) and otherwise
   picks the hangar from the **station** type alone — scene, model, camera and
   behaviours are all station-derived. This is why `GetStationItemBits`'
   `stationTypeID` has teeth and the ship does not.
3. `hangarView.ReplaceExistingShipModel` opens with
   `if not itemID or not typeID: return`, skipping the ship model, turrets,
   dock animation and `camera.SetShip`.

The shipless state stays first-class all the way to the undock button:
`undockingSvc.ExitDockableLocation` branches on `if shipID is None:` into
`_DealWithMissingShip()` → a ship picker or a `NeedShipToUndock` prompt. **[V]**

What omitting it actually costs, in full: **one** `LogTraceback` record per
login, and a `corvetteButton` controller tasklet polling
`godma.IsItemReady(session.shipid)` at 10 Hz for the life of the lobby window.
The traceback is reached before anything is written — before the
`makeShipActive` lock, before `shipIDBeingEmbarked`, and before `shipState` is
unpacked — so no lock leaks and the `shipState is None` fallback (a nested
`PerformSelectiveSessionChange('board', ...)` with a bound `ship.Board` call)
never fires. `MakeShipActive` is `uthread.pool`, fire-and-forget, so
`godma.Prime` does not wait on it either. **[V]**

And what adding it would cost, which is the real argument: a ship pulls
`baseHangarBehaviours.LoadShipModel` into the hangar draw path, and that calls
`ship.GetDirtTimestamp` (bound), `shipKillCounter.GetItemKillCountPlayer` and
`cosmeticsSvc.GetAppliedSkinState`. By the rule in `docs/guides/debugging-the-client.md`
§1, the first two become hangs *in the draw path* — so a ship is not a
one-line session field but a deliberate step with at least three new calls, an
item row in the hangar listing (`categoryID 6`, `singleton 1`, matching the
`GetItemDescriptor` header exactly, or `List` drops it), and `GetAllInfo`
growing `shipInfo` plus a real 4-tuple `shipState`. A capsule (`typeID 670`,
group 29) is the cheapest honest ship, and is what TQ itself falls back to on
`ShipNotInHangar`. **Do that after the hangar draws, not to make it draw.**

### `sid` is not validated on the client, so ours need not match

Worth knowing before anyone chases it: the client's own session reports a
`sid` of its own (`4180784862437702421` on one run) while our notifications
carry `sid = 1`, and the client does not care.
`machoNetTransport.LockSession` resolves the session for an incoming packet
and, on a client, takes the global one unconditionally:

```python
if macho.mode == 'client':
    sess = session
elif serviceSession:
    ...
elif isinstance(packet, SessionInitialStateNotification):
    sess = base.CreateSession(packet.sid, packet.sessionType)
```

`packet.sid` is only consulted on the **server/proxy** side, where it creates
the session. So for us the field is informational, and a mismatch is not the
cause of anything. **[V]**

### Nesting: the client calls us while our answer is outstanding

The real server sends the session change **inside** `SelectCharacterID`,
before responding, so the client runs the whole handler chain — including new
*blocking* remote calls — while `SelectCharacterID` is still unanswered. **[V]**

Our `conn::serve` already copes: it dispatches a payload, writes the replies
and loops straight back to reading, so it never waits on anything. Sending the
notification and the `CallRsp` in one reply is enough; the nested `CallReq`s
arrive afterwards and are answered from the same loop. Confirmed safe on the
client side too — responses are routed to the waiting tasklet's channel and
**never take the session lock**, and `TransportReader` spawns its replacement
before handling a packet. **[V]**

What we must *not* do is send a notification that the session-change handlers
are themselves waiting on: every other even-numbered command takes a session
**read** lock and would sleep until the change completes. **[V]**

## 2. The handler waves

`DoSessionChanging` runs first and is **block-trapped** —
`stackless.getcurrent().block_trap = 1`, so any remote call there raises. Every
listener is local. Nothing to answer. **[V]**

Then `ProcessSessionChange`, synchronous and blocking-allowed. This is the hot
path. `invCache` calls `sm.FavourMe` in `Run`, which puts it **first**. **[V]**

Which services are running at this point was resolved from the dependency
graph rather than guessed: `gameui.__startupdependencies__` includes `station`,
and `station → crimewatchSvc → {michelle, godma, war, facwar, corp,
securitySvc}`, `godma → invCache`. So `invCache`, `godma`, `crimewatchSvc`,
`michelle`, `station`, `journal` and `dungeonTracking` are up; `fleet`, `space`,
`target`, `skills`, `charactersheet`, `contracts` and `clientDogmaIM` are
not. **[V]**

### 2.1 `dogmaIM.GetAllInfo` — the one that matters, and the current wall

**This is where the client now stops.** It cannot be stubbed empty, which is
why it is a piece of work rather than another arm: `Prime()` ends with an
**unconditional**

```python
char = self.GetItem(session.charid)
for att in [3, 4, 5, 6, 7]:
    sm.ScatterEvent('OnAttribute', att, char, getattr(char, att))
```

so the character must actually have been primed, which means `.charInfo` must
carry a real item for it. `PrimeLocation` then calls `UpdateItem(row.invItem,
row)` per entry — reading `.itemID`, `.locationID`, `.typeID`, `.groupID`,
`.categoryID`, `.flagID`, and requiring `groupID == groupCharacter` to pass
its category filter — and finishes with
`invCache.GetInventoryFromId(itemID, 1)`. So satisfying this one call reaches
into the item row, the dogma attribute store and the inventory layer at
once. **[V]**



`invCache.ProcessSessionChange` ends in a `finally` that calls
`godma.ProcessSessionChange` → `Prime()`, which binds **`dogmaIM`** with
bindParams `(stationID, groupStation)` and the fused call
`GetAllInfo(primeCharacter, primeShip, primeStructure)` — `(True, True,
False)` on a first docked login. **[V]**

The returned object is read attribute-wise:

| attribute | shape | note |
|---|---|---|
| `.structureInfo` | falsy, or `{itemKey: row}` | |
| `.shipInfo` | falsy, or `{itemKey: row}` | |
| `.charInfo` | falsy, or **2-tuple** `(contentsDict, charBrain)` | unpacked as `_, charBrain = allInfo.charInfo` |
| `.shipModifiedCharAttribs` | falsy, or `ItemGetInfo`-shaped | |
| `.systemWideEffectsOnShip` | falsy, or opaque | |
| `.shipState` | **4-tuple** `(instanceCache, instanceFlagQuantityCache, wbData, heatStates)` | see below |

Two findings here that `docs/reference/bound-objects.md` §3.2 does not have:

- **`.shipState` is required, and it is what suppresses `ship.Board`.**
  `_MakeShipActive` only falls back to a `board` session change when
  `shipState is None` — so returning `None` makes the client fire a *nested*
  session change inside the first one. **[V]**
- **`charBrain` is a 4-tuple** `(version, charEffects, shipEffects,
  structureEffects)`; `(0, [], [], [])` is the safe empty answer. The effect
  elements are server-side objects with `.GetLiteralKey()` / `.modifierType`,
  which we cannot yet model — so keep the lists empty. **[V]**

### 2.2 `crimewatch.GetClientStates` — same wave, rigid shape

Bound on `crimewatch`, bindParams `(stationID, groupStation)`, fires because
`charid` is in the change. The reply is destructured immediately, so the
nesting is not negotiable:

```python
myCombatTimers, myEngagements, flaggedCharacters, _ = GetClientStates()
weaponTimerState, pvpTimerState, npcTimerState, criminalTimerState, disapprovalTimerState = myCombatTimers
criminals, suspects = flaggedCharacters
```

Minimal: `(((0, None), (0, None), (0, None), (0, None), (0, None)), [], ([], []), safetyLevel)`. **[V]**

### 2.2b `corpRegistry.GetAggressionSettings` — the gate that hides behind 2.2

The *same* `crimewatchSvc.ProcessSessionChange` has a **second, separate**
top-level branch:

```text
293 LOAD_CONST  'corpid'
296 LOAD_FAST   change
299 COMPARE_OP  6 (in)
302 POP_JUMP_IF_FALSE 318
308 LOAD_ATTR   RefreshCorpAggressionSettings
```

so it is not nested under the `charid`/`locationid` branch and it fires
because **our** session change carries `corpid`. Same wave, same write lock,
same class of gate as `dogmaIM.GetAllInfo`. Bound on a **bare scalar** corpID
— unlike `warRegistry` and `allianceRegistry` beside it, which take tuples.

The reply is an old-style instance of
`crimewatch.corp_aggression.settings.AggressionSettings`, state being two
single-underscore attributes. The module's own constants give the values, and
its own default picker — which ships in the client tree — says which to use:

```python
ALWAYS_ENABLED  = AggressionSettings(0, None)     # _enableAfter=0,    _disableAfter=None
ALWAYS_DISABLED = AggressionSettings(None, 0)     # _enableAfter=None, _disableAfter=0

def CreateDefaultForCorp(corpID):
    if not IsNPCCorporation(corpID): return ALWAYS_ENABLED
    else:                            return ALWAYS_DISABLED
```

Ours is an NPC corp, so `ALWAYS_DISABLED`. **[V]**

**A property of this wave worth knowing:** `NotifySessionChange` wraps each
listener in `try/except Exception: log.LogException()`, so a *wrong shape*
here is logged and swallowed and the wave carries on. Only silence hangs.
That makes a plausible wrong answer strictly better than no answer in this
wave, which is the general rule restated in one more place. **[V]**

Right behind it, on its own tasklet, `corp.OnSessionChanged` →
`__PrimeEveOwners` → `corpRegistry.GetEveOwners()`, again gated on `corpid`.
Rows are iterated, `.ownerID` read, and each is spliced into
`cfg.eveowners.data` as `list(owner) + [None]` — so a row has **four**
columns, `(ownerID, ownerName, typeID, gender)`, and the appended `None`
becomes the fifth, `ownerNameID`. Empty is safe. **[I]** on the arity: it is
inferred from that append, tightly but not directly. Also implemented.

### 2.3 `OnSessionChanged` — one uthread per service, order not guaranteed

**All implemented.** And one correction that changes how to read this whole
section: `viewStateSvc._OpenView` wraps `LoadView`/`ShowView`/`LoadCamera` in
a bare `except: log.LogException()`, and `ActivateView`'s `finally` calls
`loading.HideAllLoad()`. So a **wrong shape here is a logged traceback and an
empty view, never a stuck loading screen.** A stuck loading screen can only
be an unanswered blocking call — or `StartDependantServices`, which is the
one step not inside a try. **[V]**

#### …but that bare `except` covers `LoadView`, not `ActivateView`

Read the previous paragraph narrowly, because the first run to reach the
station proved the difference the hard way. The protection is around
`_OpenView`. `ActivateView` itself continues:

```python
if newInfo.GetViewType() == ViewType.Primary:
    self._PrepareBillboards(newInfo, oldInfo)
    self._OpenPrimaryView(newInfo, reopen=reopen, cameraID=cameraID, **kwargs)
else:
    self._OpenView(newInfo, reopen=reopen, cameraID=cameraID, **kwargs)
self.UpdateOverlays()                       # <-- NOT inside the bare except
...
if newInfo.GetViewType() == ViewType.Primary:
    sm.ScatterEvent('OnClientReady', newInfo.name)
self._NotifyOfViewStateChange(oldViewName, newViewName)
```

and the enclosing `try` catches only `eveexceptions.UserError`. So anything
raised by `UpdateOverlays` — which builds the neocom, and from there reaches
`corpRegistry.GetMyApplications` — leaves `ActivateView` through the
`finally` and **skips everything after it, including
`ScatterEvent('OnClientReady')`**. That is the event that wakes the rest of
the client.

The `finally` still runs `HideAllLoad()`, so the "never a stuck loading
screen" half of the claim survives. What does not survive is the idea that a
bad shape in this wave is always cosmetic. **[V]**

Two shapes that matter in the same breath, because this is the stage where a
drawn station is still an unfurnished one:

- **`corpRegistry.GetMyApplications()` → `{}`**, a dict keyed by
  corporationID with lists of applications as values. `len`, `in`,
  `__setitem__`, key iteration and subscript — and nothing caches a failure
  (`if self.myApplications is None`), so a bad answer re-issues on every
  neocom refresh. This is the one described above. **[V]**
- **`fwWarzoneSolarsystem.GetLocalOccupationState()` →
  `(session.solarsystemid2, None)`**. Unpacked as a 2-tuple before any test;
  then the first element is compared against the live session, and a mismatch
  costs a *second* round trip and then `raise RuntimeError`. So `(None, None)`
  and `(0, None)` are worse than useless. Returning the matching id is also
  what makes the call stop repeating — the client's own "unknown" constant is
  `(None, None)`, which can never match, so a capture showing this call three
  times is a cache that cannot fill, not a retry. Left wrong, it removes
  **every** info panel from the station screen, because
  `_GetCurrentlyAvailableInfoPanelClasses` is a comprehension calling
  `IsAvailable()` on each panel class and one throw aborts the lot. Fix
  `insurgencySolarsystem.GetAllVisibleCampaigns() → []` in the same commit,
  or the comprehension simply aborts one panel later. **[V]**

| call | kind | answer |
|---|---|---|
| `map.GetStationInfo()` | plain | `CRowset` with `['stationID','solarSystemID','ownerID','operationID']` — **capital S** — and `.Index('stationID')` is called on it. Cached in `uisvc` for the whole process, so one call per run. |
| `stationSvc.GetStationItemBits()` | plain | exactly **4 ints**: `(ownerID, itemID, operationID, stationTypeID)` |
| `invbroker` bind → `GetInventory(10004, None)` | bound | **another object reference** |
| `Inventory.List` | second-level bound | `flag=4` as a **keyword**; a plain list of item DBRows |
| `invbroker.GetItemDescriptor()` | plain | already implemented |
| `station.GetGuests()` | plain | `[]`; 4-tuples `(charID, corpID, allianceID, warFactionID)` |
| `officeManager.GetMyCorporationsOffices()` | plain | `[]`; rows with `.stationID`, `.officeID`, `.solarsystemID` (**lowercase s**) |
| `agentMgr.GetMyJournalDetails()` | plain | `[[], []]` — two lists; `.extend` is called on both |
| `agentMgr.GetAgents()` | plain | a **`Rowset` instance**, never a `CRowset` — see below |
| `warRegistry` bind → `GetWars(ownerID)` | bound | `{}` — a **dict**, and it recurs every 10 s; see below |

#### `GetStationItemBits`: three of the four values have teeth

`Row.__init__` raises unless the length is exactly 4, and then:

- **`itemID` must equal `session.stationid`.** `GetStationItem` re-issues the
  call whenever `_stationItem.itemID` differs, and `GetStationType()` is
  consulted on every hangar operation — so a wrong id is a synchronous call
  storm, not one wrong value.
- **`stationTypeID` must be a real FSD type.** `hangarRegistry.GetHangarType`
  calls `evetypes.GetCategoryID`/`GetRaceID` on it and `evetypes.GetType`
  raises `TypeNotFoundException` for an unknown one — so `0`, which is what
  the client itself substitutes when there is no station item, raises inside
  `SetUpModels` and the hangar never draws.
- **`operationID` must be a key in `stationOperations.fsdbinary`.**
  `get_station_services_for_operation` returns `None` for an unknown one and
  the caller does `set(...)` on it, so an invented value is a `TypeError` in
  the lobby window on every dock.

**The client ships the answers.** `bin64/staticdata/mapObjects.db` is TQ's own
`npcStations` table — 5210 rows of `stationID, solarSystemID, ownerID,
typeID, operationID, …`. Read the row for whatever station you dock into and
you are bit-identical to TQ by construction, with no inference at all:

```sql
select stationID, ownerID, typeID, operationID, solarSystemID
  from npcStations where stationID = 60012526;
-- 60012526 | 1000126 | 2502 | 33 | 30000001
```

That is the station the sandbox now uses. **Our first pick was inconsistent**:
station 60000004 in system 30000001, a combination that does not exist —
60000004 lives in 30002780. Nothing had complained, but the client holds this
table too and would eventually have disagreed with us about where we are.
Prefer a station that really is in the system you claim.

#### `GetAgents` must be a `Rowset`, not a `CRowset`

The caller's first act is `GetAgents().Clone()`, and **`CRowset` has no
`Clone`** — its method set is `Copy, GetColumnNames, columns, InsertNew,
Sort, Index, Filter, Rebuild, Unpack`. Only `Rowset`, `IndexRowset` and
`FilterRowset` have it. This is one of the few places the two rowset families
are *not* interchangeable, and the wrong choice is an `AttributeError` rather
than a shape error.

`Rowset` is old-style with no `__setstate__`, so the instance form is
`0x17` carrying just `header` and `lines`. **Do not send `RowClass`**: it has
a class-level default and `Clone()` reads `self.RowClass`.

Its columns are `agentID, agentTypeID, divisionID, corporationID, factionID,
level, stationID, solarsystemID` — note **all lowercase** `solarsystemID`,
where the station rowset above has `solarSystemID`. Both are read by name, so
the inconsistency is the client's and has to be matched per rowset. `quality`
and `isLocator` are carried too (**[I]** — TQ's historical columns, and the
Agency reads `isLocator`).

#### Also on this path, not previously listed

`hangarView`'s `updateCorruptionAndSuppressionStates` calls
`corruptionSuppressionSvc`, which does **not** use macho RPC at all — it goes
through `publicGatewaySvc`, a protobuf channel with a 10s timeout, in its own
tasklet. The realistic outcome is one timeout and a logged error. Harmless,
but it means the hangar view reaches for the public gateway, which the jail
blocks. **[V]**

And when the hangar finishes, `ScatterEvent('OnClientReady', 'hangar')` wakes
up the rest of the client — so expect a second wave of calls right after it
draws.

Then, pooled out of the blocking wave, `_MakeShipActive` →
`invbroker.GetInventoryFromId(shipID, 0)` → another reference →
`Inventory.List()` for the fitted modules. **[V]**

`List` **silently drops** rows whose length differs from the
`GetItemDescriptor` header, logging `'Invalid Item'`. **[V]**

#### `warRegistry.GetWars` is a dict, and it is not a one-shot

The consumer is easy to miss, because the name `warRegistry` never appears in
UI code: `grep -a` finds it only in `eveMoniker.pyc` and the marshal string
tables. It is `eve/client/script/ui/services/wars/war_cso.py`, `class Wars`,
`__guid__ = 'svc.war'`, `__servicename__ = 'war'` — one of
`crimewatchSvc.__dependencies__`, which is why it is already up at this point.

`eveMoniker.GetWar()` builds it on `(session.allianceid or session.corpid, 1)`
— for us the 2-tuple **`(corpid, 1)`**, since `allianceid` is `None` — and
`Wars.GetMoniker()` binds with `call=None`, so this is a **plain bind with no
fused call**. The `SetSessionCheck` is client-local and only raises if
`session.corpid` changes underneath it. **[V]**

**`GetWars(ownerID)` must be a mapping.** `PopulateCacheAndCleanupWars` calls
`wars.itervalues()` **twice**:

```python
self.warsByWarID.update({war.warID: war for war in wars.itervalues()})
warsByOwnerID = {war.warID for war in wars.itervalues()}
```

so `None`, `[]` and a tuple are all an `AttributeError`. It is *not* a rowset —
`.Index` and `.Clone` are never touched — and `{}` passes both comprehensions
and the `warsByOwnerID[ownerID][1]` read that follows. **[V]**

**It repeats every ten seconds, forever.** `Wars.Run` starts
`uthread.new(self.CheckForStartOrEndOfWar)`, which loops
`_CheckForWarStartingOrEnding()` / `SleepWallclock(10000)` for as long as
`session.charid` is set, and that method opens with
`self.GetWars(session.allianceid or session.corpid)`. So seeing it over and
over in a capture is normal and not a retry. A second trigger is the timers
HUD: `crimewatchTimers` → `IsPlayerCurrentlyAtWarOrInFW()` → `_IsOwnerAtWar`,
which calls `GetWars` on a cache miss. **[V]**

`Wars.OnSessionChanged` does **not** fire on first login, despite being gated
on `'allianceid' in change or 'corpid' in change` (ours has both): it bails on
`if 'charid' in change and change['charid'][0] is None: return`, and our
`charid` pair is exactly `(None, charID)`. **[V]**

**[I]** If real wars ever exist, each value is a `warUtil.War` namedtuple whose
17 fields, recovered from `co_consts` rather than from the decompiler, are:
`warID, declaredByID, againstID, timeDeclared, timeFinished, retracted,
timeStarted, retractedBy, billID, mutual, allies, createdFromWarID,
openForAllies, canBeRetracted, reasonEnded, warHQ, reasonStarted`. `allies` is
a dict, and `War.__new__` substitutes `{}` when it is missing.

Everything else on the bound object — `GetNegotiations`, `GetWarNegotiation`,
`CreateWarAllyOffer`, `AcceptSurrender`, `SetOpenForAllies` and the rest — is
reached only from the war or corp UI by explicit user action, so none needs an
arm yet.

#### 2.3b `standingMgr` — lazily started, and the strictest shape yet

This is what actually blocked the first run that reached the hangar. The
service is **not** a dependency of anything on this path: of the 303 service
classes in the tree, only `spaceView` and the resource-wars service name
`standing` in `__dependencies__`/`__startupdependencies__`. It comes up lazily,
the first time something calls `sm.GetService('standing')`, and on the docked
path that something is the station Lobby's Agents tab:

```python
def find_available_and_unavailable_agents(agents_in_station, ...):
    epic_arc_status_service = sm.RemoteSvc('epicArcStatus')
    standing_service = sm.GetService('standing')
```

unconditional, and *before* the per-agent loop — so it fires even though our
`agentMgr.GetAgents` rowset is empty. `ServiceManager.GetService` serialises
startup, so every other `GetService('standing')` on the path queues behind
whichever one got there first. **[V]**

Startup runs `Standing.Run` → `__RefreshStandings`, which issues three calls
back to back on one tasklet. `ProcessSessionChange` re-runs it whenever
`charid` or `corpid` changes, i.e. on every entry into the game.

| call | args | answer |
|---|---|---|
| `GetNPCNPCStandings()` | none | a **`Rowset` instance**, `header = ['fromID','toID','standing']`, `lines = []` |
| `GetCharStandings()` | none | `{}` |
| `GetCorpStandings()` | none | `{}` — **not issued** for an NPC-corp character |
| `GetStandingTransactions(fromID, toID)` | 2 | `[]` — off this path |
| `GetStandingCompositions(fromID, toID)` | 2 | `[]` — off this path |

`GetNPCNPCStandings` is the strictest empty-value caller in the project so far,
and a clean example of the fourth row of the table in
`docs/guides/debugging-the-client.md`:

```python
tmp = standingsMgr.GetNPCNPCStandings()
self.npcnpcstandingsto = tmp.Filter('toID')
self.npcnpcstandingsfrom = tmp.Filter('fromID')
```

`.Filter` is called twice, unconditionally, as the next two statements. There
is no truthiness test and no `isinstance` escape, so `None`, `{}`, `[]` **and**
the `(header, data)` prime tuple are all an `AttributeError`. It has to be a
real object. `Rowset.Filter` then builds `FilterRowset(self.header,
self.lines, colname)`, whose `__init__` does `header.index(idName)` — which is
why **both** key columns must be named even with no rows to key. The third
column, `standing`, is read as `standing.standing` in `GetStanding`.

`Filter('toID')` is pure overhead for us: `npcnpcstandingsto` is written in
`Run` and `__RefreshStandings` and **never read anywhere in this build**. The
call exists, so the column must. **[V]**

`GetCharStandings` is the one place in this whole document where a dict is
*proven* rather than inferred, because the caller says so:

```python
if not isinstance(self.npccharstandings, types.DictType):
    self.npccharstandings = self.npccharstandings.Index('fromID')
```

A dict skips the rowset path outright. **[I]** A non-empty answer would most
likely be a `CRowset` of `DBRow`s over `('fromID' I4, 'standing' R5)` — that is
the descriptor `Standing.Run` builds for the rows it splices in itself via
`_AppendNPCCharStanding`, and `IndexRowset`'s fast path requires matching
widths.

`GetCorpStandings` is never asked of our character, and the reason is worth
knowing before someone "fixes" its absence: the `uthread.parallel` branch runs
only when `not idCheckers.IsNPC(session.corpid)`, and `IsNPC` is
`9999 < id < 90000000`. Our corp is `1000001`, so the client sets
`npccorpstandings = {}` locally and never calls. **[V]**

##### The `raceID` trap, which fires *after* we answer correctly

Immediately after those calls, still in `__RefreshStandings`:

```python
for factionID in appConst.factions:
    if factionID not in self.npccharstandings and \
       factionID == appConst.factionByRace[session.raceID]:
```

`factionByRace[...]` is a **bare subscript** and the dict has exactly four
keys: `raceCaldari=1`, `raceMinmatar=2`, `raceAmarr=4`, `raceGallente=8`. With
our empty `npccharstandings` the first conjunct is true for at least one of the
31 factions, so the lookup *will* run. A `raceID` of `None`, or `16`
(`raceJove`), or anything else, is a `KeyError` that aborts the refresh after
we answered perfectly — a failure that would look like our shape was wrong.
Ours is `1`. **[V]**

##### Don't pre-implement `epicArcStatus`

`sm.RemoteSvc('epicArcStatus')` on the line above only builds the proxy. Its
one method, `AgentHasEpicMissionsForCharacter(agent_id)`, is inside the
per-agent loop, so with no agents in station it is never called. **[V]**

### 2.5 The capsule — what a ship actually costs

In EVE you are always in a ship or a capsule, and the client agrees: the
fitting window refuses to open with `CannotPerformActionWithoutShip`, reached
purely from `session.shipid` being `None`.

**The pod lives in the station hangar like any other ship.** TQ's own session
builder says so, walking *up* the location chain from the character item:

```python
if item.categoryID == const.categoryShip:
    updateDict['shipid'] = itemID
elif item.groupID == const.groupStation:
    updateDict['stationid'] = itemID
...
itemID = item.locationID
```

so "the character is in a pod, the pod is in the hangar" is literally how
`shipid` and `stationid` are derived. There is **no separate active-ship
list** anywhere in the client — every consumer reads the ordinary `flagHangar`
listing and picks the row whose `itemID == session.shipid`. **[V]**

`flagPilot` (57) is *not* this row's flag. It is applied client-side by
`_OnCharacterEmbarkation` to the **character** inside the pod, and
`characterDogmaItem.SetLocation` raises if it is anything else. **[V]**

| what | value | why it is not a free choice |
|---|---|---|
| `itemID` | `>= 100000000` | `IsSystemOrNPC` is `id < 90000000`, and `cfg.Recordset._Prime` routes such keys through the client's *local* sqlite fetcher before asking us. `IsPlayerItem` is `100000000 <= id < minFakeItem`. Do not reason from our charID 90000001 — that is a valid character id and specifically not a player item. |
| `typeID` | 670 | `typeCapsule`. Goes through FSD. |
| `groupID` / `categoryID` | 29 / 6 | `GetShipItemFromHangar` matches `categoryShip`, and it is what partitions the Ship Hangar from the Item Hangar. |
| `flagID` | 4 | `flagHangar`. See above. |
| `singleton` | **1** | `UpdateItem` drops a falsy singleton at `Godma:UpdateItem.skip3`, **silently**, and the pod never gets attributes. |
| `locationID` / `ownerID` | stationID / charID | Tested by `ProcessInventoryChange` and by every `IsItemHere`. |

#### `shipState` must be a real 4-tuple, and `None` is worse than wrong

`_MakeShipActive` does `UNPACK_SEQUENCE 4` unconditionally. But the branch
*above* it is the real reason:

```python
if shipState is None:
    shipState = sessionMgr.PerformSelectiveSessionChange(
        'board', 'board', False, self.remoteShipMgr.Board, shipID, session.shipid)
```

That is a remote call we do not implement, **inside** the
`uthread.Lock(self, 'makeShipActive')` critical section, and it mutates the
session (`sess.mutating += 1`, a `nextSessionChange` cooldown). It also buys
nothing: `PerformSessionChange` returns `func(*args)` verbatim, so `Board`'s
return value *is* this same tuple. Sending it in `GetAllInfo` is identical
data by a shorter route. **[V]**

The four slots, each matching the client's own `ClearInstanceCache`:
`({}, {}, None, {})` — `instanceCache`, `instanceFlagQuantityCache`,
`wbData` (`SetWeaponBanks` opens `if data is None: return`), and `heatStates`,
which is stored and never read back.

An empty `instanceCache` is not a shortcut: `GetInstance` rebuilds any miss
locally from godma, and `baseDogmaItem.Load` applies static type attributes
first and only overrides where the instance value is **non-zero** — so an
empty cache and a zero-filled one behave identically. **[V]**

#### `shipInfo` is required, not optional

`{itemKey: row}`, one entry for an empty pod, with `row.invItem` packed
against **our own `invbroker.GetItemDescriptor()` descriptor** — `godma.invitems`
is a rowset built on `invCache.GetItemHeader()`, which is that call.

Leaving it falsy costs two things:

- `godma.IsItemReady` is `itemID in self.invitems`, written only by
  `UpdateItem`, which only runs for what `shipInfo` primes. So
  `corvetteButton` spins a 10 Hz tasklet forever.
- `fittingUtil.GetTypeIDForController` gets `None` from `godma.GetItem`, feeds
  it to `evetypes.GetCategoryID`, and raises **`TypeNotFoundException`** out of
  `FittingWindow.ApplyAttributes`, which is not wrapped. **[V]**

#### `GetSelfInvItem` has to be per bound object

A correction to this project's own earlier note. It claimed a container keyed
on a real item id "takes the shortcut" in `invCacheContainer.__init__` and
never asks us. **That is true of `.itemID` only.** `.typeID` and `.item` also
call `_SetItemData`, whose body is

```python
if self._item is None and self.moniker is not None:
    self._item = self.moniker.GetSelfInvItem()
```

and `self._item` starts `None` for *every* container. So the capsule's own
inventory asks too, and with one shared answer it was told it was a station —
its dogma item would have been built with `typeID 2502`, and
`GetClassForItem` would have picked structure semantics instead of
`ShipDogmaItem`. Inventory objects are therefore minted with the arguments of
the `GetInventory`/`GetInventoryFromId` call that produced them, not with the
`invbroker` bind params they all share. **[V]**

#### Two new calls land in the draw path, and they are on the dangerous side

`baseHangarBehaviours.LoadShipModel` runs as soon as a ship exists:

| call | kind | answer |
|---|---|---|
| `ship.GetDirtTimestamp(itemID)` | bound on `(stationID, groupStation)` | a win64 **filetime** — `CalcDirtLevelFromAge` does `blue.os.TimeDiffInMs(ts, now)` with no `None` guard |
| `shipKillCounter.GetItemKillCountPlayer(itemID)` | plain | `0` |

**Both are synchronous.** `hangarView.LoadCamera` calls
`ReplaceExistingShipModel` directly rather than on a tasklet, and `_OpenView`
runs `LoadView` / `ShowView` / `LoadCamera` together — so an *unanswered* one
stalls `_OpenView`, `ActivateView`'s `finally: HideAllLoad()` never runs, and
`OnClientReady` never fires: a stuck loading screen. A *wrong* one is caught
by that function's bare `except` and merely logs. §1's corollary at its
sharpest.

`cosmeticsSvc.GetAppliedSkinState` looks like a third but is **not a macho
call at all** — `sm.GetService`, not `sm.RemoteSvc`, double-guarded by
`IsPlayerItem` and a blanket `except Exception`, and it goes out over the
protobuf gateway the jail blocks. One Info-level log line. Nothing to do.

#### And one that is not obviously inventory at all

`config.GetMultiLocationsEx([capsuleID])` must return a row. `cfg.evelocations`
is a `cfg.Recordset`, and `Recordset.Get` **raises `KeyError('RecordNotFound')`
on a miss** — unlike the prime caches elsewhere, where an omitted key is simply
remembered in `knownLuzers` and never asked for again. The fitting window's
`ConstructLayout` ends in `UpdateShipName` → `GetFitName` →
`cfg.evelocations.Get(itemID)`, and `invCtrl.ShipCargo.GetName` does the same
with no guard. **[V]**

### 2.6 The fitting-window wave — what the capsule unlocked **[V]**

Giving the character a ship opened a second front. These arrive once the
hangar has drawn and the player opens the fitting window, and all of them are
observed on the wire, not inferred.

| call | answer | why that shape |
|---|---|---|
| `fighterMgr.GetFightersForShip()` | `([], [], {})` | unpacked with no guard; third element gets `.iteritems()` |
| `charFittingMgr.GetFittings(charID)` | `{}` | the client **mutates** it: `del fittings[id]`, `fittings[id] = fitting` |
| `corpFittingMgr.GetFittings(corpID)` | `{}` | same client code path, chosen by ownerID |
| `corpFittingMgr.GetCommunityFittings()` | `{}` | stored in the same dict under corp `1000282` |
| `notificationMgr.GetAllNotifications(fromID=…)` | `[]` | **kwarg, not positional** |
| `shipCosmeticsMgr.GetEnabledCosmetics(shipID)` | `{}` | `{packedSlot: cosmeticType}` |
| `invbroker::inventory.GetAvailableTurretSlots()` | `0` | a capsule has no hardpoints |
| `crimewatch.GetMySecurityStatus()` | `0.0` | **a float**, see below |
| `storeManager.get_offers(4)` | truthy, self-filtering | **empty latches the store**, see below |

Three things here are worth carrying forward.

**`fighterMgr` is triggered by the capsule itself.**
`ShipFighterState.OnSessionChanged` runs on any `shipid` change, so the call
did not exist until we gave the character a ship. Expect the same pattern
again: each thing we grant the character wakes a subsystem that was dormant.

**`GetMySecurityStatus` must be a float, and `None` was never tolerated.**
Python 2 orders `None` below every number, so `None <= -4.5` is `True` and
the client had concluded our brand-new character was a -4.5 outlaw. It never
threw. `baseDogmaLocation.GetCharacterSecurityStatus` is literally
`return 0.0`, and the value is written into the dogma attribute
`attributePilotSecurityStatus` whenever a pilot is installed in a hull —
which, for us, is every capsule boarding.

**The store latches.** `vgsClient.get_offers` sets `self.available = False`
on any falsy answer, and every later store call then raises client-side
without reaching us. Answer a truthy list whose single offer carries no
`products`: the client's own comprehension drops it, and the PLEX vault takes
its authored "no price" branch.

**Still unimplemented, deliberately:** the fitting *mutators*
(`SaveFitting`, `UpdateFitting`, `DeleteFitting`, `SaveManyFittings`). We have
nowhere to persist a fitting, and `fittingSvc.PersistFitting` acts only
`if fittingID > 0`, so an unanswered save reads as "the save failed" — which
is true. Note also that saving a *capsule* never reaches the wire at all:
`VerifyFitting` raises `UserError('ParseFittingFittingDataEmpty')`
client-side when `fitData` is empty, and a pod has nothing fitted.

## 2.4 Observed on the wire, beyond this document's first draft **[V]**

The first run to get past the session change named these. They are not in the
recon above because the recon traced the handler waves, and these come from
elsewhere in the client — which is the useful reminder that the waves are not
the whole story.

| call | status |
|---|---|
| `warRegistry` bind → `GetWars(ownerID)` | **implemented** — and it is what forced `handle_bound_call` into existence |
| `air_npe.get_air_npe_state()` | not yet |
| `clientStatLogger.LogString(str)` | not yet; seen carrying "Failed to get a valid ballpark in time after try..." |
| `operationsManager.can_character_play_the_tutorial(charID)` | not yet |
| `skillMgr2.GetMySkillHandler()` | not yet; the name suggests an object rather than data |
| `calendarProxy.GetEventList(10, 2026)` | not yet |

Two details from that run worth keeping:

- `dogmaIM`'s bindParams are `(60000004, **15**)`, so `groupStation` is 15.
- the fused call arrived as `GetAllInfo(True, None, None)`, not
  `(True, True, False)` — because `session.shipid` and `session.structureid`
  are both absent, which is our own doing and confirms the `shipid` choice in
  §1 propagated as intended.

## 3. What will hang first, ranked

1. **`dogmaIM` bind → `GetAllInfo`.** First call after the session change, in
   the blocking wave, guaranteed first by `FavourMe`. Unanswered, the
   session-change tasklet never finishes, so the session **write lock** is
   never released and every later notification stalls behind it. This single
   call is the gate.
2. **`crimewatch.GetClientStates`.** Same wave, same lock, same symptom — but
   a *wrong shape* raises instead of hanging, which is the better failure.
2b. **`corpRegistry.GetAggressionSettings`.** The back half of the same
   handler as 2, in a separate branch gated on `corpid` (§2.2b). Easy to miss
   precisely because it is in a function you have already read.
3. **`invbroker` bind → `GetInventory` → `List`**, plus
   `stationSvc.GetStationItemBits` and `map.GetStationInfo`. The session change
   completes but the hangar never draws: the loading screen stays up.
4. `officeManager`, `station.GetGuests`, `agentMgr.*` — hang the lobby window
   and the Agents tab, not the transition.
5. `warRegistry.GetWars` — cosmetic, and silent.

Non-hang hazards, all of which produce a traceback rather than silence:
`.shipState` missing → `AttributeError`; `.charInfo` not a 2-tuple →
`ValueError`; `charBrain` not a 4-tuple → `ValueError` later in
`ApplyBrainEffects`; a change dict that matches the current session → total
silence.

## 4. Corrections to `docs/reference/bound-objects.md`

- **A 30th moniker service it missed: `officeManager`.** It builds
  `Moniker(REMOTESVC, stationID)` from a *module-level constant*, so a
  literal-argument scan cannot see it. Worth re-running `tools/scan_bound.py`
  with constant propagation — there may be more. **[V]**
- `GetAllInfo` returns `.shipState`, undocumented there (§2.1).
- `.charInfo` is `(contentsDict, charBrain)`, and `charBrain` is a 4-tuple.
- `crimewatch.GetClientStates`'s exact nesting (§2.2); the doc lists the
  method but no shape.
- `warRegistry` is on the ordinary docked-login path, not only in corp UI.
- Resolve is a **separate round trip** before the bind, worth stating next to
  §1.3. Also: a client's own `machoNet.GetNodeID()` is always `None`, so the
  "bind locally" branch in `Moniker.Bind` is unreachable and our node id
  cannot collide with it. **[V]**

## 5. Not traced: the undocked variant

`_HandleSessionChangeInSpace` instead of the station path, and these wake up:
a second `crimewatch.GetClientStates`, `michelle`/`Park` → bound `beyonce`
with a **scalar** `solarsystemID` as bindParams and a polled
`UpdateStateRequest()`, `fleetSvc`, `targetMgr` → `godma.RefreshTargets`,
`abyss`, `sov`, `faction`, `fwWarzoneSvc`. That is M3 territory (Destiny) and
is deliberately left for later.
