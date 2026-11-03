<!-- Provenance: the `targeting` research lane, 2026-10-22. Read-only with
     respect to code: no Rust was written or changed, no cargo was run, no
     gate was run, nothing was committed. Two files were written: this one
     and `docs/lanes/targeting.md`.

     Every client line number below came from `tools/dis.py` (`co_lnotab`),
     the numbers the client's own tracebacks print -- never from uncompyle6's
     reconstructed output. Decompiled text is quoted for readability where it
     is faithful, but the citation is always the `dis.py` line.

     Marks are the house convention: **[V]** read from this build's bytecode
     or from this tree's code; **[V2]** a third-party decode of a different
     build; **[I]** inferred, with the inference stated; **[A]** an
     assumption we are choosing to ship, each one carrying what would
     falsify it. Nothing here has been run against a client. -->

# Target locking — implementation spec

Client build **V24.01 / 3619879**. Citations are `<client path>.py:LINE`, the
form the client's tracebacks use; on disk that is `out/pyc/<same path>.pyc`.

Read `docs/reference/push-path.md` §2.3 first if you have not: it is the rule that makes
§1 below work, because every message in this feature is an `On*` broadcast and
nothing else.

---

## 0. The headline, before anything else

**A lock is confirmed by the return value of one remote call. There is no
mandatory push, no destiny action, and no second message.** [V]

```
dogmaIM.AddTarget(targetID)  ->  (0, [])     # the client draws the bracket itself
```

`targetMgr._LockTarget` (`targetMgr.py:1586`) is six lines of bytecode:

```python
self.StartLockTarget(tid, autotargeting)              # 1587
flag, targetList = self.GetDogmaLM().AddTarget(tid)   # 1589  UNPACK_SEQUENCE 2
if not flag:                                          # 1590
    self.OnTargetAdded(tid)                           # 1592
```

`OnTargetAdded` (`targetMgr.py:674`) is the whole visible effect of a lock: it
stops the locking sound, sets `state.targeted`, builds the `TargetInBar`
widget, turns on the bracket's `targeted_indicator`, and makes the target
active if nothing else is. **The server does not have to send anything for
that to happen.** `targetList`, the second element, is stored and never read
again — one `STORE_FAST`, no `LOAD_FAST` [V] — so `[]` is a complete answer.

So the smallest correct lock is **one method on a bound object we already
serve**, returning a two-element sequence whose first element is falsy. That
is a much smaller feature than "combat gateway" suggests.

**The asymmetry, and it is the thing to get right tonight: unlock is *not*
return-confirmed.** `targetMgr.UnlockTarget` (`targetMgr.py:1611`) calls
`dogmaIM.RemoveTarget(tid)` and then `RemoveFromTeam(tid)`, and
`RemoveFromTeam` (`targetMgr.py:420`) opens with `if self.IsObserving():` —
false for any ordinary pilot, so **it does nothing at all** [V]. Nothing
client-side removes the bracket. Without a push the player clicks "unlock",
the server forgets the lock, and the target box stays on screen for the rest
of the session. That is the ghost.

---

## 1. Minimum viable lock — the ordered exchange

Alice is in space, holding Bob's ball (§27 of `docs/port-tracker.md`). She
ctrl-clicks Bob.

| # | direction | message | shape |
|---|---|---|---|
| 1 | — | client UI | `targetMgr.TryLockTarget(bobShipID)` (`targetMgr.py:1513`). Four local guards, none of them a range or count check (§6). |
| 2 | — | client, local | `StartLockTarget` (`targetMgr.py:1476`): starts the `TargetLocking` sound loop, scatters `OnTargetLockStarted`, sets `state.targeting=1` on Bob's bracket, records `self.targeting[bobShipID] = blue.os.GetSimTime()`. **The acquisition animation begins here, with no server involvement.** |
| 3 | C→S | **`dogmaIM` bound-object call**, method **`AddTarget`**, args `(bobShipID,)` | see §2 |
| 4 | S→C | **the `CallRsp`**, value `(0, [])` | see §3. Falsy first element. |
| 5 | — | client, local | `OnTargetAdded(bobShipID)` (`targetMgr.py:674`) — bracket drawn, box in the target bar, sound stops, `OnClientEvent_LockItem` scattered. **Done.** |
| 6 | S→Bob | **`OnTarget` notification**, args `('otheradd', aliceShipID)` | §8. Optional for Alice's bracket; required for Bob to see "you are being targeted". |

And the unlock, which needs the push:

| # | direction | message | shape |
|---|---|---|---|
| 1 | — | client UI | `targetMgr.UnlockTarget(bobShipID)` (`targetMgr.py:1611`) |
| 2 | C→S | `dogmaIM.RemoveTarget(bobShipID)` | return value discarded (`POP_TOP` at `targetMgr.py:1613`) [V] |
| 3 | S→Alice | **`OnTarget` notification**, args `('lost', bobShipID, 'Unlocked')` | `OnTarget` → `uthread.new(self.OnTargetLost, tid, reason)` (`targetMgr.py:660`) → `ClearTarget` (`targetMgr.py:1422`): box closed, `OnClientEvent_UnlockItem` scattered |
| 4 | S→Bob | `OnTarget` args `('otherlost', aliceShipID, 'Unlocked')` | clears Bob's `threatTargetsMe` and his `targetedBy` list |

That is the whole feature. Six messages, one of them a return value, and all
four pushes are the **same** notification name with a different first
argument.

### 1.1 If you want the acquisition delay to be real

Return `(1, [])` from `AddTarget` instead, and push `OnTarget('add', tid)`
after the delay. The client's bar is already running (step 2) and will reach
100% on its own schedule; the push is what turns the bar into a locked box. The
two clocks do not have to agree — see §4.3 for what the player sees when they
disagree, and why `(0, [])` is still the right call for tonight.

---

## 2. The lock call

### 2.1 Service, bind, method

**It is not `beyonce`, not `tactical`, and not a ship object.** It is the
`dogmaIM` bound object the client already holds, re-bound to the solar system.

```python
# eveMoniker.py:63  CharGetDogmaLocation
def CharGetDogmaLocation():
    moniker = Moniker('dogmaIM', GetLocationBindParams())
    moniker.SetSessionCheck(GetLocationSessionCheck())
    return moniker

# eveMoniker.py:37  GetLocationBindParams
if session.solarsystemid:  return (session.solarsystemid, const.groupSolarSystem)   # 5
if session.stationid:      return (session.stationid,     const.groupStation)       # 15
```

**[V]** `groupSolarSystem = 5`, `groupStation = 15` (`inventorycommon/const.py:955,971`).

The chain from `targetMgr` to the wire, each step **[V]**:

| step | file:line |
|---|---|
| `targetMgr.GetDogmaLM()` → `self.godma.GetDogmaLM()` | `targetMgr.py:497` |
| `Godma.GetDogmaLM()` → `self.GetStateManager().GetDogmaLM()` | `godma.py:379` |
| `StateManager.GetDogmaLM()` → `eveMoniker.CharGetDogmaLocation()`, memoised in `self.dogmaLM` | `godma.py:1321` |

`targetMgr.Reset` (`targetMgr.py:356`) and `CleanUp` (`targetMgr.py:577`) both
set `self.dogmaLM = None`, so the moniker is re-minted — and therefore
**re-bound against the new bind params** — after every session change. We
already serve the solar-system re-bind: `service/inventory.rs:747` says so in
as many words.

