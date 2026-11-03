# Destiny and the ballpark

> **Demoted 2026-10-24** — §4.4 has three rows of the ball table wrong. Current truth for §4.4: [re/destiny-ball-crosscheck.md](../re/destiny-ball-crosscheck.md) §2. Kept for history; do not brief a lane from this file.

What happens on the client between `michelle.AddBallpark` and a ship the
player can see, read off the V24.01 bytecode, `_destiny.dll` and one real
capture of the undock that got us here.

The headline, up front, because it decides an architecture question:

> **A visible ship requires a server-initiated `Notification`.** The client
> never asks for Destiny state. After it binds the ballpark it falls silent
> and waits to be told. There is no polling path to retrofit.

Contents: 0 verified vs inferred, 1 where the client is now, 2 `AddBallpark`
line by line, 3 the ballpark without Destiny, 4 the state blob, 5 push,
6 the `beyonce` surface, 7 appendices, 8 what to build, in order.

---

## 0. What is verified, and what is not

`[V]` means read out of this client's bytecode, out of `_destiny.dll`, or off
a real capture in `captures/`. `[I]` means inferred and **not** settled —
treat every `[I]` as a thing that can still be wrong.

**Verified:**

- `AddBallpark` makes exactly **one** server call, `beyonce.GetFormations()`,
  and nothing after it (§2). It cannot fail for want of state.
- The `TypeError: must be tuple, not list` is `michelle.py(453)`,
  `self.__bp.LoadFormations(formations)` — a **native** call. Confirmed from
  the disassembly *and* from the client's own traceback with
  `formations = []` in the frame (§1).
- The next and **only** server calls after `GetFormations` are
  `beyonce.MachoResolveObject(30000001)` and
  `beyonce.MachoBindObject(30000001, None)`, and **both already succeed** on
  our server today. Verified on the wire, capture
  `captures/1792431326-0002.evecap`, calls `[324] [326] [328]` (§3).
- After that bind the client issues **no further service calls at all** — only
  `alert.SendClientStackTraceAlert` and `eventLog.LogClientStats` (§3, §5).
- Destiny state reaches the client only as `michelle.DoDestinyUpdate`, which
  is a **notify event**, not a method the client calls. `michelle`'s
  `__notifyevents__` resolves to a list beginning
  `['DoDestinyUpdate', 'DoDestinyUpdates', …]` (§5).
- The carrier is a command-12 `Notification` with a **broadcast** destination
  whose `broadcastID` is the literal string `'DoDestinyUpdate'`;
  `CoreBroadcastStuff.NotifyUp` routes any `broadcastID` starting with `Do`
  to `sm.SendEventWithoutTheStars` (§5).
- The state blob itself is an **opaque binary string** parsed by
  `destiny.Ballpark.ReadFullStateFromStream` in `_destiny.dll`. It is not
  marshalled Python (§4).
- The first four bytes of that blob are an integer stored at `Ballpark+0x1c`;
  the ball records that follow are self-delimiting and terminate when the
  reader hits end of stream (§4.3).
- `DstBallMode` is 0..12 in the order GOTO, FOLLOW, STOP, WARP, ORBIT,
  MISSILE, MUSHROOM, BOID, TROLL, FIELD, MINIBALL, RIGID, FORMATION —
  cross-checked three ways (§7.1).
- The client keep-alive is 240 s (`clientKeepAliveTimerInterval`), and the
  capture shows the client sending **no ping at all** for the last 48 s of a
  62 s session. You cannot ride inbound frames (§5.3).

**Inferred, and flagged as such throughout:**

- ~~`[I]` The *names* of the fields inside each ball record.~~ **Closed, and
  three of them were wrong.** A second independent implementation of the same
  C++ serialiser names nine of the eleven, with every sentinel default (-1,
  -1, -1, 1.0, 0) matching an init value read out of the DLL. `followId` in
  the common header was really `harmonic`; `inertia` and `speedFraction` were
  swapped and mistyped. See `docs/re/destiny-ball-crosscheck.md`. Still `[I]`:
  block 8 and the `0xff` byte (§6 there).
- `[I]` The shape of the `bag` object `Park.SetState` consumes. The attribute
  *names* are verified (they are `LOAD_ATTR` operands); the **container
  class** is not — `utillib.KeyVal` is the obvious candidate and is what we
  already emit, but the real server's class is not in this tree (§4.2).
- `[I]` That a minimal one-ball `SetState` is enough to draw a ship. Nothing
  downstream of `SetState` has ever run against our server.
- `[I]` Everything in §8 about *how long* each step takes.

---

## 0a. Confirmed by an actual undock, 2026-10-20

Everything below this section was read out of the client. On 2026-10-20 the
owner undocked against the live server for the first time since the session
change landed, and the capture plus the client's own log settle several of its
claims — and narrow the gap to one thing.

**The session change is correct and the client accepts it.** From the client's
log, `client-2026.10.20.06-300.log:33209`:

```text
GameUI::OnSessionChanged:{'stationid': (60012526, None),
                          'locationid': (60012526, 30000001),
                          'solarsystemid': (None, 30000001)}
  - Activating space view without a ego ball properly inserted into a proper
    ballpark.
```

All three keys move exactly as `service::undock_change` sends them, and the
client **activates the space view** on the strength of it. So nothing in the
session, the `ship.MachoBindObject(('Undock', …))` reply, or
`beyonce.GetFormations()` is what holds the undock up.

**The ballpark is created, and it ticks.** Same log:

```text
33443  Waiting for ballpark 0
36499  Adding ballpark 30000001
36501  Ballpark:-2026721328 created with time-step 1000 msec. Total of 2
36518  Done adding ballpark 30000001
37487  [...] client _destiny BallPark            <- and every ~11 s after
```

So §3's conclusion — *"the ballpark is reachable without Destiny, and then it
stops"* — is confirmed from the other side. It is reachable, it is built for
the right solar system, and it runs.

**What is missing is exactly one thing: the ego ball.** The client's own words
are *"without a ego ball properly inserted into a proper ballpark"*, and that
is the **only** complaint it makes about the undock. A whole-log search for
`ego ball`, `SetState`, `AddBall` and `no ball` returns that single line.

**And the client never asks for it.** The capture's own tally for the session
is **232 calls, 232 answered, 0 unanswered** — the first complete session with
nothing refused. The client binds `beyonce` for solar system 30000001 and then
simply waits. There is no call of ours to answer better, which is §5's "a
visible ship requires a server-initiated push" confirmed empirically rather
than by inference.

One claim here needs narrowing rather than confirming. The *"Failed to get a
valid ballpark in time after trying 30 times"* line, which earlier sessions
treated as the undock symptom, appears at **33.6 s** in this capture — during
login, not during the undock at 609 s. It fires once and does not recur after
undocking. So it is a login-path artefact and not the undock failure; the
undock failure is silent.

**What this means for the work.** The undock is now a single-cause gap: push a
Destiny state carrying the player's own ball. Nothing on the request side is
missing, the session is right, the ballpark is ready to receive it, and the
blob's shape is the open question — which is what `tools/destiny-ref.sh` and
the `/execfile` exist to settle.

## 1. Where the client is now

The undock chain, from `captures/1792431326-0002.evecap` and the matching
client log `client-2026.10.19.17-296.log`:

