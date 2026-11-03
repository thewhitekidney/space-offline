# A stargate jump, end to end

Implementation spec for the first inter-system jump and, with it, the server's
first second solar system. Read off the V24.01 / 3619879 client's own bytecode
(`out/pyc/`, real `co_lnotab` lines via `tools/dis.py`), the shipped static
data (`tools/fsd.py`), and `_destiny.dll`'s string table.

**Evidence markers.** `[V]` verified against this client's bytecode or shipped
data. `[I]` inferred — a deduction from `[V]` facts, stated with what would
falsify it. `[A]` an assumption we choose to ship, with its falsification.
`[V2]` a third-party decode of a different build. There is no `[V2]` in this
document: no third-party gate-jump implementation was available, and
`docs/archive/evejs-261018-reference.md` has nothing on stargates.

Nothing here has been run against a client. Everything marked `[V]` is a read;
everything marked `[I]` is a chain of reads; §12 lists what a relaunch settles.

---

## 1. Minimum viable jump — the ordered exchange

Two characters: **Alice** jumps 30000001 → 30000003 through gate 50000056.
**Bob** stays in 30000001. **Carol** is already in 30000003.

Preconditions the server must already satisfy, or the jump cannot even be
*initiated* (§2.4):

- The destination gate's ball and slim item exist in Alice's park, and the
  slim item carries **`jumps = [row(toCelestialID=50000342, locationID=30000003)]`** `[V]`.
- Alice is within **2500 m surface distance** of the gate (`const.maxStargateJumpingDistance`) `[V]`.
- Alice's last session change was more than **10 s** ago (`SESSIONCHANGEDELAY`) `[V]`.

| # | direction | message | notes |
|---|---|---|---|
| 1 | C → S | `beyonce` bound-object call **`CmdStargateJump(50000056, 50000342, <Alice shipID>)`**, blocking | §2. Object id is the one minted at `beyonce.MachoBindObject(30000001, None)`; prefix `N=` so it arrives on the existing `handle_bound_call` path |
| 2 | S → C | `DoDestinyUpdate` to **Alice + Bob** (everyone in 30000001): one action `('OnSpecialFX', (shipID, None, None, 50000056, None, 'effects.JumpOut', False, True, True, -1, None, None, 0, (30000003,)))` | §3.1. Alice gets the tunnel; Bob gets `StartJumpForAudience`, which hides her model and plays the departure effect |
| 3 | S → C | `DoDestinyUpdate` to **Alice + Bob**: `('RemoveBalls', ([<Alice shipID>],))` | §3.2, §6. For Bob this is her leaving the grid. **For Alice this is load-bearing**, not cosmetic: it is what makes step 5 take the branch that rebuilds the ballpark |
| 4 | S → C | **`SessionChangeNotification`** (command 16), change = `{'solarsystemid': (30000001, 30000003), 'solarsystemid2': (30000001, 30000003), 'constellationid': (20000001, 20000001), 'regionid': (10000001, 10000001)}` | §3.3. `locationid` is **derived client-side** and must not be sent. `constellationid`/`regionid` are **not** derived and must be sent whenever they change |
| 5 | S → C | `CallRsp` for step 1. Return value is discarded by every caller | §2.3 |
| — | client | `subwaySvc.OnSessionChanged` sees `locationid` in the change and `InJump()` → swaps the scene to 30000003, `FinishJump()`. `gameui._HandleSessionChangeInSpace` finds `shipid not in bp.balls` → `wannaBeEgo = shipid`, `ChangePrimaryView('inflight')` → `OnPrimaryViewChanged` → `michelle.UpdateBallpark` → `RemoveBallpark()` (old park released, old `beyonce` object **unbound**) → `AddBallpark(30000003)` | §3.4 |
| 6 | C → S | `beyonce.GetFormations()` — answer an **empty tuple `()`**, never a list | `[V]`, same as the undock |
| 7 | C → S | `beyonce.MachoResolveObject(30000003)` — bare long, not a tuple | `[V]` |
| 8 | C → S | `beyonce.MachoBindObject(30000003, None)` → `(object, None)` | `[V]`. **This is the arrival trigger**: `dispatch::pending_for_bound_call` already fires `destiny::on_ballpark_bind` here |
| 9 | S → C | `DoDestinyUpdate` to **Alice**: `('SetState', (bag,))` — the 30000003 bootstrap: Alice's ball at the destination gate, Carol's ball, every gate in 30000003, no station | §5, §7 |
| 10 | S → C | `DoDestinyUpdate` to **Carol**: `('AddBalls2', ((blob, extra),))` with Alice's ball | §6 |
| 11 | S → C | `DoDestinyUpdate` to **Alice + Carol**: `('OnSpecialFX', (shipID, None, None, 50000342, None, 'effects.JumpIn', …))` | §5.3, `[I]` |
| 12 | C → S | `crimewatch.MachoResolveObject((30000003, 5))` + `MachoBindObject(…, ('GetClientStates', …))` → 4-tuple | §8.1 `[V]` — fired synchronously from `crimewatchSvc.OnSessionChanged` |
| 13 | C → S | `dogmaIM`, `invbroker`, `ship` resolve + bind with `(30000003, 5)` as they are next needed | §8.1 `[V]` |

Session keys that move, and only these: **`solarsystemid`**, **`solarsystemid2`**,
and `constellationid` / `regionid` when the jump crosses one. `stationid` stays
`None`. **`shipid` must not be in the change** — same trap as the dock
(`service.rs:2220`): `invCache.ProcessSessionChange` has a branch keyed on
`shipid` that returns early and skips cache invalidation `[V]`.

---

## 2. The call

### 2.1 Service, method, arguments

**`CmdStargateJump(stargateID, beaconID, shipID)`** on the **bound `beyonce`
ballpark object** — not on the `beyonce` service, not on `michelle`. Three
positional arguments, all integers:

| arg | meaning | source on the client |
|---|---|---|
| `stargateID` | the **origin** gate's itemID, e.g. 50000056 | the right-clicked / double-clicked item |
| `beaconID` | the **destination** gate's itemID, e.g. 50000342 | `slimItem.jumps[0].toCelestialID` |
| `shipID` | `session.shipid` | the session |

`[V]`, two independent call sites:

- `out/pyc/stargate/client/gateJumpSvc.pyc`, `JumpThroughGate` **line 131**:
  ```python
  self.sessionMgr.PerformSessionChange(
      localization.GetByLabel('UI/Inflight/Jump'),
      remoteBallpark.CmdStargateJump, stargateID, beaconID, session.shipid)
  ```
  `remoteBallpark` is `self.michelle.GetRemotePark()` (line 99), which is
  `michelle.__bp.remoteBallpark` — the bound object itself, no wrapper
  (`michelle.py:511-516`).
- `out/pyc/eve/client/script/parklife/autopilot.pyc`, `Update` **lines 445-450**:
  ```python
  sm.GetService('sessionMgr').PerformSessionChange(
      'autopilot',
      sm.GetService('michelle').GetRemotePark().CmdStargateJump,
      destID, theJump.toCelestialID, session.shipid)
  ```

Signature `JumpThroughGate(self, stargateID, typeID, beaconID, solarSystemID)`,
`argcount=5`, `out/pyc/stargate/client/gateJumpSvc.pyc` line 95 `[V]`.

### 2.2 The client-side call sites, in full

```
double-click or right-click a gate
  movementFunctions.DockOrJumpOrActivateGate(itemID)         movementFunctions.py:393
    item = bp.GetInvItem(itemID)                             michelle.py:2121 -> slimItems.get(id)
    if item.groupID == const.groupStargate:                  groupStargate = 10
        slimItem = bp.slimItems.get(itemID)
        jump = slimItem.jumps[0]                             <-- IndexError if we omit `jumps`
        menuSvc.StargateJump(itemID, item.typeID,
                             jump.toCelestialID, jump.locationID)   movementFunctions.py:413
  menusvc.StargateJump(stargateID, typeID, beaconID, solarSystemID) menusvc.py:3535
    GetCloseAndTryCommand(stargateID, gatejump.JumpThroughGate,
                          (stargateID, typeID, beaconID, solarSystemID),
                          interactionRange=const.maxStargateJumpingDistance)  menusvc.py:3541
  gatejump.JumpThroughGate(...)                              gateJumpSvc.py:95
    sessionMgr.PerformSessionChange('<UI/Inflight/Jump>',
                                    remoteBallpark.CmdStargateJump,
                                    stargateID, beaconID, session.shipid)     gateJumpSvc.py:131
```

All line numbers from `tools/dis.py`, i.e. the client's own `co_lnotab` `[V]`.

`GetCloseAndTryCommand` with `interactionRange` is the "approach first, then
run the command" wrapper — the client closes to within 2500 m by itself using
ordinary `Cmd*` movement before the jump call goes out `[V]`.

### 2.3 The reply is discarded; the call is still blocking