### 2.2 The five methods, with arity from the call sites

Taken from `findings/bound-objects.json` (`services.dogmaIM`), which records
one call site per method with its `CALL_FUNCTION` arg count **[V]**:

| method | args | return | caller |
|---|---|---|---|
| `AddTarget(targetID)` | 1 | **`(flag, targetList)`**, unpacked with `UNPACK_SEQUENCE 2` | `targetMgr._LockTarget`, `targetMgr.py:1589` |
| `RemoveTarget(targetID)` | 1 | discarded | `targetMgr.UnlockTarget`, `targetMgr.py:1613` |
| `RemoveTargets(targetIDs)` | 1 (a list) | discarded | `targetMgr.UnlockTargets`, `targetMgr.py:1619` |
| `CancelAddTarget(targetID)` | 1 | discarded | `targetMgr.CancelAddTarget`, `targetMgr.py:1510` |
| `ClearTargets()` | 0 | discarded | `targetMgr.ClearTargets`, `targetMgr.py:1647` |
| `GetTargets()` | 0 | **iterated** | `godma.RefreshTargets`, `godma.py:2730` |
| `GetTargeters()` | 0 | **iterated** | `godma.RefreshTargets`, `godma.py:2732` |
| `AddTargetOBO(structureID, targetID)` | 2 | `(flag, targetList)` | `playerOwnedTargetMgr.LockTargetOBO` — structures, out of scope tonight |
| `RemoveTargetOBO(structureID, targetID)` | 2 | discarded | same |

**`GetTargets` and `GetTargeters` are already implemented** and answer
`PyValue::List(vec![])` (`service/inventory.rs:752`, tested at
`service.rs:6620-6621`). They are called from `godma.RefreshTargets`
(`godma.py:2729`), which `targetMgr.Run` (`targetMgr.py:72`) calls
**synchronously** whenever `session.shipid and not session.stationid`. Once we
hold locks, these two must answer the real lists or every lock vanishes on the
next session change (§5.5).

### 2.3 The subject is the session, not an argument

The client-facing `AddTarget` takes **one** argument. The server's own
internal form takes two, and we know this because the NPC behaviour trees ship
in the client and call it:

```python
# behaviors/actions/combattargets.py:424   AcquireTargetLock._LockTarget
self.context.dogmaLM.AddTargetEx(self.context.myItemId, selectedTarget)
# behaviors/utility/dogmatic.py:61         get_my_locked_targets
return task.context.dogmaLM.GetTargets(task.context.myItemId)
# behaviors/utility/dogmatic.py:191        remove_my_target
task.context.dogmaLM.RemoveTarget(task.context.myItemId, target_id)
```

**[V]** So CCP's own model is `(subjectShipID, targetID)` throughout, and the
bound object strips the subject and takes it from `session.shipid`. **Our
dispatch must do the same**: the subject is `Registry::roster().get(actor)
.ship_id`, never an argument, and a client that sends two arguments is sending
something we do not know how to read.

The ids on this wire are **ball ids**, i.e. `Presence::ship_id` for a player.
Three independent confirmations **[V]**:
`targetMgr.TryLockTarget` compares the argument against `session.shipid`
(`targetMgr.py:1525`); `OnTargetAdded` resolves it with
`bp.GetInvItem(tid)`; `OnTargetByOther` caches
`self.ownerToShipIDCache[slimItem.ownerID] = otherID`
(`targetMgr.py:1342`ff), which only type-checks if `otherID` is a ship and
`ownerID` is its pilot.

### 2.4 The four client-side guards before the call

`targetMgr.TryLockTarget` (`targetMgr.py:1513`), the universal entry point,
returns silently when **[V]**:

1. `itemID in self.GetTargeting()` or `itemID in self.targetsByID` — already
   locking or locked (`:1515`)
2. `GetBallparkRecord(itemID)` is falsy — **the ball must be in the park**
   (`:1518`)
3. `slimItem.groupID in const.nonTargetableGroups` (`:1522`) — that tuple is
   `(groupPlanet 7, groupMoon 8, groupAsteroidBelt 9, groupCosmicAnomaly 885)`
   (`inventorycommon/const.py:2986`) **[V]**. Note what is *not* in it:
   stargates, stations, wrecks and containers are all locally lockable, so the
   server will be asked to lock them.
4. `itemID == session.shipid and session.shipid == session.structureid`
   (`:1525`) — i.e. self-lock is blocked **only** when piloting a structure.
   **A normal pilot can ask to lock their own ship** and the server has to
   refuse it (`DeniedTargetSelf`, §6.4).

There is **no range check and no count check** here. See §6.

---

## 3. The confirmation — the return value, loudly

### 3.1 The three answers `AddTarget` can give

| we return | `UNPACK_SEQUENCE 2` | `if not flag` | player sees |
|---|---|---|---|
| `(0, [])` or `(False, [])` or `(None, [])` | ok | **true** → `OnTargetAdded` | bracket locked **immediately**, no push needed |
| `(1, [])` | ok | false | bar keeps spinning; nothing until we push `OnTarget('add', tid)` |
| `PyValue::None` | **TypeError** | — | `LockTarget`'s bare `except Exception` (`targetMgr.py:1580-1583`) calls `FailLockTarget` and `sys.exc_clear()`. **Silent failure**, no dialog, no log the player sees. |
| a `UserError` raise | — | — | `FailLockTarget` then either silence or a dialog, per §6.4 |

`None` being a silent no-op is the failure mode to avoid: it looks exactly
like a lost packet.

### 3.2 There is no destiny action and no mandatory push

Three independent checks, all **[V]**:

- `out/pyc/ballparkCommon/` contains no occurrence of `target` in any file.
  The shared ball code has no lock concept at all.
- `docs/reference/destiny-reference-blob.md` records every field of the ball tail we
  have dumped and read back; the only `target` in it is `FollowBall`'s
  *followID*, the orbit/approach subject, not a lock.
- `targetMgr` learns about locks through exactly three notify names —
  `OnTarget`, `OnTargets`, `OnTargetClear` (`targetMgr.py:55-57`) — plus the
  ball-lifecycle events it shares with everybody. None of them is a `Do*`
  destiny event.

So **`Park::encode` does not change**, and a lock is invisible to observers in
the park unless we tell them (§8). See §7.

### 3.3 `OnTargets` is not a server push — do not send it

This is the trap. `OnTargets` is registered in `targetMgr.__notifyevents__`
(`targetMgr.py:56`), so it *looks* sendable, but the only thing that produces
it is the client itself, inside godma's batcher:

```python
# godma.py:274  Godma.OnMultiEvent_
dastatedict = {..., 'OnTarget': target, ...}
for ev in events:
    dastatedict.get(ev[0], other).append(ev)
...
if target:
    target = reduce(remDups, target, [target[0]])
    sm.ScatterEvent(*('OnTargets', target))
```

**[V]** `OnMultiEvent` (`godma.py:268`) is the real-server batch push; it
groups `events` by `itemgetter(-1)` and hands `e[0]` of each to
`OnMultiEvent_`, so its wire shape is a list of `(eventTuple, groupingKey)`
pairs. Each `eventTuple` is `('OnTarget', what, tid, reason)` — element 0 is
the event *name*, which is why `targetMgr.OnTargets` does
`self.OnTarget(*each[1:])` (`targetMgr.py:655`).