```
[297] 42816ms  ship.MachoBindObject((60012526,15), ("Undock", (100000001, false), {...}))  -> None
                session change {'stationid': (60012526, None),
                                'locationid': (60012526, 30000001),
                                'solarsystemid': (None, 30000001)}
[300] 42822ms  dogmaIM.MachoResolveObject((30000001, 5))
[304] 42832ms  dogmaIM.MachoBindObject(..., ("GetAllInfo", ...))
[306] [308]    invbroker resolve + bind (GetInventoryFromId)
[310] [312]    crimewatch resolve + bind (GetClientStates)
[316]          crimewatch bind again
[322] 42886ms  home_station.get_home_station()
[324] 43418ms  beyonce.GetFormations()            -> []        <-- THREW
[326] 43419ms  beyonce.MachoResolveObject(30000001)  -> 1
[328] 43433ms  beyonce.MachoBindObject(30000001, None) -> object, None
[332] 43434ms  alert.SendClientStackTraceAlert(...)             <-- the throw
... nothing but alerts and eventLog for the remaining 19 seconds ...
```

The client log shows, in order:

```
17874: ChangePrimaryView inflight
18206: view ViewInfo(view=SpaceView(name=inflight) type=0) opened
18302: Adding ballpark 30000001
18325: EXCEPTION #23 ... TypeError: must be tuple, not list
```

Two things fall out of that ordering and both matter.

**The space view opened before the ballpark was even attempted.** `SpaceView`
is up, drawn, and has been for ~90 ms when `AddBallpark` starts. The view
layer does not wait on Destiny.

**The no-ego branch had already fired**, at `gameui.py(591)` in
`_HandleSessionChangeInSpace`:

```
GameUI::OnSessionChanged:{'stationid': (60012526, None), 'locationid':
(60012526, 30000001), 'solarsystemid': (None, 30000001)} - Activating space
view without a ego ball properly inserted into a proper ballpark.
```

This is *not* a fallback that we tripped by being incomplete. It is the
normal path. At session-change time `michelle.__bp` is `None` — the ballpark
is built later, from `OnPrimaryViewChanged`. The real server takes this branch
on every undock too. What the branch does is set
`self.wannaBeEgo = session.shipid` and carry on; `gameui.DoBallAdd`
(`gameui.py` ~820) later sees a ball arrive whose `ball.id == self.wannaBeEgo`
and promotes it to ego then. **The ego assignment is deferred, by design, to
the moment the ship's ball lands.**

The throw itself, from the alert we captured:

```
michelle.py(213) OnPrimaryViewChanged
michelle.py(224) UpdateBallpark
michelle.py(453) AddBallpark
      formations = []
      solarsystemID = 30000001
```

`michelle.py(453)` is `self.__bp.LoadFormations(formations)` — confirmed from
the lnotab, not guessed. `LoadFormations` is a native method on
`destiny.Ballpark` (the string is in `_destiny.dll`), so it type-checks its
argument the way C does. Caught by
`serviceManager.MollycoddledUthread(1014)`, so the client survived it — but
`bpReady` stayed `False`, which is why the log then fills with
`Waiting for ballpark 0..19` from `WaitForBallpark` (`michelle.py` 501–509)
and eventually `clientStatLogger.LogString("Failed to get a valid ballpark in
time after trying 30 times")`.

---

## 2. `AddBallpark`, line by line

`eve/client/script/remote/michelle.py`, lines 442–463. These are real source
line numbers from the code object's `firstlineno`/`lnotab`, not uncompyle6
output lines.

```python
442  def AddBallpark(self, solarsystemID):
443      self.LogNotice('Adding ballpark', solarsystemID)
444      self.bpReady = False
445      if self.__bp is not None:
446          self.RemoveBallpark()
447      self.__bp = blue.classes.CreateInstance('destiny.Ballpark')
449      self.LogInfo('GetBallpark1 object', ...)
450      Park(self.__bp, {'broker': self, 'solarsystemID': long(solarsystemID)})
451      self.LogInfo('GetBallpark2 object', ...)
452      formations = sm.RemoteSvc('beyonce').GetFormations()
453      self.__bp.LoadFormations(formations)
455      self.ballNotInParkErrors = {}
456      self.handledBallErrors = set()
457      self.__bp.SetBallNotInParkCallback(self.HandleBallNotInParkError)
458      sm.ScatterEvent('OnAddBallpark')
459      self.__bp.Start()
460      self.bpReady = True
462      self.LogNotice('Done adding ballpark', solarsystemID)
463      return self.__bp
```

**After `GetFormations` returns, `AddBallpark` makes no further server call.**
Lines 453, 457, 458, 459, 460 are all local: two native calls on the
Ballpark, a local `ScatterEvent`, and a flag.

Two subtleties in line 450.

`Park` is `decometaclass.WrapBlueClass('destiny.Ballpark')`, whose metaclass
`__call__` (`decometaclass/decometaclass.py`) is:

```python
def __call__(cls, inst=None, initDict=None, *args, **kwargs):
    if not inst: inst = blue.classes.CreateInstance(cls.__cid__)
    inst.__klass__ = cls
    if initDict:
        for k, v in initDict.iteritems(): setattr(inst, k, v)
    if hasattr(inst, '__init__'): inst.__init__()
    return inst
```

So `Park(self.__bp, {...})` does not build a new object — it **re-classes the
native instance** created on line 447, sets `broker` and `solarsystemID` on
it, and runs `Park.__init__`. That is why `Park`'s methods can call
`self._parent_X(...)`: `_parent_` reaches the native implementation that the
Python override shadows.

And `Park.__init__` (lines 1010–1060) ends with, at **line 1032**:

```python
self.initRemoteBallparkThread = uthread2.StartTasklet(self.InitializeRemoteBallpark)
```

`InitializeRemoteBallpark` (lines 2316–2329) is:

```python
self.remoteBallpark = eveMoniker.GetBallPark(self.solarsystemID)
self.remoteBallpark.Bind()
```

with ten retries, a 1 s sleep between, and `stdlog.exception(...)` per
failure. `eveMoniker.GetBallPark` is `Moniker('beyonce', solarsystemID)` with
`SetSessionCheck({'solarsystemid': solarsystemID})`.

That tasklet is what produced calls `[326]` and `[328]` in §1: it was started
on line 450 and ran while line 452's `GetFormations` was outstanding. **The
`Bind()` return value is discarded** — `InitializeRemoteBallpark` ignores it.
Remember that; §5 turns on it.

### The surrounding methods

- `OnPrimaryViewChanged` (204–213): if we left `inflight` for `structure`,
  `RemoveBallpark()`; then `UpdateBallpark()` unconditionally (line 213).
- `UpdateBallpark` (215–234): if `session.solarsystemid` **and** the primary
  view is `inflight` or `structure` — start the `space` service, then
  `self.AddBallpark(session.solarsystemid)` (line 224) if `__bp` is `None`,
  then `sm.ScatterEvent('OnEnterSpace')`. Otherwise `RemoveBallpark()` and, if
  there is no ballpark, unregister the `default` scene and
  `blue.recycler.Clear()`.
- `GetBallpark(doWait=False)` (474–499): returns `__bp` **only if
  `self.bpReady`**. With `doWait=True` it spins `WaitForBallpark` — 30 tries,
  `SleepSim(1000)` each — then gives up with the `LogString` we saw.
- `GetMyPosition` (590–598) and `IsPositionWithinWarpDistance` (896–908) both
  call `GetBallpark(doWait=True)`, which is where a 30-second stall comes
  from when the ballpark never becomes ready.