`PerformSessionChange` returns `func(*args, **kw)`'s value
(`sessions.py:522`, `CALL_FUNCTION_VAR_KW` then `RETURN_VALUE`), and both call
sites `POP_TOP` it `[V]`. So the return value is free — but the call is a
synchronous `CallReq` and the client's tasklet is parked inside it, exactly
like `ship.MachoBindObject(('Undock', …))`. That is the precedent that makes
step 4 safe: notifications are processed by the machoNet reader while a call
is outstanding — proven on the wire at the undock, where
`InitializeRemoteBallpark` issued calls `[326]`/`[328]` while
`GetFormations` was still open `[V]`.

### 2.4 What makes the call fail before it is made

`JumpThroughGate` returns early, with no server call, if any of these hold `[V]`:

| line | condition |
|---|---|
| 96 | `beaconID is None` |
| 100 | `michelle.GetRemotePark()` is `None` |
| 104 | `check_cancel_stargate_jump(stargateID, solarSystemID)` is true (`stargate/client/jump_checks.pyc`) |
| 108 | `self._canUseGate(stargateID, typeID)` is false — only when the type has an alignment-based toll space component |
| 111 | `CheckForGateRestriction(session.solarsystemid, stargateID)` — an emanation lock on that gate |
| 120 | destination in `self._lockController.get_restricted_systems()` — prompts, does not block |

`stargate/client/gateLockController.pyc` and `gateLockMessenger.pyc` reach the
**protobuf public gateway**, which we do not have (`port-tracker.md` §10.4).
`get_current_system_lock()` returning `None` is the benign path and is what an
absent gateway gives us `[I]`.

The autopilot adds `_IsSessionAndBpSafeToJump(bp, ship)`
(`autopilot.py:505-523`), all five of which the server can break `[V]`:

```python
if ship.isCloaked:                                              return False   # :507
if session.mutating:                                            return False   # :511
if session.changing:                                            return False   # :514
if bp.solarsystemID != session.solarsystemid:                   return False   # :517
if michelle.GetRemotePark()._Moniker__bindParams != session.solarsystemid:
                                                                return False   # :520
```

The last two are the client telling us, in its own code, that **after a jump
the ballpark and the `beyonce` moniker must both be on the new system** — the
single strongest piece of evidence that a jump is a park teardown and rebind
and not a content swap.

### 2.5 Gate jump vs jump drive vs wormhole vs Upwell gate — the client separates all four

| mechanism | call | evidence |
|---|---|---|
| **stargate** | bound `beyonce.CmdStargateJump(gateID, destGateID, shipID)` | `gateJumpSvc.py:131` `[V]` |
| **Upwell jump gate** (`groupUpwellJumpGate` = 1408) | service `structureJumpBridgeMgr.CmdJumpThroughStructureStargate(gateID)` | `autopilot.py:430-433` `[V]` |
| **jump drive / cyno** | `eve/client/script/ui/services/jumpQueue.pyc`, `capitalnavigation.pyc`, `cynoTravelSvc.pyc`; effect guids `effects.JumpDriveOut`, `effects.JumpDriveOutBO` | `[V]` |
| **wormhole** | `menuSvc.EnterWormhole(itemID)` for `groupWormhole` = 988; effect `effects.JumpOutWormhole` | `movementFunctions.py:428-429` `[V]` |
| **jump bridge** | `jumpbridge/client/jumpBridge.pyc`; effect `effects.JumpBridgeOut` | `[V]` |
| **abyssal / Triglavian** | `menuSvc.ActivateAbyssal*Gate`; effects `effects.JumpOutAbyssal{,Between}` | `[V]` |

The eight travel-effect guids are the keys of
`res:/staticdata/traveleffects.fsdbinary` (8 rows, 2284 bytes). Read out of the
resource file's string pool `[V]`:

```
effects.JumpOut                 effects.JumpBridgeOut
effects.JumpDriveOut            effects.JumpDriveOutBO
effects.JumpOutAbyssal          effects.JumpOutAbyssalBetween
effects.JumpOutEventB           effects.JumpShipCaster
```

Only **`effects.JumpOut`** is the stargate one. The table also carries a `type`
column with values `BetweenSystems` / `WithinSystem`
(`fsdBuiltData/client/travelEffects.pyc:74 IsJumpWithinSystem`) `[V]`.

---

## 3. The transition sequence

### 3.1 The departure effect comes first, and it is a Destiny action

The client's jump cinematic is **not** driven by the session change. It is
driven by an `OnSpecialFX` Destiny action whose `guid` is a travel effect.

`out/pyc/eve/client/script/parklife/subwaySvc.pyc`, `OnSpecialFX` lines 450-475 `[V]`:

```python
def OnSpecialFX(self, shipID, moduleID, moduleTypeID, targetID, otherTypeID,
                guid, isOffensive, start, active, duration, repeat, startTime,
                timeFromStart, graphicInfo):
    if self.ENABLED and IsTravelEffect(guid):               # :452
        destinationSystem = None
        if graphicInfo:
            destinationSystem = graphicInfo[0]              # :455
        guid = self.JUMP_EFFECT_OVERRIDE or guid            # :458
        if shipID == session.shipid:                        # :459  <-- the jumper
            self.shipID         = shipID
            self.gateID         = targetID                  # :461
            self.originSystemID = session.solarsystemid     # :462
            self.destSystemID   = destinationSystem         # :463
            try:
                self.StartJump(shipID, targetID, guid, destinationSystem)   # :465
            except Exception as e:
                log.exception(...); PostJumpServiceCalls(self.gateID); self.Cleanup()
            sm.GetService('space').PrioritizeLoadingForIDs(
                GetPriorityLoads(self.gateID))              # :470
        else:                                               # :472  <-- everyone else
            self.StartJumpForAudience(shipID, guid, destinationSystem)      # :473
```

So the server must send, as one Destiny action in a batch addressed to
**everyone in the origin system including the jumper**:

```
('OnSpecialFX', (shipID, moduleID, moduleTypeID, targetID, otherTypeID, guid,
                 isOffensive, start, active, duration, repeat, startTime,
                 timeFromStart, graphicInfo))
```

Fourteen positional arguments, verified from the `argcount`/`varnames` of both
`Park.OnSpecialFX` (`michelle.py:2087`, argcount 15 incl. `self`) and
`subwaySvc.OnSpecialFX` (`subwaySvc.py:472`, argcount 15) `[V]`. Note there is
**no `area` argument** — `docs/archive/destiny-and-the-ballpark.md` §4.2's prose mentions
one and is wrong.

Values that matter:

| arg | value for a gate jump | why |
|---|---|---|
| `shipID` | the jumper's ship id | `== session.shipid` selects jumper vs audience `[V]` |
| `targetID` | the **origin** gate's itemID | stored as `self.gateID`, and `IsStargate(targetID)` gates the audio stop (`subwaySvc.py:602`) `[V]` |
| `guid` | `'effects.JumpOut'` | `[V]` |
| `graphicInfo` | a sequence whose `[0]` is the **destination solar system id** | `subwaySvc.py:477` `[V]` |
| `moduleID` | `None` | `Park.OnSpecialFX:2088` iterates it when it is `collections.Iterable` and scatters one event per element; a scalar or `None` scatters one `[V]` |
| the rest | `None` / `0` / booleans | not read on this path `[V]` |

`Park.OnSpecialFX` is in `nonDestinyCriticalFunctions` (`michelle.py:1048-1057`),
so it is dispatched **without** `SynchroniseToSimulationTime` — its stamp costs
no `Evolve` `[V]`.

`fxSequencer.OnSpecialFX:480` short-circuits: `if IsTravelEffect(guid) and
sm.GetService('subway').Enabled(): return` `[V]`. So in V24.01 the subway
service owns the jump cinematic and `eve/client/script/environment/effects/Jump.pyc`'s
`JumpOut`/`JumpIn` classes are the legacy fallback.

`PreJumpServiceCalls(gateID, destinationID)` (`subwaySvc.py:60`) is purely
local: `ScatterEvent('OnJumpStarted')`, `flightControls.Stop()`, fade the UI.
No server call `[V]`.

### 3.2 The ballpark is torn down and a new one bound — not a content swap

This is the headline and it decides the shape of the server's jump.

**`michelle.UpdateBallpark`, lines 215-234, is the only caller of
`RemoveBallpark` on the jump path** `[V]`:

```python
215  def UpdateBallpark(self):
216      if session.solarsystemid and self.viewState.IsPrimaryViewActive('inflight', 'structure'):
217          if self.__bp and self.__bp.solarsystemID != session.solarsystemid:
218              self.RemoveBallpark()
219          if self.__bp is None:
223              sm.GetService('space')
224              self.AddBallpark(session.solarsystemid)
225          sm.ScatterEvent('OnEnterSpace')
226      else:
227          self.RemoveBallpark()
231          if self.__bp is None:
232              sm.GetService('sceneManager').UnregisterScene('default')
233              sm.GetService('sceneManager').CleanupSpaceResources()
234              blue.recycler.Clear()
```

Line 217 is the jump: **the ballpark whose `solarsystemID` disagrees with the
session is destroyed**, and line 219 builds a fresh one for the new system.
A whole-tree grep finds `UpdateBallpark` referenced in `michelle.pyc` only, and
`RemoveBallpark` in `michelle.pyc` plus `transitions.pyc` (character creation)
`[V]`.