The shape is confirmed a second time, independently, by a **second
subscriber** to `OnTargets` that indexes the tuple by hand — and its existence
is itself a reason to prefer `OnTarget`: `OnTarget` has exactly one subscriber
in the build, `OnTargets` has two
(`findings/push-events.tsv:2816,2821-2822`) **[V]**.

```python
# nodegraph/client/events/target.py, class TargetExploded
def OnTargets(self, targets):
    for target in targets:
        event     = target[1] if len(target) > 1 else ''
        target_id = target[2] if len(target) > 2 else ''
        reason    = target[3] if len(target) > 3 else ''
        if event == 'lost' and reason == 'Exploding':
            self.invoke(item_id=target_id)
```

**[V]** So: **send `OnTarget` directly.** `targetMgr` is the only subscriber to
it in the whole build (`findings/push-events.tsv:2816`), its prefix is `On`
so `ScatterEventWithoutTheStars` dispatches it on a worker tasklet that is
allowed to block (`docs/reference/push-path.md` §2.3), and `godma` does **not** subscribe
to `OnTarget` (`godma.py:111-136` lists its 22 notify names and `OnTarget` is
not among them) so there is no double delivery. Sending `OnMultiEvent` would
work too and would cost us a nested pickle shape for nothing.

### 3.4 `OnTarget`, the one push this feature needs

```
broadcastID: "OnTarget"
args:        (what, tid, reason)      # 1..3 positional, reason may be omitted
```

`targetMgr.OnTarget` (`targetMgr.py:660`) **[V]**:

| `what` | effect | handler |
|---|---|---|
| `'add'` | lock completes, bracket + target box appear | `OnTargetAdded(tid)`, `:674` |
| `'lost'` | our lock on `tid` ends | `uthread.new(OnTargetLost, tid, reason)`, `:1313` |
| `'clear'` | **all** our locks end | `OnTargetClear()`, `:1399` |
| `'otheradd'` | `tid` has locked **us** | `OnTargetByOther(tid)`, `:1342` |
| `'otherlost'` | `tid` has stopped locking us | `OnTargetByOtherLost(tid, reason)`, `:1381` |
| anything else | nothing, but `ProcessTargetChanged` still chains | — |

Every arm then runs `sm.ChainEvent('ProcessTargetChanged', what, tid, reason)`
unconditionally. The only subscriber is `TurretSvc.ProcessTargetChanged`
(`turret.py:91`), which **discards all three arguments**
(`findings/push-events.tsv:3149`) **[V]** — so no unknown `what` can raise.

`reason` is accepted and **never read** by `OnTargetLost`'s ordinary path or by
`OnTargetByOtherLost` **[V]** — `ClearTarget(self, tid, reason=None)`
(`targetMgr.py:1422`) has no `LOAD_FAST reason` in its body. The single
exception is `reason == 'Exploding'`, which routes to `ClearDeadTarget`
(`targetMgr.py:1327`) and plays the target box's `anim_disappear()` instead of
closing it flat. So: any string is safe; `'Exploding'` is load-bearing.

---

## 4. The progress animation — entirely client-side

**No message carries a lock duration. There is no field for one anywhere.**
[V] The client computes the acquisition time itself, from two dogma
attributes, and animates it on a 50 ms timer with no server involvement.

### 4.1 The formula

`bracketMgr.GetScanSpeed(source, target)` (`bracketMgr.py:1305`), read off the
bytecode and transcribed instruction by instruction **[V]**:

```python
if source is None: source = session.shipid              # :1307-1308
if not source:     return None                          # :1310
myitem = sm.GetService('godma').GetItem(source)          # :1313
scanSpeed = None                                         # :1314
if myitem.scanResolution and target:                     # :1317
    slimItem   = target
    targetitem = sm.GetService('godma').GetType(slimItem.typeID)   # :1319  the TYPE
    radius = targetitem.signatureRadius if targetitem.AttributeExists('signatureRadius') else 0  # :1325-1330
    if radius <= 0.0:
        radius = sm.GetService('michelle').GetBallpark().GetBall(slimItem.itemID).radius  # :1333-1334
        if radius <= 0.0:
            radius = evetypes.GetRadius(targetitem.typeID)         # :1336
            if radius <= 0.0: radius = 1.0                         # :1338
    scanSpeed = 40000000.0 / myitem.scanResolution / math.log(radius + math.sqrt(radius*radius + 1)) ** 2.0  # :1339
if scanSpeed is None:                                    # :1341
    scanSpeed = 2000                                     # :1342  + a LogWarn
return min(scanSpeed, 180000)                            # :1346
```

i.e. **`lockTime_ms = min(180000, 40_000_000 / scanResolution / asinh(signatureRadius)²)`**,
since `ln(r + sqrt(r²+1))` is `asinh(r)`.

Two things in that which matter for us **[V]**:

- `scanResolution` (**attribute 564**) is read off **our own ship's godma
  item**, i.e. the one `capsule_dogma_row` builds.
- `signatureRadius` (**attribute 552**) is read off the **target's `typeID`
  via `godma.GetType`**, not off the target's live dogma item. The client
  already has the whole `typedogma` table in FSD, so **we never have to serve
  the target's attributes for the animation to be right.** The fallback chain
  then reads the ball's `radius`, which we already encode.

### 4.2 The animation loop

`inSpaceBracket.CountDown` (`inSpaceBracket.py:503`) is started from
`Targeting` (`inSpaceBracket.py:1498`), which fires when `state.targeting` is
set — and `targetMgr.StartLockTarget` (`targetMgr.py:1476`) sets it. The loop
reads the start time back with `targetMgr.GetTargetingStartTime`
(`targetMgr.py:1468`), which is the one-expression
`return self.targeting.get(targetID, None)`. Tick is
`TARGETTING_UI_UPDATE_RATE = 50` ms; the two half-circle sprites get
`rotationSecondary` set from `ratio = (time - dt) / time`; it breaks out with
the `UI/Inflight/Brackets/TargetLocked` label when `dt > time`.

`bracketMgr.OnAttribute` (`bracketMgr.py:1045`) forwards an attribute change
into the loop **only** when `item.itemID == session.shipid and attributeName ==
'scanResolution'` **[V]**, in which case the loop rebases its start time and
rescales mid-lock. So attribute 564 is the only live input; everything else is
fixed for the duration.

### 4.3 What this means for us, and why `(0, [])` is still right tonight

The bar runs on the client's own clock whatever we do. Three cases:

- **We return `(0, [])`.** `OnTargetAdded` runs in the same tasklet, deletes
  `self.targeting[tid]`, and the box appears. The `CountDown` tasklet finds the
  start time gone on its next 50 ms tick and stops. The player sees the reticule
  flash and the lock complete at once. **Not EVE-authentic, but correct and
  complete, and it is the smaller change.**
- **We return `(1, [])` and push after `lockTime_ms`.** Authentic. We must
  compute `lockTime_ms` ourselves with the formula above, which needs
  `scanResolution` for the actor's hull and `signatureRadius` for the target's
  type — both static per type, so a table beside `destiny::hull` would do it.
  If our number is **shorter** than the client's, the bar is still spinning when
  the box appears (harmless). If it is **longer**, the bar says "Target Locked"
  and sits there until the push arrives (looks broken).