---

## 3. Is the ballpark reachable without Destiny? Yes — and then it stops

**`AddBallpark` succeeds with nothing.** With `GetFormations` answered as an
empty *tuple*, every line from 453 to 463 runs: `LoadFormations(())` loads no
formations, `Start()` starts the native tick, `bpReady = True`, and
`GetBallpark()` begins returning a live, empty ballpark. `michelle` is then
in its steady state with zero balls.

**The next server call after `GetFormations` succeeds** is the pair the
`InitializeRemoteBallpark` tasklet already makes, and both are already served:

| call | bind params | what we answer today | enough? |
|---|---|---|---|
| `beyonce.MachoResolveObject(30000001)` | the solar system id, a **bare long**, not a tuple | `1` (our node id) | yes `[V]` |
| `beyonce.MachoBindObject(30000001, None)` | same, fused call is `None` | `(<object N=1:21>, None)` | yes `[V]` |

Note the bind params: `beyonce` is bound by **bare `solarsystemID`**, where
`dogmaIM`, `invbroker`, `ship` and `crimewatch` are bound by
`(locationID, groupID)` 2-tuples. `eveMoniker.GetBallPark` is the only place
that constructs it, and `eveMachoNet._GetNodeFromAddressFromDB` enforces
`const.minSolarSystem <= address <= const.maxSolarSystem` on the server side.

Worth knowing for the resolve: the client's
`eveMachoNet.GuessNodeIDFromAddress` **begins with a bare `return`** — the
whole alternatives table below it is dead code in this build. So the client
will always issue a real `MachoResolveObject` for `beyonce` rather than
guessing the node from a sibling service's cache entry.

**And then the client goes quiet.** In the capture, after `[328]` there are no
further calls for 19 seconds except `alert` and `eventLog`. That is not an
artifact of the `GetFormations` throw: the throw happened *after* the bind,
and nothing in `AddBallpark`'s remaining lines, in `Park.__init__`, or in
`InitializeRemoteBallpark` issues another call. There is nothing left for the
client to ask.

So the honest description of the state we are about to reach is:

- space view open, scene loaded, HUD up;
- a running, empty ballpark; `bpReady == True`;
- `ballpark.ego` unset, `gameui.wannaBeEgo == session.shipid`;
- **no ship on screen, and no camera target** — `gameui._NewState` only acts
  `if bp and bp.balls.get(bp.ego, None)`;
- the client idle on the wire.

That is a real and useful waypoint. It is also the end of what the client
will do unprompted.

---

## 4. The Destiny state blob

### 4.1 Which call carries it — none of them

There is no `beyonce.GetState`. A scan of all 12,774 parseable modules for
`ReadFullStateFromStream`, `WriteFullStateToStream`, `FlushState` and
`AddBallsToPark` finds them in exactly two files: `michelle.pyc` and
`destiny/net/client/_ticker.pyc`. Nothing fetches state.

The path is:

```
server  --Notification(broadcastID='DoDestinyUpdate')-->  client
    CoreBroadcastStuff.NotifyUp          (BroadcastStuffGPCS.py)
    -> sm.SendEventWithoutTheStars('DoDestinyUpdate', args)
    -> Michelle.DoDestinyUpdate(state, waitForBubble, dogmaMessages=[])   michelle.py 625
    -> Park.FlushState(state, waitForBubble)                              michelle.py 668 -> 1423
    -> (next client tick) Park.DoPreTick -> Park.RealFlushState           michelle.py 1121 -> 1507
    -> Park.SetState(bag)                                                 michelle.py 1525 -> 1224
    -> self._parent_ReadFullStateFromStream(ms)   NATIVE                  michelle.py 1239
```

`michelle.DoDestinyUpdate` is reachable only as a notify event: it is **not**
in `Michelle.__exportedcalls__`, and nothing in the client calls it locally.
It *is* the first entry of `Michelle.__notifyevents__`. (uncompyle6 renders
that list as `[15, 16, … 25]`; resolved against the class body's const table
— the trap `docs/guides/debugging-the-client.md` §4 warns about — it is
`['DoDestinyUpdate', 'DoDestinyUpdates', 'OnFleetStateChange',
'OnDroneStateChange', 'OnDroneActivityChange', 'OnAudioActivated',
'DoSimClockRebase', 'OnPrimaryViewChanged', 'OnSpaceWhiteOutStart',
'OnAsteroidRevealed', 'OnAsteroidTerminated']`. `eve/client/script/parklife/
recorder.py` lists the same two names in plain strings, which is an
independent confirmation.)

A corroborating detail that costs nothing to check: the native 195-entry
marshal string table (`eve-marshal/src/string_table.rs`, read out of
`blue.dll`) contains `beyonce`, `DoDestinyUpdate`, `DoDestinyUpdates`,
`AddBalls`, `RemoveBalls`, `SetMaxSpeed`, `SetSpeedFraction`, `OnSpecialFX`,
`TerminalExplosion`, `ballID`, `foo.SlimItem` and `macho.Notification`. That
table is CCP's list of the strings most often on the wire. Every one of those
is a Destiny action name or a Destiny payload type. The protocol we are
missing is the one the table was built for.

### 4.2 The shape of a destiny update

`state` is a list of entries, each `(timestamp, (funcName, args))`:

```python
for stamp, (funcName, args) in state: ...     # recorder.py DoDestinyUpdate
```

- `timestamp` is a **destiny tick**, not a wall clock. `Park.tickInterval =
  const.simulationTimeStep = 1000` ms, and `_destiny.dll` logs
  `"Ballpark:%d created with time-step %d msec"` — **Destiny ticks at 1 Hz.**
- `Park.FlushState` (1423) special-cases `state[0][1][0] == 'SetState'`: it
  records `latestSetStateTime` and discards history older than it. Any batch
  whose first entry is older than `latestSetStateTime` is dropped with
  `'Michelle discarded a state that was too old'`.
- All entries in one batch should share one timestamp.
  `Michelle.DoDestinyUpdate` line 666 calls
  `clientStatsSvc.OnFatalDesync()` if `len({a[0] for a in state}) > 1`.
- One pseudo-action is expanded client-side first: an entry
  `(stamp, ('PackagedAction', (blob,)))` is `blue.marshal.Load`ed and spliced
  in (line 645). We will never need to send one.

`Park.RealFlushState` (1507) then dispatches each `funcName` three ways, from
sets built in `Park.__init__` (resolved from `__init__`'s const table —
uncompyle6 shows these as small integers too):

| set | members | handling |
|---|---|---|
| `localActions` | `AddBalls`, `AddBalls2`, `RemoveBalls`, `SetState`, `RemoveBall`, `TerminalPlayDestructionEffect` | `getattr(self, funcName)(*args)` — Python, on `Park` |
| `nonDestinyCriticalFunctions` | `OnDamageStateChange`, `OnSpecialFX`, `OnFleetDamageStateChange`, `OnShipStateUpdate`, `OnSlimItemChange`, `OnDroneStateChange`, `OnSovereigntyChanged`, `OnDbuffUpdated`, `OnClientControllerEvent`, `OnDotVictimUpdated` | `getattr(self, funcName)(*args)`, **without** time synchronisation |
| `scatterEvents` | `Orbit`, `GotoDirection`, `WarpTo`, `SetBallRadius`, `GotoPoint`, `SetBallInteractive`, `SetBallFree`, `SetBallHarmonic`, `FollowBall`, `Stop` | native `_parent_<funcName>`, then `sm.ScatterEvent('OnBallparkCall', funcName, args)` |
| everything else | e.g. `SetSpeedFraction`, `SetMaxSpeed`, `CloakBall`, `SetBallVelocity`, … | native `_parent_<funcName>(*args)` |