`RemoveBallpark` (`michelle.py:466`) → `Park.Release()` (`:1177`), which:

- waits up to **10 s** for `subway.OkToDumpBallPark()` while `subway.InJump()`
  — i.e. until the tunnel effect has left the source scene (`:1188-1193`) `[V]`;
- `RemoveBalls(self.balls.iterkeys(), isRelease=True)` (`:1196`);
- `_parent_ClearAll()` (`:1199`), `FlushSimulationHistory(newBaseSnapshot=False)` (`:1200`);
- `_CleanUpRemoteBallpark()` (`:1213`) → kills the init tasklet and
  **`self.remoteBallpark.Unbind()`** (`:1174`);
- resets `validState = False`, `history = []`, **`latestSetStateTime = 0`** (`:1221`).

That `subway` wait is positive evidence that `Release` runs *during* a jump:
the only reason to block a park teardown on a jump animation is that the two
coincide `[V]`.

A second, independent witness: `subwaySvc.StartJump:579-584` locks the ship's
and the gate's `spaceObject`s from release —

```python
557  self.lockedBalls = []
558  for ballID in [targetID, shipID]:
559      ball = GetBall(ballID)
560      if ball and hasattr(ball, 'LockFromRelease'):
561          self.lockedBalls.append(ball)
562          ball.LockFromRelease()
```

and `spaceObject._Release:818-825` returns without tearing down any model when
`lockedFromRelease` is set `[V]`. That lock exists **because the balls are
released from the park mid-jump** and the tunnel still has to draw them.

**Therefore: one ballpark per solar system, created on arrival and destroyed on
departure; a new `beyonce` resolve + bind every time.** Not one continuous
ballpark whose contents are replaced. `[V]`

### 3.3 The session change, and exactly which keys move

`SessionChangeNotification`, command 16, `change = (flag, {attr: (old, new)})` —
the same envelope the undock already uses.

**Send:**

| key | value | why |
|---|---|---|
| `solarsystemid` | `(30000001, 30000003)` | `gameui.OnSessionChanged`'s `relevantSessionAttributes` (`gameui.py:555`) `[V]`; `michelle.UpdateBallpark:216-217` `[V]`; every location moniker's session check `[V]` |
| `solarsystemid2` | `(30000001, 30000003)` | `gateJumpSvc._IsSolarSystemTransition` is literally `'solarsystemid2' in change` (`gateJumpSvc.py:61`) `[V]`; local chat keys its channel on `session.solarsystemid2` `[V]` |
| `constellationid` | when it changes | not derived — see below |
| `regionid` | when it changes | not derived |

**Do not send:**

- **`locationid`.** `basesession.RecalculateDependantAttributesWithChanges`
  (`basesession.py:2183-2189`) sets `changes['locationid'] = (self.locationid,
  changes.get('stationid', [None,None])[1] or changes.get('solarsystemid',
  [None,None])[1])` whenever `stationid` or `solarsystemid` is in the change
  `[V]`. The client *adds* the key for us — which is how `subwaySvc.OnSessionChanged`
  (`:478`, `if 'locationid' in change`) sees it. Our undock already demonstrates
  this: we send two keys and the client logs three.
- **`shipid`.** §1.
- **`stationid`.** It is already `None` and absent.

The same function recalculates only `locationid` and the `corprole` family
(`:2191`, over `('hqID','baseID','rolesAtAll','rolesAtHQ','rolesAtBase',
'rolesAtOther','stationid','solarsystemid')`) `[V]`. **`solarsystemid2`,
`constellationid` and `regionid` are not in it** — the server owns them.

The full eve session attribute set that can be indexed is
`eveSessions.eveSessionsByAttribute` (`eveSessions.py:25-39`): `regionid`,
`constellationid`, `corpid`, `fleetid`, `wingid`, `squadid`, `shipid`,
`stationid`, `structureid`, `locationid`, `solarsystemid`, `solarsystemid2`,
`allianceid`, `warfactionid` `[V]`.

#### The 10-second session-change timer is client-enforced

`sessions.SessionMgr.PerformSessionChange` (`sessions.py:451`), on the
`machobase.mode == 'client'` branch `[V]`:

- `sessionChangeDelay = SESSIONCHANGEDELAY = 10 * const.SEC`
  (`basesession.py:43`, `sessions.py:1147`) `[V]`;
- pre-op it sets `sess.nextSessionChange = blue.os.GetSimTime() + sessionChangeDelay` (`:516`);
- if `sess.nextSessionChange > GetSimTime()` when a new change is attempted, it
  logs *"Too frequent session change attempts. You'll just get yourself stuck
  doing this. Ignoring."* (`:485`) and then either returns `None` — if the
  reason is in `sessionChangeShortCircuitReasons` — or raises
  `UserError('PerformSessionChangeInProgress')` (`:489`, `__RaisePSCIP:419`).

`eveSessions.SessionMgr.__init__:186` sets
`sessionChangeShortCircuitReasons = ['autopilot']` `[V]`. The manual jump's
reason is `localization.GetByLabel('UI/Inflight/Jump')`, which is **not** in
that list — so a manual jump inside 10 s of the previous session change raises
in the player's face, and an autopilot jump is silently skipped and retried.

**This is entirely client-side state.** The server neither sets nor sees
`nextSessionChange`; it is set from the client's own `GetSimTime`. The only
server lever is `DoSimClockRebase`, which adjusts it by the delta
(`sessions.py:661-665`) `[V]`. So the server does not need a jump cooldown to
match — but it will *see* a client that refuses to jump for 10 s after
undocking, and that is correct behaviour, not a bug.

#### The jump delay is 4000 ms, and the number is in our own client

`out/pyc/ballparkCommon/jumpdelay.pyc` — **server-shared code that ships with
the client** (rule zero) `[V]`:

```python
5   ABYSSAL_JUMP_DELAY_MS = 4000
8   def GetStargateJumpDelayMilliseconds(solarSystemID):
9       if IsTriglavianSystem(solarSystemID):
10          return ABYSSAL_JUMP_DELAY_MS
11      return invulnerabilityJumping                 # = 4000, appConst
14  def GetStargateJumpDelayBlueTime(solarSystemID):
15      return GetStargateJumpDelayMilliseconds(solarSystemID) * gametime.MSEC
```

`const.invulnerabilityJumping = 4000` `[V]` (read out of
`out/pyc/eve/common/lib/appConst.pyc`; the whole family is
`invulnerabilityDocking 5500`, `invulnerabilityJumping 4000`,
`invulnerabilityRestoring 60000`, `invulnerabilityUndocking 30000`,
`invulnerabilityWarpingIn 10000`, `invulnerabilityWarpingOut 5000`,
`invulnerabilityRecloning 60000`).

So **4000 ms** is the real server's stargate jump delay. `[I]` what it gates:
the most likely reading is the interval between `CmdStargateJump` and the
session change, i.e. the tunnel duration — the function's name says
"invulnerability" and `ballparkCommon` is ballpark code, so it is plausibly
both the delay and the arrival invulnerability window. Falsified by a capture
of the real server, which we do not have.

**Recommendation for the first implementation:** do not delay. Send steps 2-5
in one dispatcher turn. The client's own tunnel has its own duration
(`JUMP_FINISH_DELAY = 0` by default, `subwaySvc.py:438`) and
`Park.Release` waits for `OkToDumpBallPark()` by itself, so a zero-delay server
is tolerated by construction. Add the 4000 ms later as a cosmetic matter, with
a `push_later`.

### 3.4 Why `RemoveBalls` to the jumper is not optional

`gameui._HandleSessionChangeInSpace`, lines 762-780 `[V]`:

```python
765  bp = sm.GetService('michelle').GetBallpark()
766  if 'solarsystemid' in change:
768      oldSolarSystemID, newSolarSystemID = change['solarsystemid']
769      if oldSolarSystemID:                              # <-- jump, not undock
771          sm.GetService('FxSequencer').ClearAll()
772          self.LogInfo('Cleared effects')
773      if bp and session.shipid in bp.balls:
774          self.LogNotice('GameUI::OnSessionChanged, Heading for inflight with a valid ego', ...)
775          bp.ego = session.shipid
776          self.wannaBeEgo = -1
778      else:
          self.LogNotice('GameUI::OnSessionChanged:%s - Activating space view '
                         'without a ego ball properly inserted into a proper ballpark.' % (change,))
779          self.wannaBeEgo = session.shipid
780          uthread.pool('GameUI::ChangePrimaryView::inflight',
                          self._GetActivateViewFunction(viewSvc), 'inflight', change=change)
```

Line 769 is `[V]` proof that the client distinguishes a jump
(`oldSolarSystemID` truthy) from an undock (`None`) — it clears the FX
sequencer only on the former.

The branch at 773 is the trap. `ChangePrimaryView` is called **only in the else
branch**, and `ChangePrimaryView` → `_CloseView` + `_OpenView`
(`viewStateSvc.py:704-705`) → `ScatterEvent('OnPrimaryViewChanged', old, new)`
(`viewStateSvc.py:619`) → `michelle.OnPrimaryViewChanged` (`michelle.py:204`) →
`UpdateBallpark` (`:213`) → the teardown and rebind of §3.2.
`ChangePrimaryView` closes and reopens unconditionally, even `inflight` →
`inflight` `[V]`.