- **We return `(1, [])` and never push.** The bar completes, says "Target
  Locked", and nothing ever happens. Permanent stuck reticule. This is the
  failure mode that makes `(1, …)` the riskier default.

`[A]` We assume `flag` means "the lock is pending, wait for the notification"
rather than something narrower such as "the target was already locked". Both
readings produce the same server behaviour for us because the client's only use
of `flag` is `if not flag`. **Falsified if** returning `(0, [])` produces
anything other than an immediately drawn bracket — that is one loopback
assertion plus one human click.

---

## 5. Unlock, and every way a lock ends

The client is **self-healing for ball removal and for its own session
changes**, and **blind to everything else**. That split is the whole of this
section, and it is what decides which events need a push.

### 5.1 What the client cleans up on its own [V]

| trigger | handler | effect |
|---|---|---|
| a ball leaves the park (`RemoveBalls`) | `DoBallRemove(ball, slimItem, terminal)`, `targetMgr.py:639` | `ClearTarget(ball.id)` — **the target box and bracket go away by themselves** |
| the park is released (`Park.Release`, i.e. we dock or jump) | `DoBallsRemove(pythonBalls, isRelease=True)`, `targetMgr.py:613` | closes every target box, drops `targetsByID`, `pendingTargets`, `rowDict` |
| `DoBallClear(solitem)`, `targetMgr.py:651` | → `CleanUp`, `targetMgr.py:577` | clears targets **and** `targetedBy`, and nulls `self.dogmaLM` |
| we dock / disconnect / lose our character | `ProcessSessionChange`, `targetMgr.py:545` → `CleanUp` when `not connection.IsConnected() or session.stationid is not None or session.charid is None` | as above |
| we change ship in space (`'ship' in change and 'solarsystemid' not in change`) | `ProcessSessionChange` → `OnTargetClear()` and `self.pendingTargeters = []` | all our locks drop; `targetedBy` is **not** cleared |

**So: when a locked target leaves the grid, we do not have to push anything to
the locker.** Removing the ball — which §27 already does with
`destiny::BallsRemoved` — removes the box. We still have to forget the lock
server-side, or `GetTargets()` will resurrect it on the next session change
(§5.5).

### 5.2 What the client is blind to — the three ghosts

**Ghost 1: the player's own unlock.** `UnlockTarget` (`targetMgr.py:1611`)
calls the server and then `RemoveFromTeam`, which is a no-op outside spectator
mode (`targetMgr.py:420-427`) **[V]**. **Must push `OnTarget('lost', tid,
reason)`.**

**Ghost 2: `ClearTargets` and `UnlockTargets`.** `ClearTargets`
(`targetMgr.py:1646`) is a bare `self.GetDogmaLM().ClearTargets()` with no
local effect at all; `UnlockTargets` (`targetMgr.py:1616`) calls
`RemoveTargets(list)` and then `RemoveFromTeam` per id — again a no-op.
**Must push `OnTarget('clear')` (or one `'lost'` per id).** `OnTargetClear`
takes an optional `targetsToSkip` (`targetMgr.py:1399`), so
`('clear',)` with one element is a valid args tuple.

**Ghost 3 — the nasty one: `targetedBy` is never cleared by ball removal.**
`DoBallRemove` and `DoBallsRemove(isRelease=False)` touch only `targetsByID`
**[V]**; `self.targetedBy` is cleared only by `Reset`, `CleanUp` and
`ClearTargetedBy` (`targetMgr.py:1685`). So if Alice has locked Bob and then
Alice docks, warps off, or dies, **Bob keeps `state.threatTargetsMe` on a
bracket that no longer exists and keeps Alice's id in `targetedBy` forever.**
`IsTargeted()` (`targetMgr.py:1414`) stays true, and
`OnClientEvent_TargetedByChanged` never fires again for her.
**Must push `OnTarget('otherlost', aliceShipID, reason)` to Bob.**

Worse: `OnTargetByOther` (`targetMgr.py:1342`) opens with
`if otherID in self.targetedBy: self.LogError('already targeted by', otherID)
else: self.AddTargetedBy(otherID)` **[V]** — so a stale entry makes Alice's
*next* lock on Bob log an error and silently skip the indicator. The ghost is
self-perpetuating.

### 5.3 The table the implementation needs

Audience is a character id list (`notify::Push::audience`). "Locker" = the
character who holds the lock; "victim" = the character being locked.

| event | to locker | to victim | also |
|---|---|---|---|
| locker asks to unlock (`RemoveTarget`) | `OnTarget('lost', victimShip, 'Unlocked')` | `OnTarget('otherlost', lockerShip, 'Unlocked')` | — |
| locker asks to clear all (`ClearTargets` / `RemoveTargets`) | `OnTarget('clear')` | one `otherlost` per locker→victim pair dropped | — |
| **victim dies** | `OnTarget('lost', victimShip, 'Exploding')` | — | `'Exploding'` routes to `ClearDeadTarget`, `targetMgr.py:1327`, which plays `anim_disappear()`. Any other reason closes the box flat. |
| **victim docks / jumps / warps off grid** | nothing *required* — `BallsRemoved` already clears the box | — | but drop the lock server-side, and send `otherlost` to the victim so **their** `targetedBy` is clean before their park is torn down. Order matters: the victim's own `Park.Release` will clear it anyway, so this one is optional. `[I]` |
| **victim goes out of our targeting range** | `OnTarget('lost', victimShip, 'OutOfRange')` | `OnTarget('otherlost', lockerShip, 'OutOfRange')` | **required**: no ball left the park, so nothing self-heals. See §6.3 — this is also the only EVE rule we have to implement ourselves. |
| **locker docks / jumps** | nothing — `ProcessSessionChange`/`Park.Release` clears their own state | `OnTarget('otherlost', lockerShip, reason)` | **required for the victim**, per Ghost 3 |
| **locker changes ship in space** | nothing — `ProcessSessionChange` calls `OnTargetClear()` | `OnTarget('otherlost', oldLockerShip, 'DeniedShipChanged')` | the client already expects `DeniedShipChanged` as a lock-abort sentinel (§6.4) |
| **locker disconnects** | — | `OnTarget('otherlost', lockerShip, reason)` | `Registry::session_closed` already queues pushes; note `docs/lanes/two-in-space.md` §1.3 — a push queued outside a turn waits for *somebody* to send us a payload |

### 5.4 `CancelAddTarget` — an in-progress lock the player aborts

`targetMgr.CancelAddTarget` (`targetMgr.py:1505`) does the **whole** client
side itself — clears `state.targeted` and `state.targeting`, stops the sound,
scatters `OnFailLockTarget` — and *then* calls
`dogmaIM.CancelAddTarget(tid)` **[V]**. So the server only has to drop the
pending lock and must send **nothing**. Reached from the right-click
"Cancel lock target" entry (`menusvc.CancelLockTarget`) and from the nodegraph
`UnlockTarget` action, which calls `CancelAddTarget` then `UnlockTarget`.

With `AddTarget` returning `(0, [])` there is no pending state on our side and
this call is a no-op — but it must still **answer**, not raise `NoSuchMethod`.

### 5.5 The state that survives a session change: `GetTargets` / `GetTargeters`