Anything not in the first two sets is first time-synchronised:
`SynchroniseToSimulationTime(eventStamp)` rewinds to a stored snapshot and
`_parent_Evolve()`s forward. If that fails the client calls
`OnRecoverableDesync()` and `RequestReset()` (line 1569) — see §6.

Every action is wrapped in `try/except Exception: stdlog.error(...)`, so a
malformed action is a logged traceback and a skipped action, not a hang. That
is a good debugging property: **the Destiny stream self-reports.**

### 4.3 `SetState` and the blob

`Park.SetState(bag)`, michelle.py 1224–1283. One positional argument, so the
action is `(stamp, ('SetState', (bag,)))`.

```python
1225  self.componentRegistry = self.broker.CreateComponentRegistry()
1228  self.stateByDroneID = {d.droneID: KeyVal(droneID=…, ownerID=…, locationID=session.solarsystemid,
1231                           controllerID=…, activityState=…, typeID=…, controllerOwnerID=…, targetID=…)
                              for d in bag.droneState}
1233  sm.SendEvent('DoBallClear', bag.solItem)
1234  self.ClearAll()
1236  ms = blue.MemStream()
1237  ms.Write(bag.state)
1239  self._parent_ReadFullStateFromStream(ms)          # NATIVE
1240  self.ego = long(bag.ego)
1241  self._parent_Start()                              # NATIVE
1243  self.slimItems = {}
1246  for slimItem in bag.slims:
          if type(slimItem) is int: raise TypeError
          if slimItem.itemID not in self.balls:
              raise RuntimeError('BallNotInPark', slimItem.itemID, slimItem)
1253      self.ProcessBallAdd(slimItem, ballQueueData)
1260  self.validState = True
      self.FlushSimulationHistory()
      self.QueueBallData(ballQueueData, newState=self)
1266  self.damageState = {}
1267  for k, v in bag.damageState.iteritems(): self.damageState[k] = (v, now)
1271  for effectInfo in bag.effectStates:
          self.OnSpecialFX(*effectInfo); self.ScatterEwars(*effectInfo)
1275  self.solItem = bag.solItem
1277  self.industryLevel = bag.industryLevel
1278  self.researchLevel = bag.researchLevel
1280  self.OnDbuffUpdated(self.ego, bag.dbuffState)
1282  sm.ScatterEvent('OnBallparkSetState'); ballpark_signals.on_ballpark_reset()
```

**`bag` is a plain attribute bag.** The attribute names above are verified —
they are `LOAD_ATTR` operands in the code object. The *class* is not in this
tree. `[I] utillib.KeyVal` is the right choice: it is in the client's
unmarshal whitelist, we already emit it, and attribute access is all `SetState`
does.

The minimum `bag` for one ship:

| attribute | type | smallest honest value | why |
|---|---|---|---|
| `state` | **bytes** | the binary blob, §4.4 | `ms.Write(bag.state)` then native parse |
| `ego` | long | `session.shipid` | `self.ego = long(bag.ego)` |
| `slims` | list of `foo.SlimItem` | one, for the ship | every `itemID` must already be a ball or `RuntimeError('BallNotInPark')` |
| `droneState` | iterable | `[]` | comprehension — `None` throws |
| `damageState` | dict | `{}` | `.iteritems()` — `None` throws |
| `effectStates` | iterable | `[]` | `for ... in` — `None` throws |
| `solItem` | object | the solar-system slim item, or `None` `[I]` | only `SendEvent('DoBallClear', …)` and stored |
| `industryLevel` | number | `0` `[I]` | stored only |
| `researchLevel` | number | `0` `[I]` | stored only |
| `dbuffState` | — | `None` | `incomingDbuffTracker.OnDbuffUpdated`: falsy → `_ResetState()` **[V]** |

`foo.SlimItem` (`eve/common/script/util/slimItem.py`) is forgiving by
construction: `__passbyvalue__ = 1`, and `__getattr__` returns `None` for any
name that does not start with `__`, derives `groupID`/`categoryID`/`radius`
from `typeID` through `evetypes` (client-side static data), and returns `[]`
for `modules`/`jumps` and `0` for `ballID`/`bounty`. So a SlimItem carrying
`itemID`, `typeID` and `ownerID` is a complete one as far as the client is
concerned.

What `ProcessBallAdd` (1611) then does with it, for a ship:
`ownersToPrime` gets `ownerID`, and `charID`/`corpID` too if `charID` is set;
`corpID` goes to `tickersToPrime`; `allianceID` to `allyTickersToPrime`; and
since a ship is neither celestial nor stargate nor fake, it ends up in
`cfg.evelocations.Hint(...)`. Those drain through `michelle.Dispatcher` into
`cfg.evelocations.Prime` / `cfg.eveowners.Prime` /
`cfg.corptickernames.Prime` / `cfg.allianceshortnames.Prime` — i.e. into
`config.GetMultiLocationsEx`, `GetMultiOwnersEx`,
`GetMultiCorpTickerNamesEx`, all of which `service.rs` already answers. **The
prime wave behind a ball is already served.**

Then `QueueBallData` → `sm.SendEvent('DoBallsAdded', …)` → `gameui.DoBallAdd`,
which sees `ball.id == self.wannaBeEgo`, sets `bp.ego`, and calls `OnNewState`
→ `camera.LookAtMaintainDistance(bp.ego)`. **That is the moment the ship
appears.**

### 4.4 The blob is binary, and here is what we have of it

`destiny/__init__.py` is `_destiny = blue.LoadExtension('_destiny')` with
`Ball`, `Ballpark` and `ClientBall` wrapped. `ReadFullStateFromStream`,
`WriteFullStateToStream` and `WriteBallsToStream` are strings in
`bin64/_destiny.dll` (916 KB, `_destiny.pdb`, built from `src/Ballpark.cpp`).
There is **no Python source to port and no marshal structure to mirror** —
this is a C++ serialiser, and it is the hard wall `docs/archive/roadmap.md` M3 names.

What the disassembly gives, bounded effort, from `.pdata` function bounds:

**The stream header**, `0x1800978c0` (the balls reader; it calls the ball
reader at `0x1800989e0`):

```
0x1800978ef  rcx = stream; call stream->Read(dst, 4)   ; FOUR bytes
0x18009790c  eax = those 4 bytes
0x180097913  mov dword ptr [ballpark + 0x1c], eax
```

So the blob opens with a **4-byte integer**. `[I]` it is the simulation tick
(`Ballpark.currentTime`): it is the only `uint32` the ballpark needs from a
snapshot, and `SynchroniseToSimulationTime` compares incoming event stamps
against exactly that. **[V]** that it is four bytes and the first thing read.

Then a loop: `ball = ReadBall(ballpark, &stream, flag)` until it returns
`NULL`. **The ball list is self-delimiting** — the reader bails when its first
read comes up short (`0x180098a36 test rax,rax; je`). There is no count.

Then a fixup pass over the balls just created: for `mode` in bitmask
`0x1032` — bits 1, 4, 5, 12, i.e. **FOLLOW, ORBIT, MISSILE, FORMATION** —
resolve `followId` to a pointer, logging
`"%s: Ball %lld not in park"` if it cannot. `cmp eax, 0xc; ja` bounds mode at
12.