So: **if the jumper's own ship ball is still in the jumper's own park when the
session change lands, the client takes the "valid ego" branch, never scatters
`OnPrimaryViewChanged`, never calls `UpdateBallpark`, and the ballpark stays on
the old system for ever.** `_IsSessionAndBpSafeToJump` then returns `False`
permanently (both `bp.solarsystemID` and the moniker's bind params are stale),
`beyonce` is never rebound, and no further Destiny state can arrive.

The remedy: **send `('RemoveBalls', ([shipID],))` to the jumper before the
session change.** `[I]`, deduced from the `[V]` reads above, and consistent
with `Park.Release` doing the same thing locally (`:1196`) and with
`LockFromRelease` existing at all.

**Falsification.** If the real server does not do this, the client would stall
on every jump — so either it does, or there is a path to `UpdateBallpark` that
this lane did not find. The discriminator is one log line: after the jump,
`tools/client-log.sh | grep 'Heading for inflight with a valid ego'` versus
`grep 'without a ego ball properly inserted'`. The second is the working case.
If we see the first, the ball was not removed in time.

A safer variant worth considering: send `RemoveBalls` with **every** ball id in
the jumper's origin park, not just the ship. `Park.Release` does exactly that,
and it removes any chance of a stale ball surviving into the new park. Cost:
one longer list. `[A]`, falsified by the client logging `DoBallRemove sending a
None ball` (`michelle.py:1803`) for ids it no longer has — harmless, but noise.

### 3.5 Before or after?

**The session change must come after the `RemoveBalls`, and both before the
`CallRsp`.** Order within the turn:

1. `OnSpecialFX` (`effects.JumpOut`) — must precede the session change, because
   `subwaySvc.OnSessionChanged` acts only `if self.InJump()` (`:500`), and
   `InJump()` is true only once `StartJump` has created the effect
   (`subwaySvc.py:456`) `[V]`. A session change that arrives first finds
   `InJump()` false, skips the scene swap, and the player arrives in the old
   system's skybox.
2. `RemoveBalls` — §3.4.
3. `SessionChangeNotification`.
4. `CallRsp`.

The new park's `SetState` must come **after** the client's
`beyonce.MachoBindObject(newSSID, None)`, not with the session change:
`michelle.DoDestinyUpdate` returns immediately while `self.__bp is None`
(`michelle.py:632-633`) `[V]`, and between the session change and the new
`AddBallpark` there is a window in which `__bp` is `None`. This is the same
"failure mode with no symptom at all" that `port-tracker.md` §19.1 names for
the undock, and the existing `dispatch::pending_for_bound_call` hook is already
in the right place.

---

## 4. The destiny clock across the boundary

**Answer: the new system's park starts from whatever stamp we send, the client
does not carry the old one, and our existing single global unix-seconds clock
is correct unchanged.**

Three `[V]` facts, each of which independently rules out a reset hazard:

1. **A fresh `Park` has an empty history and `latestSetStateTime = 0`.**
   `Park.Release` sets `self.history = []` and `self.latestSetStateTime = 0`
   (`michelle.py:1220-1221`), and `AddBallpark` (`michelle.py:442-463`) builds
   a brand-new native `destiny.Ballpark` and a brand-new `Park` wrapper. So
   rule 2 of `docs/lanes/movement.md` §4.4 (`S >= latestSetStateTime`) is
   trivially satisfied by any stamp after a jump, and rule 3's per-client
   monotonicity is enforced against an empty `states` list `[V]`.

2. **`SetState` sets the park's clock from the blob, and needs no
   synchronisation.** `SetState` is in `localActions`
   (`michelle.py:1060`), so `RealFlushState` calls it directly with no
   `SynchroniseToSimulationTime` `[V]`; and `_parent_ReadFullStateFromStream`
   reads the blob's first four bytes into `Ballpark+0x1c` =
   `self.currentTime` before anything else
   (`docs/reference/destiny-reference-blob.md` §760, `docs/archive/destiny-and-the-ballpark.md` §4.4) `[V]`.
   `Bootstrap::new` already enforces `blob header == action stamp`
   (`destiny.rs:710`) `[V]`.

3. **The one deferral window is two ticks wide and cannot swallow a far-future
   stamp.** `Park.DoPreTick`, `michelle.py:1130`, with the `COMPARE_OP`
   operands read out of the bytecode (`tools/dis.py` does not print them; this
   lane re-derived them) `[V]`:

   ```
   1130    67  LOAD_FAST   eventStamp
   1130    70  LOAD_ATTR   currentTime
   1130    76  COMPARE_OP  >
   1130    82  LOAD_FAST   eventStamp
   1130    88  LOAD_ATTR   currentTime
   1130    91  BINARY_SUBTRACT
   1130    92  LOAD_CONST  3
   1130    95  COMPARE_OP  <
   1131   101  BREAK_LOOP
   ```

   i.e. `if eventStamp > self.currentTime and eventStamp - self.currentTime < 3: break`.
   The drain is deferred **only** when the stamp is 1 or 2 ticks ahead, and
   applied immediately when it is 3 or more ahead. This independently confirms
   `docs/lanes/movement.md` §4.4 rule 4, which had read the same line, and it is
   why the first `SetState` into a brand-new park works regardless of what the
   native `Ballpark` initialises `currentTime` to.

**So: do not reset, do not rebase, keep `Registry::last_destiny_stamp` global.**
`next_destiny_stamp` (`service.rs:758`) clamps `destiny::tick()` — unix seconds
— to be non-decreasing. That is already the right behaviour for N systems: one
clock for the whole cluster, every park's `currentTime` re-established from its
own `SetState` header.

### 4.1 `DoSimClockRebase` exists and is not what a jump needs

`michelle.DoSimClockRebase((oldSimTime, newSimTime))` →
`self.__bp.AdjustTimes(newSimTime - oldSimTime)` (`michelle.py:890-893`) `[V]`,
and `sessions.SessionMgr.DoSimClockRebase` shifts `session.nextSessionChange`
by the same delta (`sessions.py:661-665`) `[V]`. It is a notify event
(broadcastID `DoSimClockRebase`) with nine consumers: `michelle`,
`fxSequencer`, `godma`, `scanSvc`, `flightControls`, `spaceCamera`,
`cameraUtil`, `evecamera/animation`, `sessions` `[V]`.

This is the real cluster's mechanism for a **node's simulation clock** changing
under a client — plausibly including a move between nodes with different clock
bases. **We do not need it**, because we have one clock and because a jump
destroys the park whose times would need adjusting. `[I]`; it would be
falsified by a jump in which the client's `currentTime` after arrival is wrong
by a constant, which the `StoreState:` log line would show.

### 4.2 The one real stamp rule a jump adds

`michelle.DoDestinyUpdate:659-666`: more than one distinct stamp in a batch
calls `clientStatsSvc.OnFatalDesync()` **and proceeds anyway** `[V]`. Steps 2
and 3 of §1 are two separate actions. They may share one batch **only if they
share one stamp**; otherwise send two notifications. Sharing one stamp is free
and is what our existing `next_destiny_stamp` naturally produces inside one
turn — but the two actions have different audiences in the general case
(step 3 to the jumper and the origin observers; step 2 to the same set), so
one batch is fine here. Do not merge step 9 (`SetState`, a different system,
seconds later) into either.

---

## 5. Arrival state

### 5.1 Where the ship appears — the server decides, the client does not check

There is **no client-side arrival position computation for a gate jump**. The
only warp-in geometry in the client is `eveCfg.GetWarpInPoint(ballID, position,
radius)` (`eveCfg.py:1892-1898`) `[V]`:

```python
offset = 5000000
p = const.jumpRadiusFactor / 100.0                        # 130/100 = 1.3
x = position[0] + (radius + offset) * math.cos(radius)
y = position[1] + p * radius - 7500.0
z = position[2] - (radius + offset) * math.sin(radius)
```

which is the **warp** destination ring (5000 km out) for a celestial, used by
`michelle.GetWarpinPoint` (`michelle.py:1904`). It is not the gate arrival.
`const.jumpRadiusRandom = 15000` exists in `appConst` but has **no reader
anywhere in the client tree** `[V]` — it is a server-side constant that leaked
into the shared constants file, and the natural reading is a randomised
arrival offset up to 15 km.

**Recommendation** `[A]`: place the arriving ship at the **destination gate's
position** (from `solarsystemcontent.static`, §7) offset along a direction
derived from the roster count, at `gate radius + ~2000 m`, i.e. inside
`maxStargateJumpingDistance` so that an immediate jump back works. Gate type
29624's radius is **2473 m** (`types.fsdbinary`) `[V]`. Falsified by: the
player arriving visually inside the gate model, or by an immediate return jump
failing the 2500 m surface-distance check.

This also inherits `port-tracker.md` §27.6's known cosmetic defect — two ships
arriving at one coordinate — and the same fix applies.

### 5.2 Mode

**Mode `STOP` (2)**, zero velocity, exactly like the undock. `STOP`'s tail is
**0 bytes** (`port-tracker.md` §19.3) `[V]`, so the arriving ball is the common
header and nothing else, and `destiny::ego_ball` already builds it.

### 5.3 Invulnerability and cloak

The client **does** render a cloak, and the field is named.

`_destiny.dll`'s own method-table docstrings `[V]`:

```
DSTNORMALCLOAK      "isCloaked value for normal cloak"
DSTRESTORECLOAK     "isCloaked value for restoreing cloak"
DSTGMCLOAK          "isCloaked value for GM cloak"
isCloaked           "True if ball is cloaked"
CloakBall           / UncloakBall
"CloakBall on non-existing ball. Ignored."
"CloakBall called with mode <= 0. Ignored."
"Decloaking Ball %I64d: Removing ball from sensor %I64d"
"Ball:%I64d trying to follow cloaked ball:%I64d. Cancelled."
"Ball:%I64d trying to orbit cloaked Ball:%I64d. Cancelled."
"Missile:%I64d targeting cloaked target:%I64d. Canceled."
```

Two ways in, and both are ours to choose:

1. **In the ball record**: the per-ball `cloakMode` byte, block 6b, 1 byte at
   `rbp-0x7b`, init 0 (`docs/re/destiny-ball-crosscheck.md` §2.1, table row
   6b) `[V]`. Set it on the arriving ball in the destination `SetState` and in
   the `AddBalls2` sent to the system's other pilots.
2. **As a Destiny action**: `('CloakBall', (ballID, mode))`. `CloakBall` is in
   `Park`'s "everything else" bucket — not in `localActions`, `scatterEvents`
   or `nonDestinyCriticalFunctions` (`michelle.py:1035-1060`) — so it reaches
   the native `_parent_CloakBall` after `SynchroniseToSimulationTime`
   `[V]`. The only name in `michelle.pyc` is the string at `michelle.py:1584`,
   inside `RealFlushState`'s dispatch `[V]`.

`mode` must be `> 0` `[V]` (the DLL ignores `<= 0`). The numeric values of
`DSTNORMALCLOAK` / `DSTRESTORECLOAK` / `DSTGMCLOAK` are **not referenced
anywhere in the Python tree** — a whole-tree grep is clean `[V]` — so they are
`unknown`. `[A]` `DSTNORMALCLOAK == 1`; falsified by the client logging
`CloakBall called with mode <= 0` or by the ship rendering uncloaked.

**The trap, and it is a real one:** `autopilot._IsSessionAndBpSafeToJump`
returns `False` while `ship.isCloaked` (`autopilot.py:507`) `[V]`. A server
that cloaks an arriving ship and never uncloaks it **disables the autopilot for
that character for ever**. So the invulnerability cloak must be cleared — by
`('UncloakBall', (ballID,))`, or by the player's first action — and the
4000 ms / `invulnerabilityJumping` constant of §3.3 is the obvious duration.

**Recommendation for the first implementation:** do not cloak at all. Nothing
in the client requires it, there is no PvP to protect, and a cloak we cannot
reliably clear costs the autopilot. Add it with the timer, not before.

### 5.4 The arrival effect

`effects.JumpIn` is a `fxSequencer` guid (`fxSequencer.py:62`) and is **not** in
`traveleffects.fsdbinary` `[V]`, so it is handled by
`eve/client/script/environment/effects/Jump.pyc`'s `JumpIn` class (`:706`) and
not by the subway. `effects.GateActivity` (`fxSequencer.py:60`,
`Jump.py:892 GateActivity`) is the gate's own flash.

`[I]` the real server sends an `OnSpecialFX(guid='effects.JumpIn',
targetID=<destination gate>)` to the destination system's audience. It is
purely cosmetic: nothing gates on it. Defer it.

---

## 6. What the two systems' observers see

`RemoveBalls` and `AddBalls2` are the right instruments, and there are no better
ones. The complete list of messages per audience:

| audience | message | content |
|---|---|---|
| **Alice** (the jumper) | `OnSpecialFX('effects.JumpOut', targetID=origin gate, graphicInfo=(destSSID,))` | the tunnel, `StartJump` |
| **Alice** | `RemoveBalls([shipID])` *(or every ball id in the origin park)* | §3.4 — load-bearing, not cosmetic |
| **Alice** | `SessionChangeNotification` | §3.3 |
| **Alice** | `SetState` after her new `beyonce` bind | the whole destination park: her ball, every other pilot there, every gate |
| **Bob** (origin, stays) | the **same** `OnSpecialFX` batch | `shipID != session.shipid` → `StartJumpForAudience` (`subwaySvc.py:523`), which hides her model (`HideJumpingShip`) and plays the departure effect `[V]` |
| **Bob** | the **same** `RemoveBalls([Alice shipID])` | her ball leaves his park |
| **Carol** (destination) | `AddBalls2(((blob, [(slimDict, damageState)]),))` | Alice's ball appended. **Never** a `SetState`: it would reset Carol's park, rebuild every model and bracket, and teleport her own ship (`port-tracker.md` §27.5) |
| **Carol** *(optional)* | `OnSpecialFX('effects.JumpIn', targetID=dest gate)` | §5.4, cosmetic |

Two arity rules, both already pinned by our tests and both still binding here:

- **`RemoveBalls` takes exactly one wire argument.** `RealFlushState` appends
  its own `exploders` dict as the second positional; a server that sends two
  has its second land in `isRelease` (`michelle.py:1811`, `argcount=4` incl.
  `self`) `[V]`, `port-tracker.md` §27.3.
- **`AddBalls2`'s `extraBallData` element must be a real `tuple`**, tested with
  `type(data) is tuple` — a two-element list is read as the slim dict itself
  and kills the whole append including the balls after it `[V]`,
  `port-tracker.md` §27.2.

One ordering constraint for Bob: the `OnSpecialFX` must reach him **before** the
`RemoveBalls`, because `StartJumpForAudience` calls `self._GetModel(shipID)` and
returns early — *"Couldn't find the jumping ball, so no effect for you"*
(`subwaySvc.py:528`) — if the ball is gone `[V]`.

---

## 7. The universe graph in the shipped static data

Confirmed with `tools/fsd.py` against this installation, not from the public
SDE. All three tables are `classic` / `classic-sidecar` FSD and decode fully
with CCP's own field names.

### 7.1 `res:/staticdata/systems.static` — 8,490 rows, 1,906,081 bytes

`dict[int]` keyed by `solarSystemID`. Schema (`systems.schema`) `[V]`:

```
center: vector3            constellationID: int      factionID: int (optional)
nameID: int                neighbours: list          planetCountByType: dict[int]
planetItemIDs: list        pseudoSecurity: float     regionID: int
securityClass: string (optional)                     securityStatus: float
solarSystemID: int         sunFlareGraphicID: int (optional)
sunTypeID: int (optional)  systemWideCloud: int (optional)
warpTunnelOverwrite: int (optional)                  wormholeClassID: int (optional)
```

**`neighbours` is the adjacency list, and it is keyed by the local gate.** One
sample row, system 30000001 `[V]`:

```
constellationID  20000001
regionID         10000001
factionID        500007
nameID           268957          (localisation id -> tools/loctable.py)
securityStatus   0.8583240509033203
securityClass    'B'
neighbours       [{'jumpType': 0, 'solarSystemID': 30000003, 'stargateID': 50000056},
                  {'jumpType': 0, 'solarSystemID': 30000005, 'stargateID': 50000057},
                  {'jumpType': 0, 'solarSystemID': 30000007, 'stargateID': 50000058}]
planetItemIDs    [40000002, 40000005, 40000007, 40000008, 40000011, 40000017]
center           {'x': -8.851e16, 'y': 4.237e16, 'z': -4.451e16}
```

This one table gives the session's `constellationid` and `regionid` *and* the
gate-to-destination-system map. It is 1.9 MB, so it can be read whole at boot.

### 7.2 `res:/staticdata/solarsystemcontent.static` — 8,490 rows, 92,396,316 bytes

`dict[int] buildIndex,multiIndex` keyed by `solarSystemID`; 33 columns. The
one that matters:

```
stargates: dict[int]                # key = this system's stargate itemID
    destination: int                # the PARTNER gate's itemID, in the other system
    position: vector3               # metres, in-system, relative to the sun
    typeID: typeID
    ignoredByCorporationDefenseDjinn: bool
```

Sample, system 30000001 `[V]`:

```
50000056 -> {'destination': 50000342, 'position': {'x': 331516354560.0,  'y':  43597455360.0, 'z': -586353991680.0}, 'typeID': 29624}
50000057 -> {'destination': 50001739, 'position': {'x': -1106114273280.0,'y': -145449984000.0, 'z':  182777241600.0}, 'typeID': 29624}
50000058 -> {'destination': 50002084, 'position': {'x': 331517583360.0,  'y':  43610234880.0, 'z': -586343669760.0}, 'typeID': 29624}
```

and the reverse row confirms symmetry: `solarsystemcontent[30000003].stargates`
holds `50000342 -> {'destination': 50000056, 'typeID': 29624}` `[V]`.

Also present per system and needed for a real second-system scene: `star`
(`{id, radius, typeID, statistics}`), `planets` (with `moons` and
`asteroidBelts`, each carrying `position`, `radius`, `typeID` and
`npcStations`), `radius`, `min`/`max`/`center`, `security`,
`solarSystemNameID`.

92 MB is too big to read whole, but the classic FSD container is a keyed index
with per-key offsets, so **random access by `solarSystemID` is cheap** — which
is exactly how `cfg.mapSolarSystemContentCache` works client-side.

### 7.3 `res:/staticdata/jumps.static` — 6,989 rows, 454,289 bytes

A flat `list fixedItemSize`, one row per *connection* (not per gate). Schema
(`jumps.schema`) `[V]`:

```
jumpID: int   fromSystemID: int   stargateID: int   jumpType: enum
toSystemID: int   fromCoordinate: vector3   toCoordinate: vector3
```

Sample row 0 `[V]`:

```
{'jumpID': 4973, 'fromSystemID': 30000777, 'stargateID': 50000001,
 'toSystemID': 30000778, 'jumpType': 0,
 'fromCoordinate': {...1.35e17...}, 'toCoordinate': {...1.37e17...}}
```

**Caution:** `fromCoordinate`/`toCoordinate` are the *systems'* universe
coordinates (~1e17 m), for drawing map lines — **not** in-system gate
positions. And the list is one-directional: gate 50000342 does not appear as a
`stargateID` at all. So `jumps.static` is the map's table, not the gate table.
Prefer `systems.static.neighbours` for adjacency.

### 7.4 `res:/staticdata/starmapcache.pickle` — 2,165,420 bytes

A plain Python 2 pickle, top-level dict with `solarSystems` (8,490),
`jumps` (6,989), `regions` (114), `constellations` (1,184) `[V]`. Already used
by `service.rs:323` for the home system's constellation/region. It is the
cheapest source for the three id families and is what the client's own
`clientPathfinderService` routes over.

### 7.5 How a gate resolves to its partner — the implementable recipe

```
given originSSID and gateID:
    destSSID  = systems[originSSID].neighbours  -> first entry whose stargateID == gateID
                                                -> .solarSystemID
    destGate  = solarsystemcontent[originSSID].stargates[gateID].destination
    destPos   = solarsystemcontent[destSSID].stargates[destGate].position
    gateType  = solarsystemcontent[destSSID].stargates[destGate].typeID     # 29624, 16 or 17
```

Both lookups are in the **origin** system's rows, so no reverse index is
needed. `[V]`, checked both directions on the 30000001 ↔ 30000003 pair.

Id ranges, from `out/pyc/inventorycommon/const.pyc` `[V]`:

```
minRegion            10000000   maxRegion            19999999
minConstellation     20000000   maxConstellation     29999999
minSolarSystem       30000000   maxSolarSystem       39999999
minUniverseCelestial 40000000   maxUniverseCelestial 49999999
minStargate          50000000   maxStargate          59999999
groupSolarSystem     5          groupStargate        10        groupStation 15
groupWarpGate        366        groupWormhole        988       groupUpwellJumpGate 1408
```

`eveMachoNet._GetNodeFromAddressFromDB` enforces
`minSolarSystem <= address <= maxSolarSystem` on a `beyonce` resolve, so our
answer to `MachoResolveObject(30000003)` is the same `1` we already give
`[V]`, `docs/archive/destiny-and-the-ballpark.md` §3.

Gate hull numbers for the ball record, from `types.fsdbinary` `[V]`:

| typeID | radius (m) | mass (kg) | groupID |
|---|---|---|---|
| 29624 | 2473.0 | 1e11 | 10 |
| 16 | 3532.0 | 1e11 | 10 |
| 17 | 2323.0 | 1e11 | 10 |

(For reference, station type 2502 is radius 29746.0, which is what
`destiny::STATION_RADIUS` already carries.)

### 7.6 The stargate's slim item

This is the smallest change with the largest blast radius, because without it
the jump cannot be started at all (§2.2). A stargate's `foo.SlimItem` needs:

```
itemID       = the gate's itemID              (50000056)
typeID       = 29624 / 16 / 17
ownerID      = the faction, or 1 (unknown)    -- see below
jumps        = [ row(toCelestialID=<partner gate id>, locationID=<dest system id>) ]
```

`slimItem.jumps` defaults to `[]` (`eve/common/script/util/slimItem.pyc`
`__getattr__` lines 39-41) `[V]`, so omitting it makes `jumps[0]` raise
`IndexError` inside `DockOrJumpOrActivateGate` and the player's click does
nothing.

The element's shape is pinned twice, by two consumers that read it differently
`[V]`:

- by name: `jump.toCelestialID`, `jump.locationID`
  (`movementFunctions.py:413`; `autopilot.py:449`);
- by position, as a 2-tuple: `destGate, destSystem = gateItem.jumps[0]`
  (`subwaySvc.py:82`).

So it must be a **named 2-column row in the order `(toCelestialID,
locationID)`** — a `DBRow` or a `utillib.KeyVal`-style object that also unpacks.
A `blue.DBRow` with that descriptor satisfies both; a plain tuple satisfies only
the second; a `KeyVal` satisfies only the first. `[I]` — `DBRow` is the right
pick, and it is falsified by either consumer raising.

`ownerID`: `ProcessBallAdd` primes an owner only when `charID is not None`
(`destiny.rs:801` has the note) `[V]`, so a gate takes `char_id: None` and the
bracket name comes from `config.GetMultiLocationsEx`, which we already answer.

---

## 8. What else breaks with two systems — the implementing round's checklist

### 8.1 Confirmed `[V]`: calls the client will repeat with the new system id

Every location moniker is built from `session.solarsystemid` **and carries a
session check on it**. `moniker.Moniker.__PerformSessionCheck`
(`moniker.py:76-91`) raises `RuntimeError('MonikerSessionCheckFailure',
{attribute, expected, current, serviceName, bindParams})` on the first call
after the session moved, and `MonikeredCall` rebinds `[V]`. From
`out/pyc/eve/common/script/net/eveMoniker.pyc` `[V]`:

| moniker | bind params | session check | line |
|---|---|---|---|
| `dogmaIM` | `(solarsystemid, groupSolarSystem=5)` | `{'solarsystemid': …}` | `:63 CharGetDogmaLocation` |
| `crimewatch` | `(solarsystemid, 5)` | `{'solarsystemid': …}` | `:166 CharGetCrimewatchLocation` |
| `ship` | `(solarsystemid, 5)` | `{'solarsystemid': …}` | `:75 GetShipAccess` |
| `invbroker` | `(solarsystemid, 5)` | `{'solarsystemid': …}` | `:145/:153 GetInventoryMgr` |
| `beyonce` | bare `solarsystemID` | `{'solarsystemid': …}` | `:172 GetBallPark` |

So the server must answer `MachoResolveObject` / `MachoBindObject` for **five
services with the new solar system id**. Today those are answered for one
system; they become per-system. At least one fires synchronously inside the
session-change handler: `crimewatchSvc.OnSessionChanged` (`:150-157`) does
`eveMoniker.CharGetCrimewatchLocation().GetClientStates()` on any change
containing `solarsystemid` or `shipid`, and unpacks the answer as a 4-tuple
`[V]`.

### 8.2 Confirmed `[V]`: local chat is on the protobuf gateway, so it cannot break

`chat/client/local_chat/channel_controller.pyc` keys the local channel on
`session.solarsystemid2`, reacts to `'solarsystemid2' in change`
(`:91 OnSessionChanged`, `:98`), and talks to `LocalChatMessenger` — the
**public gateway / gRPC** transport, not machoNet `[V]`. We do not have that
transport (`port-tracker.md` §10.4), so local chat is already absent and a
second system does not make it worse. Expect `_ensure_members_for_new_solar_system`
to fail quietly on arrival, as it does on undock.

### 8.3 Confirmed `[V]`: the route and the system's contents are computed client-side

- `starmap` depends on `clientPathfinderService` and routes over
  `starmapcache.pickle`'s own jumps (`starMapSvc.py:178-183`) `[V]`. No server
  route service is needed. `autopilot.OnSessionChanged:166` calls
  `starmap.UpdateRoute(fakeUpdate=True)` on every session change `[V]`.
- `gateJumpSvc.IsNextSystemInRoute` reads `self.starmap.GetDestinationPath()[0]`
  (`:135-138`) `[V]`, client-side.
- `mapSvc.GetSolarsystemItems(ssid)` is `cfg.GetLocationsLocalBySystem` —
  which **raises if `boot.role != 'client'`** (`eveCfg.py:984`) `[V]`, i.e. it
  is built entirely from the client's own `solarsystemcontent` cache — plus
  `structureDirectory.GetStructureMapData(ssid)`, again the protobuf gateway.
  So the destination system's celestials come from the client's static data,
  not from us.

### 8.4 To verify in the implementing round

These modules declare `OnSessionChanged` in `__notifyevents__` **and** mention
`solarsystemid` — 102 of them in the tree. The ones whose handler plausibly
makes a blocking machoNet call we would have to answer, ranked by how likely a
relaunch is to hit them:

1. `eve/client/script/environment/invCache.pyc` — `ProcessSessionChange`; the
   inventory cache keyed by location. Already the source of the `shipid` trap.
2. `eve/client/script/ui/services/crimewatchSvc.pyc` — settled `[V]`, §8.1.
3. `eve/client/script/parklife/tactical.pyc` — the overview's data source.
4. `eve/client/script/ui/inflight/overview/overviewWindow.pyc` — also listens
   to `OnEnterSpace`, which `UpdateBallpark:225` scatters on every arrival.
5. `eve/client/script/parklife/sceneManager.pyc` — scene registration races
   the subway's own `UnregisterScene`/`RegisterScene` (`subwaySvc.py:507-508`).
6. `eve/client/script/parklife/environmentSvc.pyc`, `scanSvc.pyc`,
   `sensorSuiteService.pyc` (`OnEnterSpace`), `prefetchSvc.pyc`.
7. `eve/client/script/ui/services/marketsvc.pyc` — region-scoped; only matters
   if the jump crosses a region.
8. `facWarSvc.pyc`, `sovSvc.pyc`, `sovHubSvc.pyc`, `factionsvc.pyc`,
   `incursionSvc.pyc`, `solarsystemInterferenceSvc.pyc`,
   `corruptionSuppressionSvc.pyc`, `insurgencyCampaignSvc.pyc` — per-system
   world state; all of these already fail quietly on our one system, and the
   relevant check is that they fail the *same* way on the second.
9. `eve/client/script/ui/services/structure/structureProximityTracker.pyc`,
   `structureDirectory.pyc` — protobuf gateway.

### 8.5 Server-side assumptions a one-system build has baked in

| assumption | where | what it costs |
|---|---|---|
| the scene's only non-ship ball is **the character's own station** | `destiny::bootstrap_for`, `destiny.rs:1444-1467` — the comment already says *"the line to change when it does not"* | a second system has no station and three gates; the park would be ships in a void |
| a ship entering space starts at **the station mouth** | `destiny::undock_motion`, called from both `bootstrap_for:1372` and `on_ballpark_bind:1533` | an arriving jumper is teleported to the origin station |
| `Slim` has five fields and no `jumps` | `destiny.rs:805-811` | the player cannot initiate a jump at all (§7.6) |
| `Config::solar_system_id` / `constellation_id` / `region_id` are **scalars on the config** | `service.rs:314-334`, `:411` | the session change has nothing to read the new values from |
| `Character::solar_system_id` is set once at login | `service.rs:290` | nothing mutates it; the jump must |
| hull numbers exist only for category-6 ships | `destiny::hull::HULLS`, `port-tracker.md` §19.6 | a stargate is group 10, not a ship, so it is outside `SPAWNABLE` and outside the table; its `radius` and `mass` come from the same `types.fsdbinary` columns (§7.5) and want a sibling table with a sibling mechanical rule, so that adding a gate type without its numbers is a test failure rather than a gate that renders at the native default size |
| one station, one system, in `characters_at_station` and the guest list | `port-tracker.md` §27.4 | already fixed by the roster; re-check with two systems |

### 8.6 What does **not** have to change, and this is the good news

- **`space_state::Roster` is already keyed by character with the system as a
  field**, and `others_in_system` already selects the audience. A jump is
  `roster.leave(actor)` in the origin plus `roster.arrive(…new system…)` on the
  new bind — and `on_ballpark_bind` already does the arrive.
- **`destiny::on_ballpark_bind` already is the arrival.** It runs on every
  `beyonce` bind, seeds the roster, pushes the `SetState` to the arriver and the
  `AddBalls2` to the system's incumbents (`destiny.rs:1519-1600`). A jump
  reaches it for free, because the jump *is* a rebind. The only thing it gets
  wrong is where the ship starts.
- **`Registry::next_destiny_stamp` needs no change** (§4).
- **The push machinery needs no new types.** `StateUpdate`, `BallsAdded`,
  `BallsRemoved` all exist and implement `notify::Push`; only `OnSpecialFX` is
  new, and it is one more action name in `destiny.rs`.
- **`dispatch::session_change_for` needs one arm**:
  `("beyonce", "CmdStargateJump") => w.jump_change(...)`, exactly parallel to
  the existing `("beyonce", "CmdDock" | "CmdTurboDock")`.

---

## 9. Falsifiable tests

Properties, not our implementation. Each is written so it fails for the wrong
reason as well as the right one.

### 9.1 Wire-shape properties

1. **`a_jump_removes_the_jumper_from_their_own_park_before_the_session_change`** —
   fold the frames Alice receives in order; assert that the index of the
   `RemoveBalls` naming her ship is strictly less than the index of the
   `SessionChangeNotification`. Asserting only that both arrived would pass for
   the wedged client of §3.4.
2. **`the_jump_session_change_carries_exactly_four_keys_and_not_locationid`** —
   assert the key set is `{solarsystemid, solarsystemid2}` ∪ (`constellationid`,
   `regionid` iff they changed), and that `locationid` and `shipid` are
   **absent**. Pin absence explicitly: a present-and-correct `locationid` is a
   lie the client overwrites, and a present `shipid` silently skips the
   inventory cache invalidation.
3. **`the_departure_fx_precedes_the_removal_for_an_observer`** — Bob's frame
   order: `OnSpecialFX` index < `RemoveBalls` index. Otherwise
   `StartJumpForAudience` cannot find the model (`subwaySvc.py:528`).
4. **`the_departure_fx_has_fourteen_arguments_and_the_destination_in_graphicInfo`** —
   arity 14, `args[5] == 'effects.JumpOut'`, `args[3] == originGateID`,
   `args[13][0] == destinationSolarSystemID`. Arity, not just content: a
   13-argument version shifts `graphicInfo` into `timeFromStart` and the client
   jumps to `None`.
5. **`removeballs_still_takes_one_argument_after_the_jump_lands`** — the
   existing arity test, re-run on the jump's removal.
6. **`a_jump_batch_has_one_distinct_stamp`** — for every notification the jump
   emits, `len({a.stamp for a in batch}) == 1`. Violating it calls
   `OnFatalDesync` and proceeds, so the symptom is invisible on the wire.

### 9.2 Clock properties

7. **`stamps_never_decrease_across_a_system_boundary`** — jump, jump back, jump
   again; assert the sequence of every stamp this server emitted is
   non-decreasing. Assert monotonicity, never a value: `next_destiny_stamp` is
   wall-clock-derived and clamped, so an equality assertion fails exactly when
   the clamp is working (`docs/lanes/two-in-space.md` §9).
8. **`the_arrival_blob_header_equals_the_arrival_action_stamp`** — already
   enforced by `Bootstrap::new`; re-assert on the jump's `SetState` so that a
   future jump-specific constructor cannot bypass it.
9. **`no_sim_clock_rebase_is_sent_on_a_jump`** — assert the jump emits no
   `DoSimClockRebase`. If a relaunch shows the arrival park's `currentTime`
   wrong by a constant, this is the test to delete.

### 9.3 Graph properties

10. **`every_gate_resolves_to_a_partner_that_resolves_back`** — over every
    system in the static data: for each `(ssid, gateID)`,
    `content[destSSID].stargates[destGate].destination == gateID` and
    `systems[destSSID].neighbours` contains `(destGate, ssid)`. One pass over
    8,490 systems; catches an off-by-one in the reader, a wrong column, and a
    one-directional index all at once.
11. **`neighbours_and_solarsystemcontent_agree_on_the_destination_system`** —
    for each gate, the `solarSystemID` from `systems.neighbours` equals the
    system whose `stargates` dict contains the `destination`. Two files, one
    fact: the cross-file positive control that `port-tracker.md` §19.6 praises.
12. **`a_jump_only_uses_a_gate_in_the_actors_own_system`** — the server refuses
    `CmdStargateJump` for a `stargateID` not in
    `content[actor.solar_system_id].stargates`. Pin the refusal's **error name**
    (§9.5), not just that it refused.

### 9.4 Observer properties, using `fold_balls`

13. **`a_jumper_vanishes_from_the_origin_park_and_appears_in_the_destination_park`** —
    fold Bob's and Carol's frames; assert Alice's ship is in Bob's set before
    and not after, and in Carol's set after and not before, and that **Carol's
    `SetState` count is unchanged** (the news must be an append).
14. **`the_destination_park_contains_its_own_gates_and_no_station`** — fold
    Alice's arrival `SetState`; assert the ball set is
    `{her ship} ∪ {every gate in the destination system} ∪ {other pilots}` and
    contains no station id. Asserting "has at least her ship" would pass for an
    empty void.
15. **`jumping_back_puts_both_pilots_in_one_park_again`** — Alice jumps out and
    back; assert Bob's folded set returns to `{Alice, Bob, gates, station}` and
    that she arrived via `AddBalls2`, not a `SetState` he would have to absorb.

### 9.5 Error-vocabulary properties

The client has named handlers for these server errors; a server that raises a
different name gets a silent autopilot. From `autopilot._HandleJumpUserErrors`
(`autopilot.py:525-553`) `[V]`:

| error | client reaction |
|---|---|
| `NotCloseEnoughToJump` | sets throttle to 1.0, `CmdFollowBall(gateID, 0.0)`, retries |
| `SystemCheck_JumpFailed_Stuck` | autopilot off, re-raises |
| `SystemCheck_JumpFailed_*` (any) | `prompt_player_fsd_dialog(e.msg, e.dict)` |
| `SystemCheck_JumpFailed_StandingsTooLow` | autopilot off, re-raises |
| `CantJumpEmanationLockedToAnotherGate` | autopilot off, re-raises |
| `JumpNotEnoughCharge3`, `StructureServiceUnavailable`, `CannotUseJumpGateShipTooHeavy`, `NoLinkToDestination` | autopilot off, re-raises |
| anything else | logged, `ignoreTimerCycles = 5`, retried for ever |

16. **`a_jump_from_too_far_raises_NotCloseEnoughToJump`** — the one error the
    client *recovers from*, so it is the one worth getting right. Measure with
    the gate's surface distance (`maxStargateJumpingDistance = 2500` against
    `GetSurfaceDist`, i.e. radius-subtracted), not centre-to-centre.