`targetMgr.Run` (`targetMgr.py:72`) calls
`self.godma.GetStateManager().RefreshTargets()` **synchronously** whenever
`session.shipid and not session.stationid`, and `RefreshTargets`
(`godma.py:2729`) is **[V]**:

```python
for objectID in self.GetDogmaLM().GetTargets():
    sm.ScatterEvent('OnTarget', 'add', objectID)
for objectID in self.GetDogmaLM().GetTargeters():
    sm.ScatterEvent('OnTarget', 'otheradd', objectID)
```

So these two calls are how a lock **survives a reload, a view reopen, or a
reconnect**, and they are the client's only way to rediscover who is targeting
it. They are already wired to `PyValue::List(vec![])`
(`service/inventory.rs:752`); once we hold locks they must answer the real
lists, keyed on the acting character's ship.

`service/inventory.rs:747` already records why these two must never raise: the
`target` service is one of `SpaceView.__dependencies__`, started with
`StartServiceAndWaitForRunningState` **outside any try block**, so a raise here
aborts `_OpenView` and the space view never appears at all.

---

## 6. The limits

### 6.1 The attribute ids, with their FSD defaults

Resolved from `out/pyc/dogma/const.pyc` and cross-checked against the shipped
`dogmaattributes.fsdbinary` with `tools/dogma.py attributes` **[V]**:

| name | attributeID | FSD `defaultValue` | read where |
|---|---|---|---|
| `maxTargetRange` | **76** | **0** | `targetMgr.GetMaxTargetRange`, `:1551`; `celestialCheckers.IsWithinTargettingRange`; `bracketMgr.ShowInTargetRange` |
| `maxLockedTargets` | **192** | **0** | `targetMgr.GetNumAdditionalTargetsAllowed`, `:1665`; `OnTargetByOther`, `:1365` |
| `scanResolution` | **564** | **0** | `bracketMgr.GetScanSpeed`, `:1317,1339` |
| `signatureRadius` | **552** | **100** | `bracketMgr.GetScanSpeed`, `:1326` — off the **type**, so FSD already answers it |

`maxLockedTargetsBonus` 235, `maxTargetRangeMultiplier` 237,
`maxTargetRangeBonus` 309 and the rest of the bonus family exist but **no
client code reads them** [V] — they are server-side stacking inputs. We do not
need them.

`maxLockedTargetsFromSkills` **does not exist in this build** [V]: the name is
absent from `dogma/const.pyc` and from all 12,774 `.pyc` files.

### 6.2 `GetAttribute` falls back to the FSD default, so nothing raises

`StateManager.GetAttribute` (`godma.py:2186`) ends its lookup chain with
`return self.attributesByName[attrKey].defaultValue` (`godma.py:2234`), and
`attributesByName` is `dogma.data.get_all_attributes_by_name()` — **the
client's own shipped table** [V]. So omitting 76, 192 or 564 is never an
`AttributeError`; it is silently **zero**, and zero is worse than wrong:

- **76 = 0** → `IsWithinTargettingRange` is `dist < 0`, always false, so
  `celestialCheckers.OfferLockTarget` hides the right-click **"Lock target"**
  entry with the hint `UI/Menusvc/MenuHints/NotInTargetRange`. The Ctrl+click
  path still works, so this reads to a tester as "the menu item is missing".
- **192 = 0** → `GetNumAdditionalTargetsAllowed()` returns `-n`, and
  `navigation._FilterBallsByCombatCmd` does `balls[:int(maxTargets)]`, i.e.
  `balls[:0]` — the **marquee mass-lock silently selects nothing**.
- **564 = 0** → `GetScanSpeed` returns the 2000 ms default and logs a warning;
  the bar still animates. Harmless.

**This is almost certainly the first thing a human tester will hit, and it has
nothing to do with the lock itself.** `ship_attributes` (`service.rs:4040`)
currently sends ten attribute ids and none of these three.

### 6.3 Real values, and the capsule trap

From the shipped `typedogma` table (`tools/dogma.py type <id>`) **[V]**:

| type | 76 `maxTargetRange` | 192 `maxLockedTargets` | 564 `scanResolution` | 552 `signatureRadius` |
|---|---|---|---|---|
| 587 Rifter | 22 500 | 4 | 660 | 35 |
| 596 Impairor | 25 500 | 3 | 485 | 50 |
| **670 Capsule** | **0** | **0** | **absent** | 25 |

**The capsule genuinely cannot lock anything in real EVE**, and every
character in our sandbox is sitting in one (`Registry::new` seeds the capsule;
`capsule_dogma_row`). So a tester in a pod will find the right-click Lock entry
missing and the marquee lock inert, and **both are CCP's own data, not our
bug**. Either put the tester in a real hull or give the pod non-zero 76/192/564
and say so in the test note.

For a Rifter the formula gives
`40e6 / 660 / asinh(35)² = 40e6 / 660 / 4.2525² ≈ 3351 ms` against another
Rifter — about three and a half seconds, which is the right order for EVE.

### 6.4 Does the client enforce the limits? **No. Both are ours.**

**[V]** `TryLockTarget` (`targetMgr.py:1513`) has four guards and none of them
is a count or a range check (§2.4). `IsInTargetingRange`
(`targetMgr.py:1558`) is **defined and never called from the lock path** — its
only two referencing files in the build are `targetMgr.pyc` itself and
`fleetBroadcastCont.pyc` [V]. The client-side range logic is purely cosmetic:
it hides a menu entry (`celestialCheckers.OfferLockTarget`) and shows a
"can target" sprite on a 1000 ms timer (`bracketMgr.ShowInTargetRange`,
`bracketMgr.py:1530`).

`GetNumAdditionalTargetsAllowed` is consulted in exactly one place outside
`targetMgr`: `navigation._FilterBallsByCombatCmd`, truncating a **marquee
multi-lock** selection [V]. A single Ctrl+click is never gated by it.

So: **a permissive server is wire-safe** — the client will happily display
every lock we grant, at any distance, in any number. If we want EVE's rules we
raise a `UserError` from `AddTarget`.

For the comparison, if we do implement range: `IsInTargetingRange` is
`max(0, ball.surfaceDist) < shipItem.maxTargetRange`, strict less-than, on
**surface** distance (`GetDistanceToTarget`, `targetMgr.py:1531,1547`) [V].
Matching that exactly is what keeps the greyed menu entry and the server's
answer agreeing.

### 6.5 The error vocabulary

`_LockTarget`'s handler (`targetMgr.py:1593-1607`) **[V]**:

```python
except UserError as e:
    self.FailLockTarget(tid)                                  # :1594
    if autotargeting: sys.exc_clear(); return                 # :1596-1601
    if e.msg in ('DeniedShipChanged', 'TargetingAttemptCancelled'):   # :1602
        sys.exc_clear(); return                               # :1604-1605
    prompt_player_fsd_dialog(e.msg, e.dict)                   # :1606
    raise                                                     # :1607
```

The `raise` is then swallowed by `LockTarget`'s bare
`except Exception` (`targetMgr.py:1580-1583`), which calls `FailLockTarget`
again. So **any** `UserError` is a clean lock rejection; the key only decides
what the player reads.