**One ball record**, `0x1800989e0` (3264 bytes; this is the first ~25% of it,
which is the part common to all modes):

| # | bytes | destination | `[I]` meaning |
|---|---|---|---|
| 1 | 8 | `rbp+0x00` | **itemID / ballID**, int64. A short read here ends the list `[V]` |
| 2 | 1 | `rbp-0x7c` | **mode** (`DstBallMode`) `[V]` — it is compared against `0x0b` = RIGID at `0x180098b1d` |
| 3 | 4 | `rbp-0x54` | **radius**, float32, default 1.0 *(cross-check §1)* |
| 4 | 24 | `rbp+0x38` | **position**, 3 × float64 `[V]` (the slot is zeroed as 24 bytes) |
| 5 | 1 | `rbp+0x158` | **flags**; bit 0 gates block 7 `[V]` — bit 0 is `IS_FREE`; whole byte in cross-check §3 |
| 6 | *if mode != RIGID* | | |
| 6a | 8 | `rbp-0x20` | **mass**, float64 — the constant is **1e6**, or 1e4 for a container/wreck *(cross-check §1)* |
| 6b | 1 | `rbp-0x7b` | **cloakMode**, u8, init 0 *(cross-check §1)* |
| 6c | 8 | `rbp+0x08` | **harmonic**, int64, init `-1` — *was `[I]` followId; corrected, `docs/re/destiny-ball-crosscheck.md` §2.1* |
| 6d | 4 | `rbp-0x50` | **corporationID**, int32, init `-1` *(§1 of the cross-check)* |
| 6e | 4 | `rbp-0x4c` | **allianceID**, int32, init `-1` *(ditto)* |
| 7 | *if flags & 1* | | |
| 7a | 4 | `rbp-0x60` | **maxVelocity**, float32 — *was `[I]` int32; mistyped, cross-check §2.2* |
| 7b | 24 | `rbp+0x20` | **velocity**, 3 × float64 `[I]` (same 24-byte shape as position) |
| 7c | 4 | `rbp-0x64` | **inertia**, float32, init 1.0 — *was guessed `speedFraction`; wrong, cross-check §2.2* |
| 7d | 4 | `rbp-0x68` | **speedFraction**, float32, init 0 — *was `[I]` int32; mistyped, cross-check §2.2* |
| 8 | *gated on a module config byte at `0x1800d42d1`* | | `[I]` the `destiny.settings` orientation flag |

**The names in the table above are no longer guesses.** Nine of them were
`[I]` until a second implementation of this same serialiser named them
independently and every default value matched an init value read out of the
DLL. Three rows were *wrong* and are corrected in place; one disagreement went
the other way and we kept ours. The whole comparison, including the
mode-dependent tails and the mini-geometry sections, is
**`docs/re/destiny-ball-crosscheck.md`**.

The short version for anyone implementing: a stationary ship is mode **STOP**,
whose tail is **empty**, so the minimum viable ball is the common header and
nothing else.

Beyond that the record becomes mode-dependent and the remaining ~2.5 KB has
not been read directly here (but see the cross-check §4.1 for a second
reading of every mode tail). `WriteBallToStream: Unknown ball mode %d in dump` and
`ReadBallFromStream: Unknown ball mode %d in dump` are the two guards.

**The cheap way to finish this is not more disassembly.** The client ships the
*writer*: `Park.StoreState` (michelle.py 1155) is

```python
ms = blue.MemStream()
self.WriteFullStateToStream(ms)
```

and `blue.MemStream` exposes `Read`, `Write`, `Seek`, `Size` and a `data`
member (strings in `blue.dll`: `MemStream/mData`). The client also loads
loose `.py` overrides with no guard (`docs/archive/roadmap.md` M0). So a dozen lines
dropped into the client can build a `destiny.Ballpark`, `AddBall` one ball
with known values, `WriteFullStateToStream`, and write the bytes to
`C:\eve-logs\`. That gives a **byte-exact reference blob produced by the very
code that will parse ours**, and turns the remaining layout work into diffing
two dumps that differ in one field. `Park.AddClientSideBall` (1711) shows the
native `AddBall` arity:

```python
ball = self.AddBall(ballID, 1.0, 0.0, 0, False, isGlobal, False, False, False,
                    x, y, z, 0, 0, 0, 0, 0)        # 17 positional args
```

See §8 step 2.

### 4.5 `AddBalls` and `AddBalls2` — the incremental forms

Both are `localActions`, so they run `Park`'s Python, not the native method.

```python
1645 def AddBalls(self, chunk):
         state, slims, damageDict = chunk
         ms = blue.MemStream(); ms.Write(state)
         self._parent_ReadFullStateFromStream(ms, 2)      # mode 2 = append
         for slimItem in slims: ... ProcessBallAdd(...)

1674 def AddBalls2(self, chunk):
         state, extraBallData = chunk
         ms = blue.MemStream(); ms.Write(state)
         self._parent_ReadFullStateFromStream(ms, 2)
         for data in extraBallData:
             slimItemDict, damageState = data if type(data) is tuple else (data, None)
             slimItem = SlimItem(); slimItem.__dict__ = slimItemDict
```

`AddBalls2` is the one to prefer once we have a writer: **it takes plain
dicts**, not `foo.SlimItem` instances, so it needs no class on the wire at
all. The same `ReadFullStateFromStream` blob format is used, with the second
argument `2` meaning append-to-existing rather than replace.

(`destiny/net/client/_ticker.py` is a newer, pure-Python reimplementation of
the whole ticker — `_set_state(state, ego_ball_id)` takes two args rather than
a `bag`, and `AddBallsToPark` takes the blob alone. **Nothing imports
`destiny.net.client` in this build**; the grep is clean. It is dead code we
should not design against, but it is a useful second reading of the same
protocol.)

---

## 5. Push: the answer, and the evidence

**A client that only ever polls cannot see its own ship.** This is as close to
proven as this project gets, and it rests on four independent observations.

### 5.1 Nothing on the client fetches state

Scanned across all 12,774 parseable modules:

- `ReadFullStateFromStream` / `WriteFullStateToStream` → `michelle.pyc`,
  `destiny/net/client/_ticker.pyc` (unused). Both are *parsers*, not fetchers.
- `UpdateStateRequest` → **`michelle.pyc` only**.
- `DoDestinyUpdate` → `michelle.pyc`, `recorder.pyc` (a capture tool), and the
  marshal string tables. Neither call site is a caller; both are
  `__notifyevents__` declarations.

There is no `GetState`, no `GetBallpark` on the remote object, no
`GetInitialState`. `beyonce`'s client-visible surface (§6) has nothing that
returns a blob.

### 5.2 The one thing that looks like a poll is not one, twice over

`Park._UpdateStateRequestThread` (michelle.py 1070):

```python
def _UpdateStateRequestThread(self):
    if self.remoteBallpark is not None:
        self.remoteBallpark.UpdateStateRequest()