17. **`an_unknown_jump_error_is_never_raised`** — assert every refusal path uses
    a name from the table above. The default branch retries for ever, which is
    a hang rather than a message.

### 9.6 The human check

One human, two commands. After a jump:

```
tools/client-log.sh | grep -E "without a ego ball properly inserted|valid ego"
tools/client-log.sh | grep -E "Adding ballpark|Done adding ballpark|Removing ball|Release"
```

The working case prints *"...without a ego ball properly inserted..."* followed
by `Adding ballpark <destSSID>`. The wedged case of §3.4 prints *"Heading for
inflight with a valid ego"* and no second `Adding ballpark`. That single line is
the whole discriminator for the biggest `[I]` in this document.

The client is already in full logging (`ROLE_QA` is in
`ROLEMASK_ELEVATEDPLAYER`, `docs/lanes/movement.md` §4.5), so every pushed
action prints with its stamp and arguments.

---

## 10. What this costs our architecture

Honest list, in the order a round should do it.

**1. A universe, where there is now a `Config` with three scalars.** `Config`
carries `solar_system_id`, `constellation_id`, `region_id` and station facts as
single values (`service.rs:314-334`). A jump needs a table: system → (region,
constellation, name, security) and system → gates → (partner, position, type).
This is a new module with a static-data reader behind it, and it is the single
biggest piece of new code. It is also independently useful: `docs/re/static-corpus.md`
already has the reader in Python and §7 names the exact three tables and
columns, so the work is a port, not a decode.