| key | what the client does |
|---|---|
| `TargetingAttemptCancelled` | **silent**: `FailLockTarget`, no dialog, no log line. The quiet "no". Exists only as a sentinel string in the bytecode — no FSD dialog row. |
| `DeniedShipChanged` | same, and also the sentinel `LockTargetPassively` and `LockTargetOBO` special-case |
| anything else | `prompt_player_fsd_dialog(e.msg, e.dict)` — an FSD lookup, so an **unknown key produces no text** |

The keys CCP's server uses, with their arg dicts, all present in
`findings/user-errors.tsv` and none of them caught by name in any `.pyc`
**[V]** — so each is a log-line notification formatted from FSD:

| key | args dict | for |
|---|---|---|
| `DeniedTargetSelf` | — | locking own ship |
| `TargetingSystemsAlreadyFullyUtilized` | `limit` (numeric) | ship's 192 exhausted |
| `TargetingAptitudeAlreadyFullyUtilized` | `limit` (numeric) | character's 192 exhausted |
| `DeniedTargetUntargetable` | `targetName` | type cannot be locked |
| `DeniedTargetOtherWarping` / `DeniedTargetSelfWarping` | `targetName` | either ship in warp |
| `DeniedTargetInvulnerable` | — | invulnerability timer |
| `DeniedTargetingAttemptFailed` | `target.name` | the generic server-side failure |
| `UnableToTarget` | — | generic |

`TooManyTargets` and `NotInSpace` **do not exist in this build** [V]. There is
no `DeniedTargetTooFar` either — the out-of-range family is
`TargetNotWithinRange` / `TargetTooFar`, which belong to *module activation*,
not to locking; CCP's lock-range refusal is `DeniedTargetingAttemptFailed`
or nothing at all.

**An unknown key is a silent failure, not a crash.**
`prompt_player_fsd_dialog` (`player_messaging/client/ui_message.py:84`) ends in
`cfg.GetMessage(dialog_key, params, onNotFound='raise')`
(`player_messaging/client/dialog_handler.py:54`, `_handle_player_fsd_dialog`)
**[V]** — so a key we invent raises out of `_LockTarget`'s own `except` block,
which `LockTarget`'s bare `except Exception` then swallows. The lock fails
cleanly and the player is told nothing.

**`CustomNotify` is the escape hatch and it does work here.** It is FSD dialog
766, `dialog_type = notify`, `presentation = logline`, args dict
`{'notify': <text>}` (`findings/user-errors.tsv`) **[V]**, and
`_handle_player_fsd_dialog` routes `type == 'notify'` to
`logger.AddMessage(dialog_data)` plus `message_player(dialog_data.text)` — one
log line carrying our own words, no modal. That is the same pattern
`service/inventory.rs:694` already uses for three other `dogmaIM` methods.

So the house rule for this feature: **`TargetingAttemptCancelled` for a refusal
the player does not need explained** (silent, zero UI noise, and the client
already treats it as a sentinel), and **`CustomNotify` with our own sentence
for anything they do**.

---

## 7. Does targeting touch destiny? **No.**

**A lock is a per-client relationship held in dogma. It is not a ball
property, it is not in the state blob, and no observer in the park learns about
it from `Park::encode`.** [V] Three independent checks:

1. `out/pyc/ballparkCommon/` — the ball code shared by client and server — has
   **no occurrence of the string `target` in any of its seven files** [V].
2. `docs/reference/destiny-reference-blob.md` records every tail field we have dumped
   from our own client and read back; the only `target` in it is `FollowBall`'s
   `followID`, the orbit/approach subject. There is no lock bit, no locker
   list, and no `DSTBALL_*` mode for it.
3. `targetMgr` hears about locks through exactly three notify names
   (`targetMgr.py:55-57`), none of which is a `Do*` destiny event, and through
   the ball-lifecycle `Do*` events it shares with every other service.

**So `Park::encode` does not change, `BallsAdded`/`BallsRemoved` do not change,
and nothing in `destiny.rs` is touched by this feature.** The one interaction
is the *reverse* direction: ball removal already clears the locker's target box
(§5.1), which is a property of code that already shipped.

The consequence for the data model: locks live in `space_state.rs` beside the
`Roster` (they are a property of being in space and must die with it), **not**
in `destiny.rs` and **not** in `Park`.

---

## 8. What the other player sees

### 8.1 The push

```
broadcastID: "OnTarget"
args:        ('otheradd', lockerShipID)
audience:    [victimCharacterID]
```

`targetMgr.OnTargetByOther(otherID)` (`targetMgr.py:1342`) **[V]**:

1. `stateSvc.SetState(otherID, state.threatTargetsMe, 1)` — `threatTargetsMe`
   is state **7** (`parklife/states.py:11`). `InSpaceBracket.Load_update` reads
   it, so the locker's bracket gains the "being targeted" marking.
2. `if otherID in self.targetedBy: LogError('already targeted by', otherID)`
   **else** `AddTargetedBy(otherID)` — which appends and scatters
   `OnClientEvent_TargetedByChanged` (`targetMgr.py:1675`). `IsTargeted()`
   (`targetMgr.py:1414`) becomes true.
3. `slimItem = bp.GetInvItem(otherID)`; if the ball is not there yet the id is
   parked on `pendingTargeters` and `DoBallsAdded_` (`targetMgr.py:605`)
   retries when it arrives — so **the push is safe to send before the ball**.
4. `self.ownerToShipIDCache[slimItem.ownerID] = otherID`.
5. **Auto-target-back**, if the player has enabled it: if
   `min(settings.user.ui.Get('autoTargetBack', 0),
   godma.GetItem(session.charid).maxLockedTargets,
   godma.GetItem(session.shipid).maxLockedTargets) > len(tgts)`, the client
   spawns `uthread.pool(..., self.LockTarget, otherID, autotargeting=1)` —
   **i.e. our `otheradd` push can cause an unsolicited inbound `AddTarget`.**
   The setting defaults to 0, and with attribute 192 unserved the `min` is 0 as
   well, so it is off twice over today. Worth knowing before it surprises
   someone.

The teardown is `('otherlost', lockerShipID, reason)` →
`OnTargetByOtherLost` (`targetMgr.py:1381`), which clears state 7, drops the
`pendingTargeters` entry, `RemoveTargetedBy` (guarded against `ValueError`),
and clears the `ownerToShipIDCache` entry. `reason` is accepted and never read
**[V]**.

### 8.2 The thing to not get wrong

`OnTarget('otheradd', …)` must be addressed to the **victim**, and the id in it
is the **locker's ship id**. Getting the two the wrong way round is invisible
in a single-client test and obvious with two, which is exactly what
`docs/lanes/two-in-space.md` built the harness for. The `audience()` contract
is a list of **character** ids (`notify.rs:72`); the payload is a **ship** id.

### 8.3 Who else in the system is told? **Nobody.**

There is no third-party notification. A bystander watching Alice lock Bob sees
nothing at all in this build — no state, no bracket change, nothing in the park
**[V]**, because there is nothing in the park to change (§7). Fan-out is
`[victim]`, not `others_in_system`.

---

## 9. The UI call sites — what actually produces an `AddTarget`

Almost nothing calls `TryLockTarget` from a click directly. The gesture goes
through a **combat command**, which is a modifier the player *holds* while
clicking anything that stands for an item **[V]**:

| step | file:line |
|---|---|
| modifier held → `cmd.callback()` | `eveCommands.py:484`, `_CheckLoadCombatCommand` |
| `CmdLockTargetItem` stashes the function | `eveCommands.py:2823`: `self.LoadCombatCommand(sm.GetService('target').TryLockTarget, cmd)` |
| a widget supplies the id | `uicore.cmd.ExecuteCombatCommand(itemID, uiconst.UI_CLICK)`, `eveCommands.py:3157` |
| → `ExecuteActiveCombatCommand` | `eveCommands.py:3176`: `uthread.new(self.combatFunctionLoaded, itemID)` |

**The default bindings, read off the bytecode** (`eveCommands.py:693-697`,
with `CTRL = uiconst.VK_CONTROL` at `:540`) **[V]**:

| command | default | effect |
|---|---|---|
| `CmdToggleTargetItem` | `None` (**unbound**) | `ToggleLockTarget` |
| **`CmdLockTargetItem`** | **CTRL** | `TryLockTarget` |
| `CmdSelectTargetItem` | SHIFT | `menusvc.SelectTarget` |
| **`CmdUnlockTargetItem`** | **CTRL+SHIFT** | `UnlockTarget` |
| `CmdUnlockTargetItems` | `None` (**unbound**) | `UnlockTargets` — fires immediately, no click |

So the gesture to tell a human tester is **hold Ctrl and left-click** the
target (bracket, overview row, the ship model itself, or the selected-item
panel's lock icon), and **Ctrl+Shift+click** to unlock. `CmdSelectPrevTarget`
/ `CmdSelectNextTarget` are ALT+Left / ALT+Right (`eveCommands.py:573-574`).

**Double-click is not a lock.** `inSpaceBracket.OnClick`
(`inSpaceBracket.py:878`) routes a double click to `menusvc.Activate` —
approach/dock/open — and early-returns when a combat command is loaded
**[V]**. The legacy `bracket.SimpleBracket.OnDblClick` (`bracket.py:623`) is
`pass`.

The other entries worth knowing, each a different inbound shape of the same
call **[V]**:

- **Right-click menu**: `menusvc.LockTarget` (`menusvc.py:3349`) →
  `TryLockTarget`; `menusvc.UnlockTarget` (`:3353`); `menusvc.CancelLockTarget`
  (`:3357`) → `CancelAddTarget`. Entry visibility is
  `celestialCheckers.OfferLockTarget`, which requires `IsWithinTargettingRange`
  — the reason §6.2's attribute 76 matters.
- **Radial menu** (hold LMB on a bracket) resolves to the *same* menu
  callbacks (`spaceRadialMenuFunctions.FindRadialMenuOptions`), so it is not a
  separate path.
- **Marquee drag with Ctrl held**: `navigation.ApplyMarqueeCommands` →
  `_ApplyCommandToMultiple` → `_FilterBallsByCombatCmd`, which truncates to
  `GetNumAdditionalTargetsAllowed()` and then issues **one `AddTarget` per
  ball**. This is the only path that depends on attribute 192, and the only one
  that can produce a burst of calls.
- **Selected-item panel**: `selectedItemConst.py:151` maps the lock icon to
  `CmdLockTargetItem`; a single left-click on it locks.
- **Direct `TryLockTarget`, no combat command**: Ctrl+click a drone row
  (`drone.py:444`, `drones/droneEntry.py:328`), a d-scan result, a
  mining-scan row, or the fleet "last broadcast" banner
  (`fleetBroadcastCont.OnLastBroadcastClick`, `fleetBroadcastCont.py:275`,
  which is the one place that *does* check `IsInTargetingRange` first).
- `LockTargetPassively` (`targetMgr.py:1630`) is **dead code** in this build:
  zero callers anywhere in `out/pyc` **[V]**. Ignore it.

### 9.1 The module-needs-a-target flow, for later

`shipmodulebutton.ActivateEffect` (`shipmodulebutton.py:1529`): activating a
target-required module (`effectCategory == 2`) with no active target calls
`targetMgr.OrderTarget(self)` (`targetMgr.py:766`), which shows the crosshair
cursor (`ShowTargetingCursor`, `:789`) and sets
`IsSomeModuleWaitingForTargetClick()` (`:772`) true. The loop closes when
`state.activeTarget` is set on some item and the module button's own
`OnStateChange` sees it. Nothing in it is a server message — noted here only so
the next lane does not rediscover it while looking for one.

---

## 10. Where this goes in our tree

Not a patch — the owner writes the code. These are the five places, with why
each is the right one.

**10.1 `service/inventory.rs:694`, the `dogmaIM` arm.** Five new methods
beside the two that already answer. `req.args` for `AddTarget` is one
`PyValue::Int`; the subject is `reg.roster().get(req.actor)?.ship_id` and never
an argument (§2.3). `GetTargets`/`GetTargeters` stop being `vec![]`.

**10.2 `space_state.rs`, beside `Roster`.** The lock table. Two maps or one —
but note that **both directions are queried by the client**
(`GetTargets` and `GetTargeters`), both are needed to build the unlock
audience, and the reverse direction is the one that produces Ghost 3. Keyed by
**ship id** on both sides, because that is what the wire carries and what
`Presence::ship_id` holds. It must be torn down wherever `Roster::leave` is
called — docking, disconnect — and that is the argument for it living in the
same file as the `Roster` rather than in `service.rs`.

**10.3 A `TargetChange` type implementing `notify::Push`**, in whichever module
owns the lock table. `to_push()` returns `("OnTarget", vec![bstr(what),
PyValue::Int(id), reason])`; `audience()` returns the one character who must be
told. Per `notify.rs`'s own doc comment and `docs/lanes/README.md` rule 2, this
is **zero edits to the dispatcher**.

**10.4 `service.rs:4040`, `ship_attributes`.** Three ids: `76` maxTargetRange,
`192` maxLockedTargets, `564` scanResolution, all `PyValue::Real` (§6.1-6.3).
Three new `Hull` fields (`destiny.rs:428`) is the shape that matches the ten
already there. **`destiny.rs` is another lane's file — the coordinator decides
whether this rides with targeting or waits.**

**10.5 `service.rs:2737`, `character_dogma_row`.** Attribute `192` on the
character, beside the five attributes already there. Only needed for
auto-target-back (§8.1 step 5), so it is the lowest-value of the five.

---

## 11. Falsifiable tests

Written as assertions about **the client's requirements**, not about our
implementation, so they survive a rewrite.

### 11.1 Loopback, one client

1. **`dogmaIM.AddTarget` answers a two-element sequence.**
   `assert!(matches!(call("dogmaIM","AddTarget",[bob_ship]), PyValue::Tuple(v) | PyValue::List(v) if v.len() == 2))`.
   This is the `UNPACK_SEQUENCE 2` at `targetMgr.py:1589` and it is the single
   highest-value assertion in the feature: a one-element or three-element answer
   is a silent lock failure with no log the player sees.
2. **Its first element is falsy, and nothing is queued.** With the
   return-confirmed design, `AddTarget` must produce **no** push — a `('add',
   …)` push on top of the client's own `OnTargetAdded` makes the client
   `ClearTarget` and rebuild the box (`targetMgr.py:674`, `if tid in
   self.targetsByID: self.ClearTarget(tid)`), which flickers. Assert
   `reg.take_pending()` is empty after a successful `AddTarget`.
3. **`RemoveTarget` queues exactly one push per audience.** `('lost',
   victimShip, _)` to the locker **and** `('otherlost', lockerShip, _)` to the
   victim. Two pushes, two audiences, one call.
4. **`RemoveTarget` for a target we do not hold answers without raising.** The
   client calls it only from inside `if tid in self.targetsByID`, but our
   roster and its `targetsByID` can disagree after any desync.
5. **`CancelAddTarget` and `ClearTargets` answer and do not raise.** Both
   discard our return value, so the only observable requirement is "not
   `NoSuchMethod`" — and `no_method` raises, which `LockTarget`'s bare except
   swallows into a silent failure.
6. **`GetTargets()` and `GetTargeters()` are always `PyValue::List`**, even
   for an actor who is docked, has no ship, or is not in the roster.
   `godma.RefreshTargets` iterates both **synchronously inside
   `targetMgr.Run`**, which `viewStateSvc._OpenView` calls outside any try
   block — so a raise here does not fail a lock, it means the space view never
   opens (`service/inventory.rs:747`).
7. **`GetTargets()` returns what `AddTarget` put there, and `GetTargeters()`
   returns the mirror.** Lock Bob as Alice; assert Alice's `GetTargets()`
   contains `bob.ship_id` and Bob's `GetTargeters()` contains `alice.ship_id`.
8. **Docking drops the locks and tells the other side.** Alice locks Bob,
   Alice docks: assert Alice's `GetTargets()` is empty, Bob's `GetTargeters()`
   is empty, and a `('otherlost', aliceShip, _)` push is queued for Bob. This
   is Ghost 3 and it is the one a loopback test can actually pin.
9. **Self-lock is refused.** `AddTarget(own_ship_id)` must raise, not answer —
   the client's own guard only blocks it while piloting a structure
   (`targetMgr.py:1525`).
10. **The three attributes are present and are `Real`.**
    `ship_attributes(587)` contains keys 76, 192 and 564, each a
    `PyValue::Real` — ints floor-divide in the client and `40000000.0 /
    scanResolution` would still be float, but 192 feeds `int(maxTargets)` in
    `_FilterBallsByCombatCmd` and the house rule is floats.
11. **A push's event name is exactly `OnTarget`.** Assert the string, because
    `ScatterEventWithoutTheStars` logs two `LogError`s and a traceback for a
    name not starting with `On` (`docs/reference/push-path.md` §2.3) and dispatches
    nowhere for a name no service registered.
12. **`what` is one of the five strings.** `'add' | 'clear' | 'lost' |
    'otheradd' | 'otherlost'`, as a test over whatever enum we use, so a typo
    is a test failure rather than a push the client silently ignores.

### 11.2 With a human and two clients

Reading order, and each step is a different thing breaking:

1. Alice Ctrl+clicks Bob's ship. **A target box appears top-right with Bob's
   name and a distance that counts.** Failure here is §3.1.
2. **Bob's bracket for Alice gains the "being targeted" marking** and the
   fleet/HUD "you are being targeted" state appears. Failure here is §8.2 —
   almost always the audience and the ship id swapped.
3. Alice Ctrl+Shift+clicks Bob. **The box disappears on Alice's screen, and the
   marking disappears on Bob's.** Failure in the first half is Ghost 1; in the
   second half, Ghost 3.
4. Alice right-clicks Bob. **The menu contains "Lock target".** If it does not,
   that is attribute 76 being zero (§6.2) — and in a capsule it is *correct*
   that it does not (§6.3). Fly a Rifter for this one.
5. Alice locks Bob; **Bob docks**. Alice's box must vanish (it will — ball
   removal, §5.1) **and Alice must not be able to lock Bob's ghost
   afterwards**.