```

**The return value is discarded.** Whatever a real `beyonce` answers to
`UpdateStateRequest`, the client throws it away. It is a *trigger*, and the
state still has to come back by another route.

And it is not called on the happy path. Its only caller is
`Park.RequestReset` (1074), whose only caller in the entire client tree is
`Park.RealFlushState` line 1569 — the **desync recovery** branch, reached when
`SynchroniseToSimulationTime` fails on an already-running simulation.
`Michelle.RequestReset` (427) has no caller at all. A client that has never
received a state will never call it.

The same is true of the bind. `Moniker.Bind()` returns the fused call's
retval, but `InitializeRemoteBallpark` writes `self.remoteBallpark.Bind()` as
a statement and the fused call is `None` anyway — verified on the wire:
`[328] beyonce.MachoBindObject(30000001, None)`.

### 5.3 The client stops talking, and its keep-alive is four minutes

From `captures/1792431326-0002.evecap`: after the `beyonce` bind at 43.4 s,
the only client→server frames in the remaining 19 s are `alert.BeanCount`,
`alert.SendClientStackTraceAlert` and `eventLog.LogClientStats`. The **last
`PingReq` in the whole session was at 14.3 s** — 48 seconds before the
capture ended.

`machoNet.__KATLoop` explains it: `clientKeepAliveTimerInterval = 240`, the
loop wakes every 60 s, and it only pings a transport whose last ping
*response* is older than 240 s. An idle in-space client sends roughly one
frame every four minutes.

So the tempting shortcut — "queue notifications and flush them on the next
inbound frame, no new thread needed" — does not work. Destiny ticks at 1 Hz
and the client's reads arrive at 1/240 Hz. Even a *single* initial `SetState`
would be delivered up to four minutes after the undock.

### 5.4 The receive path is already waiting for us

`CoreBroadcastStuff.NotifyUp` (`carbon/common/script/net/BroadcastStuffGPCS.py`):

```python
def NotifyUp(self, packet):
    if packet.payload[0]:
        if packet.destination.broadcastID.startswith('Process'):
            sm.ChainEventWithoutTheStars(packet.destination.broadcastID, packet.payload[1])
        elif packet.destination.broadcastID.startswith('Do'):
            sm.SendEventWithoutTheStars(packet.destination.broadcastID, packet.payload[1])
        elif packet.destination.broadcastID == '__MultiEvent':
            ...
        else:
            sm.ScatterEventWithoutTheStars(packet.destination.broadcastID, packet.payload[1])
```

`'DoDestinyUpdate'.startswith('Do')` → `SendEvent`. And
`serviceManager.SendEventWithoutTheStars` iterates
`self.notify['DoDestinyUpdate']`, which `michelle` joined at service start
(`serviceManager.py` ~347, `for notify in srv.__notifyevents__: self.notify[notify].append(srv)`),
and wraps each handler in `try/except Exception: self.LogError(...);
log.LogException()`. **A wrong state blob is a logged traceback, not a hang.**

### 5.5 The exact packet to send

The GPCS chain on the client is
`[ExceptionWrapper, ExceptionMappingGPCS, ObjectCall, ServiceCall,
BroadcastStuff]` (`machoNet.get_gpcs_map()`, the `'client'` entry). Walking
the *down* direction on the server, as the real server would:

```
BroadcastStuff.ScattercastWithoutTheStars('clientID', [clientID],
                                          'DoDestinyUpdate', (state, waitForBubble))
    packet  = Notification(destination=MachoAddress(broadcastID='DoDestinyUpdate',
                                                    idtype='*clientID',
                                                    narrowcast=[clientID]),
                           payload=(1, args))