**2. `Character::solar_system_id` becomes mutable, and something owns the
transition.** Nothing in the tree assigns it after `Registry::new`. The jump is
the first mutation. The same defect class as `port-tracker.md` §27.4's
`station_id` ("the station I undock from", not "where I am") is waiting here:
anything that reads `solar_system_id` as "the system I logged in to" becomes
wrong the moment it can change. Audit the 15 read sites in `service.rs`.

**3. `destiny::undock_motion` splits into "how did this ship enter space".**
Called from `bootstrap_for:1372` and `on_ballpark_bind:1533`, it is the station
mouth, unconditionally. An arrival now has two origins — a station mouth and a
gate — and the roster is where the answer has to be parked between the session
change and the rebind, because the rebind is several seconds later and the
server has nothing else that remembers an in-flight jump. Concretely:
`Presence` (or a small pending-arrival map) gains the arrival position, and
`on_ballpark_bind` reads it instead of recomputing.

**4. `bootstrap_for` stops being station-shaped.** The scene becomes "what is in
this system": the pilots from the roster, plus the system's celestial furniture
from the static data. Three gates for 30000001. The in-code comment at
`destiny.rs:1448` already predicted this line.

**5. `Slim` grows a `jumps` field, and with it the first `DBRow` inside a
Destiny slim item.** `Slim` is `Copy` with five scalars today; `jumps` is a
one-element list of a two-column row. That is a real shape change in the
hottest struct in `destiny.rs`, and `eve-rowset` already has the `DBRow`
machinery. Keep it an `Option` so a ship's slim item is unchanged.