6. Alice locks Bob; **Alice docks and undocks**. On undock her target bar must
   be **empty** — this is `GetTargets()` after the session change (§5.5), and
   it is the test that catches a lock table that outlives the roster entry.

---

## 12. Open questions, ranked by cost if wrong

**1. The semantics of `flag`, and therefore whether the authentic acquisition
delay is free.** `[A]` We read it as "pending, wait for the push". The client
only ever evaluates `if not flag`, so both readings give the same server
behaviour — but if CCP's `flag` means something narrower, a future reader may
build on a wrong mental model. **Cost if wrong: low**, and a `(0, [])` lock is
unaffected. **Falsified by** step 1 of §11.2.

**2. Whether `AddTarget` is expected to block for the lock duration.** The
NPC behaviour tree spawns `_TryTargetLock` on its own tasklet
(`combattargets.py:410`) and tests `HasPendingTarget`
(`combattargets.py:376`) before retrying, which reads as "the server-side
`AddTargetEx` blocks until the lock completes" `[I]`. If CCP's `AddTarget`
blocks and then returns a falsy `flag`, the real design is "the return value
*is* the confirmation, after a delay" — which would make the authentic delay
purely a matter of **not answering for 3 seconds**, with no push at all. That
is an appealing shape and we must not build it: our dispatcher holds the world
write lock for the whole turn (`dispatch.rs:1467`), so a blocking `AddTarget`
would stop the world. **Cost if wrong: high if anyone tries it.** Say no now.

**3. Out-of-range lock loss.** EVE drops a lock when the target leaves
targeting range, and nothing client-side does it (§6.4). Which means a
server-side range watcher, which means either a timer (we have none) or
evaluating range on every movement command. **Cost if wrong: the most likely
source of a "ghost lock" report**, because it is the one termination path with
no client-side safety net and no obvious trigger. Deferring it is a choice to
make explicitly, not by omission.

**4. Whether `OnTarget('clear')` with a one-element args tuple is accepted.**
`OnTargetClear(self, targetsToSkip=None)` (`targetMgr.py:1399`) has the
default, and `ScatterEventWithoutTheStars` does `func(*args)`, so `('clear',)`
→ `OnTarget('clear')` → `OnTargetClear()`. `[I]` from the arity, not observed.
**Cost if wrong: low** — send one `'lost'` per id instead, which is strictly
more code and strictly safer.

**5. Whether a lock should survive the locker's ship change.** We send
`OnTargetClear` behaviour implicitly (the client does it itself on
`ProcessSessionChange`), so our table must agree or `GetTargets()` will
resurrect locks the client has already dropped. `[I]` **Cost if wrong: a
ghost that only appears after a ship swap**, i.e. one nobody will reproduce on
demand.

**6. Structures (`AddTargetOBO` / `RemoveTargetOBO`) and fighters.**
Deliberately not investigated. `playerOwnedTargetMgr.LockTargetOBO` has the
identical `flag, targetList` shape with an extra leading `structureID`
(`playerOwnedTargetMgr.py`), and its pushes are
`OnStructureTargetAdded(sid, tid)` / `OnStructureTargetRemoved(sid, tid)`
(`findings/push-events.tsv:2731-2732`). **Cost if wrong: zero tonight.**

**7. `OnMultiEvent` as a batching path.** Not needed, and its nested
`(eventTuple, groupingKey)` shape (§3.3) is the kind of thing to get wrong
once. If a burst from a marquee lock ever matters, the cheap answer is several
`OnTarget` frames in one flush — `conn.rs:146-156` already puts every
zero-delay push in one `send_all` under one `Wire` lock.