ServiceCall.NotifyDown:   payload = (0, payload)
ObjectCall.NotifyDown:    payload = (0, ToPickle(payload))
```

which is exactly the form `docs/archive/roadmap.md` already records:
**`payload = (0, pickle((0, (1, args))))`**, where `pickle(x)` is
`blue.marshal.Save(x, GetObjectID)` — the nested marshal stream
`docs/reference/bound-objects.md` §1.6 documents, with `ToPickle`/`FromPickle` already
implemented in `eve-proto`.

On the wire:

| field | value |
|---|---|
| command | **12** (`macho.Notification`, guid `carbon.common.script.net.machoNetPacket.Notification`) |
| `destination` | `MachoAddress` BROADCAST: `(4, 'DoDestinyUpdate', [clientID], '*clientID')` |
| `source` | ours, as for any other packet |
| `payload` | `(0, <marshal stream of (0, (1, (state, waitForBubble)))>)` |

`eve-proto` already has `MachoNetMsg::Notification` (`packet.rs:283`) and
`MachoAddress::Broadcast` with the `(4, broadcastID, narrowcast, idtype)`
state (`address.rs:33`). The envelope costs us almost nothing.

Three details that remove risk, all read off the client:

- **The destination is not validated.** On the client,
  `machoNetTransport._AssociateWithSession` takes the `macho.mode == 'client'`
  branch and sets `nodeID = 0`, `sess = session`, unconditionally — the
  destination address is never consulted. The narrowcast filtering happens on
  the real cluster's *proxy*, which is us. So the `clientID` and `idtype` we
  put in the address are cosmetic for a single-client server. Match the real
  server anyway; it costs one tuple.
- **The channel is the default one.** `macho.packetTypeChannelMap` maps only
  the two session-change commands to the `'sessionchange'` chain; command 12
  falls to `channelHandlersUp[None]`, the main chain.
- **`ServiceCall.NotifyUp` passes it through.** With `payload[0] == 0` it
  unwraps to `payload[1]` and forwards; the broadcast layer above sees
  `(1, args)` and dispatches. Our packet never needs a `service` field.

### 5.6 What that costs us architecturally — less than it sounds

`conn::serve` (`eve-server/src/conn.rs`) writes only from `dispatch.greeting()`
and from `dispatch.on_payload`'s `reply.payloads`, and otherwise blocks in
`stream.read`. There is genuinely no push path today.

But the change is small and needs no new dependency:

- `TcpStream::try_clone()` gives a second owned handle to the same socket.
  Hand it to a per-connection *ticker* thread.
- Wrap the write side in a `Mutex` so a notification cannot interleave a
  frame with a `CallRsp`. `send()` already takes `&mut TcpStream` and frames
  one payload at a time, so the lock is around one `write_all`.
- The ticker thread wakes on an interval and writes zero or more
  `Notification`s.

That is one `Arc<Mutex<…>>`, one extra thread per connection, and no change to
the read loop. It is well inside the "only our own code, `std::net`, thread
per connection" constraint in `docs/reference/server.md` §1 — a push thread is *more*
natural in a blocking design than in an async one.

**Do not implement push by piggybacking on reads.** §5.3 is why.

---

## 6. The `beyonce` surface

`beyonce` is reached two ways and both matter.

### 6.1 As a plain service (`sm.RemoteSvc('beyonce')`)

From a constructor-site scan of all modules:

| method | caller | answer shape | status |
|---|---|---|---|
| `GetFormations()` | `michelle.AddBallpark` line 452 | **a tuple** — consumed by native `LoadFormations`. `()` is correct and `[]` is fatal | implemented `[V]` |
| `AdminGetBubbleID(itemID)` | `michelle.AdminGetBubbleID` line 978 | an id; GM-only, never on the undock path | not needed |
| `GetCynoJammerState(...)` | `sovDashboard/cynoJammerIcon.py` | — | not on this path |

### 6.2 The bind, and the bound ballpark object

`eveMoniker.GetBallPark(solarsystemID)` →
`Moniker('beyonce', solarsystemID)` with a session check on
`solarsystemid`. On the wire, in this order, **verified**:

1. `beyonce.MachoResolveObject(30000001)` → a node id (we answer `1`).
2. `beyonce.MachoBindObject(30000001, None)` → `(object, None)`.

**There is no `michelle.Cmd*` layer, and nothing is applied locally.**
`michelle.GetRemotePark()` returns `self.__bp.remoteBallpark` — the bound
object itself, not a wrapper (`michelle.py:511-516`) — and `Michelle` defines
exactly two `Cmd*` methods of its own, both pure forwarders;
`CmdSetSpeedFraction` does not occur in `michelle.pyc` at all **[V]**. So a
command the server answers and does not echo back as a Destiny action leaves
the ship motionless. `docs/lanes/movement.md` §1 is the proof, and §6.2 below
was written before it was known.

The server's own movement layer, as CCP wrote it, is also in the client:
`behaviors/actions/ballparks.pyc` is NPC movement code calling
`myBall.GotoPoint(coord, notificationRange=, callback=)` and
`myBall.StopMovement()`. Right place to look for "what does a server-side
movement layer look like"; wrong place for wire formats, which are
`michelle`'s.

Then calls land on the bound object with no `service` field — only the
objectID string (`docs/reference/bound-objects.md` §1.6). Methods the client can
invoke on it, from a scan of `GetRemotePark()` / `remoteBallpark` call sites
(false positives such as `session.IsItSafe` removed by hand):

| method | caller | when |
|---|---|---|
| `UpdateStateRequest()` | `Park._UpdateStateRequestThread` | **desync recovery only** (§5.2). Retval discarded — answer `None`. |
| `GetFleetState()` | `michelle.GetFleetState` | only if `session.fleetid` is set |
| `GetRelativity(x, y, z, simTick, wallTime, wallTimeNow)` | `michelle.GetRelativity` | a developer command; must return a **7-tuple** `(diff, x, y, z, sTime, tTime, rTime)` — it is unpacked immediately |
| `CmdSetSpeedFraction(f)` | `eveCommands`, `autopilot` | player throttle |
| `CmdStop()` | `eveCommands` | player |
| `CmdGotoDirection(...)` | `scanSvc` | player |
| `CmdSteerDirection(...)` | `flightControls` | player |
| `CmdOrbit(...)`, `CmdFollowBall(...)` | `menuSvcExtras/movementFunctions` | player |
| `CmdWarpToStuff(subject, subjectID, **kw)` | `michelle.CmdWarpToStuff` | player; `kw` may carry `fleetFormationSettings` |
| `CmdWarpToStuffAutopilot(...)`, `CmdStargateJump(...)` | `autopilot` | player |
| `CmdGotoBookmark(...)` | `addressBookEntries` | player |
| `CmdAbandonLoot([ids])` | `menuSvcExtras/menuFunctions` | player |
| `CmdFleetTagTarget(...)` | `menusvc` | player |
| `CallComponentFromClient(...)` | `spacecomponents/*` (14 sites) | structures, traders |
| `GetOperationSpawnpointPosition(siteID)` | `michelle.CanWarpToOperationSite` | wrapped in `try/except → False` |
| `GetContainerExpiryDate(...)` | `infosvc` | |
| `GetCapacityOfSiphon(...)` | `siphonBracket` | |
| `GetFleetLocationAndInfo(...)` | `fleetSvc` | |
| `BookmarkScanResult(...)` | `bookmarkSvc` | |

**During and shortly after undock, the client calls exactly three of these:**
`GetFormations`, `MachoResolveObject`, `MachoBindObject`. Everything else is
player-initiated or fleet/structure. The `Cmd*` family is the second
milestone, not the first: nothing calls them until the player presses
something, and they are what turn a static ship into a moving one.

A note for when we get there: `Cmd*` methods are *non-blocking* on the
client. `MachoObjectCallWrapper` sends a `Notification`, not a `CallReq`, for
`blocking == 0` objects, and the client "expects no response at all"
(`docs/reference/bound-objects.md` §1.6). The visible consequence of a `CmdSetSpeedFraction`
is not its return value — it is the `SetSpeedFraction` Destiny action we push
back.

---

## 7. Appendices

### 7.1 `DstBallMode`

From the name/doc pairs in `_destiny.dll` `.rdata` at `0xa2cc8`..`0xa2f68`, in
table order:

| value | name | doc |
|---|---|---|
| 0 | `DSTBALL_GOTO` | Makes ball attempt to reach the goto point |
| 1 | `DSTBALL_FOLLOW` | Makes ball follow another ball |
| 2 | `DSTBALL_STOP` | Brings ball to a stop |
| 3 | `DSTBALL_WARP` | Moves ball very fast to another location |
| 4 | `DSTBALL_ORBIT` | Orbits another ball at some given range |
| 5 | `DSTBALL_MISSILE` | Missile tracking a target |
| 6 | `DSTBALL_MUSHROOM` | Expanding gravity wall |
| 7 | `DSTBALL_BOID` | Swarm like behavior |
| 8 | `DSTBALL_TROLL` | Free ball that will become fixed after awhile |
| 9 | `DSTBALL_FIELD` | Force field ball |
| 10 | `DSTBALL_MINIBALL` | Ball flagged as a miniball |
| 11 | `DSTBALL_RIGID` | A ball that will never move |
| 12 | `DSTBALL_FORMATION` | A ball part of a formation |

The numbering is `[I]` from table order but cross-checked three ways, which
is why it is in the verified list in §0:

- `michelle.py` module level: `DESTINY_MODE_ORBIT = 4`.
- `0x1800979b9`: `cmp eax, 0xc; ja` — modes are bounded at 12.
- `0x18009799f`: `mov ecx, 0x1032` and `bt ecx, eax` selects the modes that
  need a `followId` fixup — bits 1, 4, 5, 12 = FOLLOW, ORBIT, MISSILE,
  FORMATION. Exactly the four that follow something.
- `0x180098b1d`: `cmp byte ptr [rbp-0x7c], 0x0b` — mode 11 (RIGID, "never
  moves") skips the dynamics block. Consistent.

Also exported: `DSTNORMALCLOAK`, `DSTRESTORECLOAK`, `DSTGMCLOAK`,
`DSTLOCALBALLS` ("Starting ID for local ball range" — `michelle` tests
`slimItem.itemID > destiny.DSTLOCALBALLS` to tell real balls from
client-side ones, and seeds client balls at `-8000000000000000000`).

### 7.2 `destiny.Ball` attributes

The Python-visible attribute set, in table order with the docstrings, from
`_destiny.dll` at `0xa4fd4`..`0xa56f0`. This is the field list a state blob
ultimately has to populate:

```
id              id of ball
mass            Mass of ball (kg)
radius          Radius of ball (m)
maxVelocity     Maximum velocity of ball (m/s)
maxAngularVelocity  Maximum angular velocity of ball (rad/s)
isFree          True if ball is free to move
isGlobal        True if ball should be visible from all
isMassive       True if ball is solid
isInteractive   True if ball is interactive
isCloaked       True if ball is cloaked
isMoribund      True if ball is moribund
harmonic        Shield harmonic value of ball
corporationID   corporation ID of ball
allianceID      alliance ID of ball
x, y, z         position (m)
vx, vy, vz      velocity (m/s)
                angular velocity (rad/s), 3 components
                rotation quaternion, x/y/z/w
yaw, pitch, roll
Agility             Agility modifier of ship
RotationalAgility   Rotational agility modifier of ship
speedFraction   Cruise velocity as a fraction of maximum velocity
mode            Dynamic mode of ball
gotoX, gotoY, gotoZ   destination (m)
followId
followRange     range of follow
ownerId
effectStamp     time stamp of effect
newBubbleId     id of bubble ball currently belongs to
oldBubbleId     id of bubble ball used to belong to
formationID     id of current formation
miniBalls, miniCapsules, miniBoxes
ballpark        The ballpark
```

Native `Ballpark` methods of interest: `AddBall`, `AddMiniBall`, `RemoveBall`,
`CloakBall`, `UncloakBall`, `FollowBall`, `FormationFollow`,
`SetBall{Position,Velocity,Rotation,AngularVelocity,Mass,Radius,Agility,AngularAgility,Free,Global,Massive,Interactive,Rigid,Harmonic,Troll,Formation}`,
`ReserveFormationSlot`, `FreeFormationSlot`, `LoadFormations`,
`SetBallNotInParkCallback`, `ReadFullStateFromStream`,
`WriteFullStateToStream`, `GetBallIdsInRange`, `GetBallIdsInCone`,
`GetBallIdsInCapsule`.

### 7.3 Useful inputs to any future run

- `tools/evidence.sh --calls` then `--alerts` — §5 of this doc was settled by
  the former; §1's traceback by the latter.
- A repeat of `Waiting for ballpark N` in the client log means `bpReady` is
  stuck `False`: `AddBallpark` threw. It costs 30 s per caller.
- `grep -o 'UNANSWERED [a-zA-Z_:]*\.[A-Za-z_]*' logs/eve-server.log | sort |
  uniq -c | sort -rn` — unchanged advice; a Destiny-era client will issue
  `Cmd*` calls in bursts and a live lock there would look the same.

---

## 8. What to build, in order

Each step names what it unblocks and the **client-visible signal** that it
worked, so a run either settles it or does not.

### Step 1 — land the empty tuple (in flight)

`beyonce.GetFormations() -> ()`. Already in `service.rs`.

*Unblocks:* everything below — `AddBallpark` cannot complete without it.
*Signal:* `Done adding ballpark 30000001` in the client log, and **no**
`Waiting for ballpark` afterwards. The space view is already drawing by then
(§1), so the player should see a starfield and an empty HUD rather than a
black screen.

### Step 2 — get a reference state blob out of the client

Before writing a single serialiser byte. Drop a loose `.py` override that, at
a convenient moment, builds a `destiny.Ballpark`, `AddBall`s one ball with
values we choose, `WriteFullStateToStream`s into a `blue.MemStream` and dumps
the bytes to `C:\eve-logs\`. Then vary one field and dump again.

*Unblocks:* §4.4's `[I]` rows, and it does so **by construction** — the bytes
come from the exact code that will parse ours. Also gives a fixture to pin a
Rust serialiser against, in the spirit of `eve-inflate`'s reference vectors.
*Signal:* two dumps that differ in exactly the bytes we changed.
*Risk:* `blue.MemStream`'s read-back API is `[I]`; `ms.data` is the likely
accessor, with `ms.Seek(0); ms.Read(ms.Size)` as the fallback. Both are cheap
to try. If neither works, fall back to finishing `0x1800989e0` by hand —
~2.5 KB of mode-specific disassembly.

### Step 3 — the push path

`TcpStream::try_clone()`, a `Mutex` around the write side, one ticker thread
per connection, and a queue the dispatcher can post to. No notification
content yet — send nothing and prove the plumbing does not corrupt a reply.

*Unblocks:* every server-initiated message this project will ever need —
Destiny, `OnMultiEvent`, chat, fleet, and the multi-player work in
`docs/reference/world-state.md`. It is the single prerequisite named at the top of
`docs/archive/roadmap.md`'s reordering table.
*Signal:* a full login, undock and quit with the ticker running and
`evecap --calls` showing the same 150/150 answered, no framing errors, no
`ValueError` in the client log.
*Do this before step 4,* not after: a state blob with nowhere to go cannot be
tested, and a push path with nothing to push can.

### Step 4 — one `SetState` with one ball

Push `('SetState', (bag,))` at the current tick, with `bag` per §4.3: the
blob from step 2 holding one ball whose id is `session.shipid` (100000001),
`ego = session.shipid`, one `foo.SlimItem` with `itemID`/`typeID`/`ownerID`,
and the empty-but-correct containers.

*Unblocks:* the ship on screen.
*Signal, in order, in the client log:*
`Michelle received a SetState at time <t>` → `State history rebased at <t>` →
`Ego-wannabe 100000001 added to park` → `Post-ego change: 0 -> 100000001`,
and on screen a ship with the camera on it. Followed on the wire by a prime
wave — `config.GetMultiOwnersEx`, `GetMultiCorpTickerNamesEx`,
`GetMultiLocationsEx` — which we already answer.
*Failure modes to expect, all self-reporting:* `RuntimeError('BallNotInPark',
…)` means `slims` names an id the blob did not create; a traceback out of
`SendEvent` means the `bag` is short an attribute; a silent nothing with
`Events ignored` in the log means `validState` never became true, i.e. the
native parser rejected the blob.

### Step 5 — the tick

Push a `DoDestinyUpdate` every second carrying the actions that changed. At
minimum: nothing, which is legal and keeps `currentTime` advancing on our
side only. The client's own ballpark evolves locally between updates — that
is the point of Destiny — so a static ship needs no per-tick traffic at all.

*Unblocks:* movement, and the desync machinery becoming meaningful.
*Signal:* no `OnFatalDesync` / `OnRecoverableDesync`, no
`'Michelle discarded a state that was too old'`, no `RequestReset`.

### Step 6 — `UpdateStateRequest` and the recovery loop

Answer the bound-object call with `None` and respond by pushing a fresh
`SetState`. Until step 5 is solid this will fire; after it, it should never
fire, and seeing it is the alarm that says our tick stamps have drifted.

*Signal:* the call appearing at all. It is a regression detector, not a
feature.

### Step 7 — `Cmd*` and the first movement

`CmdSetSpeedFraction`, `CmdStop`, `CmdGotoDirection`, `CmdOrbit`. These
arrive as non-blocking bound-object **notifications** (§6.2): there is no
reply to get wrong. Each one becomes one or more Destiny actions pushed back
— `SetSpeedFraction`, `Stop`, `GotoDirection`, `Orbit` — all of which are in
the `scatterEvents` or native-passthrough sets and therefore go straight to
`_destiny.dll`.

*Unblocks:* M3 in `docs/archive/roadmap.md`, and the first thing in this project a second
player could watch happen.
*Signal:* the ship moves, the speed gauge tracks, and
`sm.ScatterEvent('OnBallparkCall', 'SetSpeedFraction', …)` fires.

---

## See also

- [`docs/guides/debugging-the-client.md`](../guides/debugging-the-client.md) — the instruments, the
  `[V]`/`[I]` convention, and the five kinds of empty.
- [`docs/reference/bound-objects.md`](../reference/bound-objects.md) §1.6 — the `CallReq`/`Notification`
  payload discriminator and `ToPickle`, which §5.5 depends on.
- [`docs/archive/wire-format.md`](wire-format.md) — command 12, and the BROADCAST
  `MachoAddress` state tuple.
- [`docs/reference/world-state.md`](../reference/world-state.md), [`docs/archive/roadmap.md`](roadmap.md) — why the
  push path in step 3 is a foundation rather than a feature.