**6. One new Destiny action: `OnSpecialFX`.** Fourteen positional arguments, in
`nonDestinyCriticalFunctions`, so no stamp arithmetic. One new type in
`destiny.rs` implementing `notify::Push`, zero dispatcher edits — `port-tracker.md`
§21's trait earns its keep again.

**7. One arm in `dispatch::session_change_for` and one in
`pending_for_bound_call`** — and the second one may well need *nothing*, because
`("beyonce", "")` already fires `on_ballpark_bind` and a jump re-binds
`beyonce`. That is the cheapest part of the whole feature and it is cheap
because `pending_for_bound_call` was named before it had a body.

**8. Per-system resolve and bind for five services.** `dogmaIM`, `invbroker`,
`crimewatch`, `ship`, `beyonce` are each bound by the solar system id with a
session check. Whatever in `service.rs` currently answers a resolve by matching
the one known system id has to answer for any system in the graph. Mechanically
small, easy to get subtly wrong, and `MonikerSessionCheckFailure` is the
client's own loud symptom.

**9. What does *not* change, and it is most of the risky surface.** The roster is
already system-keyed. The stamp clock is already global and correct. The push
path, the three Destiny message types, the marshal layer, the session-change
envelope and the `beyonce` bound-object call path are all unchanged. There is no
new transport, no new dependency, and no change to `conn.rs` or the dispatcher's
loop.

**10. A thing to decide before writing code: is a solar system a server-side
object?** Today "the system" is a field on a `Presence` and an id in a `Config`.
With two systems that is still workable; with gates, celestials and per-system
pushes it starts to want a `System` with a roster slice, a furniture list and a
stamp. The honest advice is **not yet** — `others_in_system` is one query over a
small vector and the second system is a data problem, not a structure problem.
Revisit when a third subsystem wants to iterate a system's contents.

---

## 11. Open questions, ranked by cost if wrong

| # | question | marker | cost if wrong | how to settle |
|---|---|---|---|---|
| 1 | Must the server send `RemoveBalls` to the jumper before the session change for the client to rebuild its ballpark? | `[I]` (§3.4) | **The whole feature.** The client wedges with a stale park and a stale `beyonce` moniker, silently, and the next jump is refused by its own `_IsSessionAndBpSafeToJump` | the one log line in §9.6, on the first relaunch |
| 2 | Is `locationid` really derived rather than sent, for a space→space change? | `[V]` for the derivation rule, `[I]` that it suffices to trigger `subwaySvc` | the jump cinematic never runs and the player arrives in the wrong skybox; recoverable but ugly | grep the client log for `Jump got a session change without a new system location` |
| 3 | The order `OnSpecialFX` → `RemoveBalls` → session change → `CallRsp`, all in one turn | `[I]` | the tunnel does not play, or plays over a park that has already lost the ship | the capture's frame order plus the subway's own `LogInfo` lines |
| 4 | Does the arriving ship need a cloak, and what is `DSTNORMALCLOAK`? | `[A]` (§5.3) | cosmetic if omitted; **permanently disables the autopilot** if set and never cleared | ship it uncloaked first; add the cloak with the 4000 ms timer and watch `_IsSessionAndBpSafeToJump` |
| 5 | Is `slimItem.jumps[0]` a `DBRow`, or does a `KeyVal` suffice? | `[I]` (§7.6) | the player cannot start a jump; loud, immediate, one traceback | the client raises in `DockOrJumpOrActivateGate`; a `DBRow` satisfies both consumers by construction |
| 6 | What does the 4000 ms `invulnerabilityJumping` / `GetStargateJumpDelayMilliseconds` actually gate? | `[I]` (§3.3) | cosmetic timing only; a zero-delay server works | a capture of the real server, which we do not have; or just ship zero and see |
| 7 | Do `constellationid` / `regionid` have to be sent even when they do not change? | `[I]` | a stale `regionid` across a region boundary mis-scopes the market and FW/sov UI; within a region, nothing | jump across a region boundary and read `session.regionid` from the client log |
| 8 | Is `effects.JumpIn` server-sent, and to whom? | `[I]` (§5.4) | cosmetic only | add it and look |
| 9 | Does anything in the 102 `OnSessionChanged` consumers of §8.4 make a blocking call we do not answer? | unknown | one unanswered call per arrival; our capture tally already counts these | the first two-system relaunch's capture: `calls answered / unanswered` |
| 10 | Arrival position formula — is `jumpRadiusRandom = 15000` the real server's randomisation? | `[A]` (§5.1) | cosmetic, plus a return jump that fails the 2500 m check if we place the ship too far out | the constant has no reader in the client; only a real capture settles it |

---

## 12. What one relaunch settles

A single human test — undock, fly to gate 50000056, jump to 30000003, jump back
— settles questions 1, 2, 3, 5, 7 and 9 at once, because every one of them has a
distinct `michelle`/`gameui`/`subwaySvc` log line and the capture counts
unanswered calls. The order to build in is therefore: §10 items 1-5 and 7, ship
it without the cloak, without `effects.JumpIn` and without the 4000 ms delay,
and read the log.
