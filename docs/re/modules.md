# Activating a fitted module

**Build:** V24.01 / 3619879. **Lane:** `modules` (read-only RE).
**Scope:** the step after target locking — a gun firing, a hardener running, a
mining laser cycling. This document is an implementation spec: wire shapes,
evidence with file and line, what breaks if it is wrong, and tests.

Evidence markers: `[V]` verified against this client's bytecode or shipped
data · `[I]` inferred · `[A]` an assumption we choose to ship, with what would
falsify it. There is no `[V2]` in this document: nothing here came from a
third-party decode.

All line numbers are `tools/dis.py` numbers (the client's own `co_lnotab`),
**not** uncompyle6's reconstructed output. Shorthand:

| short | file |
| --- | --- |
| `godma` | `out/pyc/eve/client/script/environment/godma.pyc` |
| `smb` | `out/pyc/eve/client/script/ui/inflight/shipModuleButton/shipmodulebutton.pyc` |
| `slots` | `out/pyc/eve/client/script/ui/inflight/shipHud/slotsContainer.pyc` |
| `shipUI` | `out/pyc/eve/client/script/ui/inflight/shipHud/shipUI.pyc` |
| `asc` | `out/pyc/eve/client/script/ui/inflight/shipHud/activeShipController.pyc` |
| `fx` | `out/pyc/eve/client/script/parklife/fxSequencer.pyc` |
| `bdl` | `out/pyc/eve/common/script/dogma/baseDogmaLocation.pyc` |
| `moniker` | `out/pyc/eve/common/script/net/eveMoniker.pyc` |
| `cdl` | `out/pyc/eve/client/script/dogma/clientDogmaLocation.pyc` |

---

## 0. Minimum viable activation

**The whole feature is two pushes and one call reply.** The client animates the
cycle itself, forever, from a duration it computes locally. The server is not
a clock.

### 0.1 Preconditions the server must already satisfy (before any click)

These are not part of the activation exchange; they are what makes a module
button exist at all. If they are missing the player has nothing to click.

1. **`dogmaIM.GetAllInfo`'s `shipInfo` must contain the module.** `shipInfo`
   is `{itemKey: row}`; today we send exactly one entry, the pod hull
   (`service.rs::capsule_dogma_row`). Each extra entry is one fitted module:

   ```text
   row = KeyVal(itemID        = <module itemID>,
                invItem       = DBRow(item_columns(), ...),   # singleton = True
                attributes    = {attributeID: float, ...},
                activeEffects = {},            # see §2.4
                time          = None,
                wallclockTime = None)
   ```

   `invItem.locationID` must be the ship, `invItem.flagID` a fitting flag
   (**11–18 low, 19–26 mid, 27–34 high** `[V]`
   `out/pyc/inventorycommon/const.pyc:122-145`), and **`invItem.singleton`
   must be truthy** — `godma:1939` returns early for
   `not item.singleton`, so a non-singleton module is silently dropped from
   the HUD. (`item.rs:46` already records this requirement.)
2. **A real fitting has to persist somewhere.** `shipInfo` is rebuilt from
   server state on every `Prime`. The minimum stored state per fitted module
   is `(item_id, type_id, owner_id, location_id = ship_id, flag_id,
   quantity = 1, singleton = true)` — i.e. **exactly the existing `Item`
   record with a slot flag**, nothing new. `/spawn` into a slot flag is
   enough for a first end-to-end test; mutators are still not needed.
3. **The module's `attributes` must include the duration attribute.** See
   §3: without it the cycle is zero-length and the ramp never draws.

### 0.2 The exchange

```text
player clicks the HUD button
  │
  │  smb:1075  ModuleButton.Click
  │    local pre-checks, no server traffic: IsModuleReloading, def_effect
  │    exists, self.online, IsModuleActivatingInGodma
  │  smb:1529  ModuleButton.ActivateEffect
  │    if relevantEffect.effectCategory == 2 and no target: ask targetMgr for
  │    the active target; if there is none, OrderTarget and STOP (no call)
  │  smb:1580  def_effect.Activate(targetID, repeats)
  │  godma:1176 EffectWrapper.Activate  -> StateManager.Activate
  │  godma:2441 StateManager.Activate   -> dogmaIM.Activate(...)
  ▼
C→S  bound object dogmaIM, bound as (solarsystemID, groupSolarSystem=5)
     Activate(itemID: int, effectName: str, target: int|None, repeat: int)
                                                            # repeat is 1000
  ▼
S→C  the call reply.  **Return value is discarded** -- `ret` is stored in
     godma:2441 and returned, and smb:1580 does POP_TOP.  Return None.
  ▼
S→C  notification  OnGodmaShipEffect  (to svc.godma; see §2 for field order)
       itemID, effectID, t, start=1, active=1, environment, startTime=None,
       duration=<ms>, repeat=1000, error=None, actualStopTime=None
     -> godma:1734-1739 writes isActive/startTime/duration/repeat/targetID
        onto the already-primed effect row
     -> godma:1742 ScatterEvent('OnModuleActivated', itemID, effectID)
     -> godma:1758 uthread.new(ActualStopEffect, ...) which immediately
        ChainEvent('ProcessShipEffect', stateManager, effectState)
     -> slots:224 module.Update(effectState)
     -> smb:1495 SetActive() -> ActivateRamps() -> DoActivateRampsThread()
        **button goes active and cycles, with no further server traffic**
  ▼
S→C  notification  OnSpecialFX  (to svc.FxSequencer; see §5)
       shipID, moduleID, moduleTypeID, targetID, otherTypeID, guid,
       isOffensive, start=1, active=1, duration=<ms>, repeat=1000,
       startTime=None, timeFromStart=0, graphicInfo=None
     -> the turret/laser/beam is drawn, for the shooter and for every
        observer in the ballpark
  ▼
(optional, for the capacitor bar to move)
S→C  notification  OnModuleAttributeChange
       ownerID=charID, itemKey=shipID, attributeID=18 ('charge'),
       time=<blue filetime>, newValue, oldValue, wallclockTime
```

Deactivation is the mirror image: `dogmaIM.Deactivate(itemID, effectName)`
then `OnGodmaShipEffect(..., start=0, active=0, ...)`.

### 0.3 Every number in that exchange, and where it comes from

| field | value for *200mm AutoCannon I* on a Rifter | source |
| --- | --- | --- |
| `effectName` | `'projectileFired'` | client FSD `typedogma` → the one effect with `isDefault` and `effectCategory ∈ {1,2}` `[V]` |
| `effectID` | `34` | `dogmaeffects` `[V]` |
| `duration` (ms) | `3750` | `dogmaeffects[34].durationAttributeID = 51` (`speed`), then `typedogma[486].speed = 3750.0` `[V]` |
| `repeat` | `1000` | the client sends it (`smb:1565`, `GetRepeatCount` → 1000); echo it back |
| `startTime` | `None` | §3.3 — `None` makes the client anchor on its own clock |
| `guid` | `'effects.ProjectileFired'` | `dogmaeffects[34].guid` `[V]` |
| capacitor drain | `0.0` | `dogmaeffects[34].dischargeAttributeID = 6` (`capacitorNeed`), `typedogma[486].capacitorNeed = 0.0` `[V]` — a projectile gun uses no cap |

Three more worked examples, all `[V]` from the shipped tables via
`tools/dogma.py`:

| typeID | name | default effect | id | cat | durAttr → value | guid | dischAttr → value |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 486 | 200mm AutoCannon I | `projectileFired` | 34 | 2 | 51 `speed` → 3750 ms | `effects.ProjectileFired` | 6 → 0.0 |
| 483 | Miner I | `miningLaser` | 67 | 2 | 73 `duration` → 15000 ms | `effects.Laser` | 6 → 5.0 |
| 399 | Small Shield Booster I | `shieldBoosting` | 4 | 1 | 73 `duration` → 2000 ms | `effects.ShieldBoosting` | 6 → 20.0 |
| 439 | 1MN Afterburner I | `moduleBonusAfterburner` | 6731 | 1 | 73 `duration` → 10000 ms | `effects.Afterburner` | 6 → 20.0 |
| 454 | Small Focused Beam Laser I | `targetAttack` | 10 | 2 | 51 `speed` → 5000 ms | `effects.Laser` | 6 → 9.025 |
| 499 | Light Missile Launcher I | `useMissiles` | 101 | 1 | 51 `speed` → 16000 ms | `''` — see §6.4 | 6 → absent |

Note that two different modules can share a `guid`: the Miner I and the Small
Focused Beam Laser I are both `effects.Laser`. Row 499 is the launcher case: the
launcher's own effect has an empty `guid` and no `capacitorNeed`, and the
visual comes from the loaded missile's own default effect (§6.4).

**Reproduce the whole chain with one command:**

```sh
tools/analysis.sh python3 - <<'EOF'
import sys; sys.path.insert(0,'/work/tools')
import dogma as D
d = D.Dogma('/client', '/client-root/ResFiles')
eff   = {k: r for k, r in d.table('dogmaeffects').rows()}
attrs = {k: r for k, r in d.table('dogmaattributes').rows()}
byname = {r['effectName']: r for r in eff.values()}
row = d.of_type(486)
for name, is_default in row['effects']:
    r = byname[name]
    if is_default and r['effectCategory'] in (1, 2):
        dur = r['durationAttributeID']
        print(name, r['effectID'], 'cat', r['effectCategory'],
              'guid', r['guid'],
              'duration_ms', row['attributes'][attrs[dur]['name']])
EOF
```

---

## 1. The activate call

### 1.1 It is a **bound object**, not a plain service call `[V]`

`godma:1330` → `eveMoniker.CharGetDogmaLocation()`; `moniker:64` builds
`Moniker('dogmaIM', GetLocationBindParams())` and `moniker:65` sets a session
check.

- **Bind key** `[V]` `moniker:42-46`: `(session.solarsystemid,
  const.groupSolarSystem)` in space, `(session.stationid,
  const.groupStation)` in station. `groupSolarSystem = 5`.
- **Session check** `[V]` `moniker:55-58`: `{'solarsystemid': ...}` or
  `{'stationid': ...}` — client-side only; it re-resolves the moniker if the
  session moved.
- `docs/lanes/call-surface.md` / `findings/bound-objects.json` already list
  `dogmaIM` among the 29 moniker services, and
  `service/inventory.rs:748` already notes the `(solarsystemID, 5)` bind. So
  **no new transport work**: this is the same path `GetAllInfo` already takes.

### 1.2 Signatures `[V]`

```text
dogmaIM.Activate(itemID, effectName, target, repeat)      # 4 args
dogmaIM.Deactivate(itemID, effectName)                    # 2 args
```

- `godma:2427` `StateManager.Activate(self, itemID, effectName, target, repeat)`
  and the remote call at `godma:2441`.
- `godma:2471` `StateManager.Deactivate(self, itemID, effectName)`, remote
  call at `godma:2479`.
- `findings/bound-objects.json` independently records
  `dogmaIM.Activate` arity 4 from `godma.pyc` offset 176 and
  `dogmaIM.Deactivate` arity 2 from offset 156. **Two sources agree.**

Types:

| arg | type | notes |
| --- | --- | --- |
| `itemID` | int | the **module's** itemID, never the charge's |
| `effectName` | str | the effect *name*, not its id. Resolve `effectID → effectName` from `dogmaeffects` server-side if you prefer ids internally |
| `target` | int or `None` | `godma:2437` raises `RuntimeError('InvalidTargetType')` unless it is `None` or `numbers.Integral` |
| `repeat` | int | 1000 in practice; 0 means one shot |

### 1.3 What the client does around the call, and the two traps in it

- **`invCache.TryLockItem(itemID, 'lockItemGodmaActivate', {}, 1)` is held
  across the whole RPC** `[V]` `godma:2439`, unlocked in a `finally` at
  `godma:2446`. `TryLockItem` is **purely client-local** `[V]`
  (`out/pyc/eve/client/script/environment/invCache.pyc:350`, a semaphore and a
  dict, no moniker). **Trap: a server that never replies leaves the module UI
  item locked.** Always reply, even to a refusal — a raised `UserError` is a
  reply.
- **`StateManager.activatingEffects` guards re-entry** `[V]` `godma:2430`: a
  second `Activate` for the same `(itemID, effectName)` while the first is in
  flight is dropped client-side with `'Trying to activate effect twice'`.
- **`online` is special** `[V]` `godma:2443`: after a successful
  `Activate(itemID, 'online', ...)` the client records
  `activatedLocation[(itemID,'online')] = session.locationid`, and
  `Deactivate` refuses to send if the location still matches
  (`godma:2474-2475`). Module activation is unaffected; onlining is not.
- **`'effect_activating'` self-clears after 500 ms** `[V]` `smb:1104`
  (`timerstuff.AutoTimer(500, ActivateEffectTimer)`), so a refused activation
  re-arms the button without any server help.

### 1.4 The call reply is discarded `[V]`

`smb:1580` is `CALL_FUNCTION; POP_TOP`. `godma:2444` returns `ret` and nothing
reads it. **Return `None`.** The *only* thing a reply conveys is "not an
exception" plus releasing the item lock.

---

## 2. The confirmation — `OnGodmaShipEffect`

This is the message that is the whole feature.

### 2.1 Delivery `[V]`

`OnGodmaShipEffect` is a member of `svc.godma`'s `__notifyevents__`
(`godma:113`, inside the class body at `godma:98`), so **a plain
`macho.Notification` with method `OnGodmaShipEffect` is dispatched directly**
— no `OnMultiEvent` wrapper is needed.

`Godma.OnGodmaShipEffect` (`godma:467`) is `(*args, **kwargs)` and forwards to
`StateManager.OnGodmaShipEffect` (`godma:1722`).

`out/pyc/eve/client/script/environment/spaceObject/ship.pyc:164` also listens
(10 positional args + one default) and uses it **only** for the mining
harvesting animation state (`effectMining=17`, `effectMiningLaser=67`,
`effectMiningClouds=2726`). A 10-argument push is therefore legal; the two
extra fields default.

### 2.2 Full field order `[V]` `godma:1722`

```python
OnGodmaShipEffect(itemID, effectID, t, start, active, environment,
                  startTime, duration, repeat, error,
                  actualStopTime=None, stall=True)
```

Twelve parameters, the last two defaulted. The names and order come from
`co_varnames[:argcount]` of the code object at `godma:1722`.

| # | field | type | what it must be | consequence of getting it wrong |
| --- | --- | --- | --- | --- |
| 1 | `itemID` | int | the module's itemID | must be a key of `effectsByItemEffect`, else the whole message is a no-op (§2.3) |
| 2 | `effectID` | int | the dogma effect id | `godma:1729` does `dogma.data.get_effect(effectID)`; an id not in the client's FSD raises inside the handler |
| 3 | `t` | blue filetime | the event time; copied into `effectState.time` | read only by `TryRecordLastStopTime`; harmless |
| 4 | `start` | 0/1 | **1 to start, 0 to stop** | this is what `ModuleButton.Update` switches on (`smb:1493-1497`): `start` → `SetActive()`, else `SetIdle()` |
| 5 | `active` | 0/1 | 1 when the effect is now running | `godma:1735` sets `isActive`; `godma:1741-1742` fires `OnModuleActivated` only when truthy |
| 6 | `environment` | list of 7 | see §2.5 | `godma:1739` reads `[3]`; `godma:1745` reads `[2]` and `[0]`. A short list is an `IndexError` inside the handler |
| 7 | `startTime` | blue filetime or `None` | **send `None`** (§3.3) | a wrong epoch makes the ramp jump; `None` cannot |
| 8 | `duration` | int, **milliseconds** | the cycle length | `godma:1737`; the ramp's fallback duration (§3.2). `-1` is the client's own sentinel for "no duration" (`godma:3072`) |
| 9 | `repeat` | int | echo the client's `repeat` | `godma:1738`; drives `GetRepeatCount`/`SetRepeat` display |
| 10 | `error` | `None`, or `(dialogKey, paramsDict)` | `None` on success | `shipUI.ProcessShipEffect` (`shipUI.pyc`, `ProcessShipEffect`) does `prompt_player_fsd_dialog(error[0], error[1])` — this is the server's channel for "it stopped because X" (§7.3) |
| 11 | `actualStopTime` | blue filetime or `None` | `None` | `godma:1776-1782`: when `start` is falsy and this is not `None`, the client `SleepSim`s until then before processing the stop. A bad value stalls the stop |
| 12 | `stall` | bool | omit, or `True` | `godma:1723`: when truthy *and* a `Prime` is in flight, the message waits on the priming channel instead of being dropped. `True` (the default) is the safe value |

**The client's own call gives us a template** `[V]` `godma:3072` — when a
module goes offline silently, `ApplyAttributeChange` fakes the event:

```python
self.OnGodmaShipEffect(itemKey, const.effectOnline, blue.os.GetSimTime(),
                       0, 0,
                       [itemKey, item.ownerID, item.locationID,
                        None, None, [], const.effectOnline],
                       None, -1, -1, None, None)
```

That is eleven positional arguments: `startTime=None`, `duration=-1`,
`repeat=-1`, `error=None`, `actualStopTime=None`, `stall` defaulted. It is the
best available confirmation of both the sentinels and the `environment`
shape.

### 2.3 The one hard precondition `[V]` `godma:1731`

```python
if itemID in self.effectsByItemEffect and effectName in self.effectsByItemEffect[itemID]:
    ... everything that matters ...
if effect.effectCategory == const.dgmEffOverload:
    ... overload bookkeeping only ...
```

**If the module was never primed, `OnGodmaShipEffect` does nothing at all** —
no error, no log beyond `LogInfo`, no button change. This is the single most
likely cause of "we sent the message and nothing happened".

The row is created by `StateManager.RefreshItemEffects` (`godma:1967`), which
runs from `UpdateItem` during `Prime`. Critically, it builds the rows
**entirely from client-side FSD** (`godma:1982-1992`): for every effect of the
type where `isDefault` (and not `effectSkillEffect`), or
`effectCategory ∈ {dgmEffActivation, dgmEffTarget, dgmEffArea, dgmEffOverload}`,
or `fittingUsageChanceAttributeID is not None`, it emits a 20-column `DBRow` (`godma:1983-1992`).
So **the server never sends effect definitions** — only which effects are
*currently* active.

### 2.4 `activeEffects` — the priming form of the same data `[V]`

`godma:1995-1999`:

```python
d = activeDict[effectID]          # activeDict is info.activeEffects
env = d[:ENV_IDX_LINE]            # d[:7]
startTime, duration, repeat = d[7], d[8], d[9]
eventLines.append((itemID, effectID, info.time, True, True,
                   env, startTime, duration, repeat, None))
... self.OnGodmaShipEffect(*line, stall=False)
```

So in `ItemGetInfo`/`GetAllInfo`, **`activeEffects` is
`{effectID: [env0..env6, startTime, duration, repeat]}` — a ten-element
list** whose first seven elements are the environment. A module that is
already cycling when the player logs in is expressed here, and the client
replays it through the same handler. The filter at `godma:1995` means only
categories 1, 2, 3 and 5 replay.

### 2.5 `environment` `[V]`

Index constants, `godma:1198-1205`:

| index | name | value |
| --- | --- | --- |
| 0 | `ENV_IDX_SELF` | the module's itemID |
| 1 | `ENV_IDX_CHAR` | the pilot's charID |
| 2 | `ENV_IDX_SHIP` | the ship's itemID |
| 3 | `ENV_IDX_TARGET` | the target's itemID, or `None` |
| 4 | `ENV_IDX_OTHER` | `None` |
| 5 | `ENV_IDX_AREA` | `[]` |
| 6 | `ENV_IDX_EFFECT` | the effectID |
| 7 | `ENV_IDX_LINE` | **not sent** — it is the slice bound in §2.4 |

Reads in the activation path: `[3]` → `effect.targetID` (`godma:1739`);
`[2] == session.shipid` (`godma:1745`) which is what gates
`ProcessShipEffect` and therefore the entire HUD update; `[0]` → fed to
`pwn.GetCurrentControl` for the structure-turret case only.

`[V]` it is a **list** (`godma:3072` builds one with `BUILD_LIST`). `[I]` a
tuple would behave identically — every access is a subscript or a slice.

### 2.6 What the confirmation triggers, end to end `[V]`

```text
godma:1742  ScatterEvent('OnModuleActivated', itemID, effectID)
              consumers: objectives/, nodegraph/ only -- no HUD effect
godma:1758  uthread.new(ActualStopEffect, itemID, effect, actualStopTime,
                        effectState)
godma:1786  ChainEvent('ProcessShipEffect', stateManager, effectState)
              -> shipUI.ProcessShipEffect     : shows error[] if not None
              -> slots:214  ProcessShipEffect : module.Update(effectState)
              -> turret.ProcessShipEffect     : 'online' only
              -> targetInBar.ProcessShipEffect
smb:1486    ModuleButton.Update(effectState)
  :1490       returns early unless stateManager.IsItemLoaded(self.id)
  :1495       effectState.start -> SetActive()
  :1497       else              -> SetIdle()
  :1505/1507  effectName == 'online' -> ShowOnline/ShowOffline
  :1510       stop with quantity == 0 -> AutoReload()
smb:1328    SetActive(): glow, isInActiveState = True, ActivateRamps()
```

Note `smb:1490`: `self.id` is the **charge's** itemID when a charge is loaded
(`smb:SetCharge` sets `self.id = charge.itemID`), and for a charge in a module
that is a *sublocation tuple*. `IsItemLoaded` (`godma:1317`) accepts both
`invitems` and `sublocationsByKey`, so a loaded gun needs its ammo sublocation
primed or **`Update` returns before touching the button** (§6).

---

## 3. Cycle timing — the server does not tick

### 3.1 The client runs the animation off its own clock `[V]` `smb:1692`

`DoActivateRampsThread` is a `uthread` loop:

```python
(firstActivation, startTime), durationInMilliseconds = self.GetEffectTiming()
if durationInMilliseconds <= 0: return
...
while self and not self.destroyed and self.ramp_active and durationInMilliseconds:
    newNow = blue.os.GetSimTime()
    ...
    portionDone = blue.os.TimeDiffInMs(startTime + startTimeAdjustment, now) / durationInMilliseconds
    if portionDone > 1:
        iterations = int(portionDone)
        startTime += long(durationInMilliseconds * iterations * const.MSEC)
        _, durationInMilliseconds = self.GetEffectTiming()     # smb:1736
        uicore.layer.shipui.sr.rampTimers[itemID] = (False, startTime)
        portionDone -= iterations
        if self.InLimboState(): self.IdleRamps(); break
    self.sr.ramps.SetRampValues(portionDone)                   # smb:1753
    blue.pyos.synchro.Yield()
```

**It repeats forever.** The loop only ends when `ramp_active` goes false
(i.e. `SetIdle`/`IdleRamps`, which the *stop* message causes) or the module
enters "limbo" (`waitingForActiveTarget`, `reloadingAmmo`, `isDeactivating`).

**Therefore: one `OnGodmaShipEffect` with `start=1` is the entire visible
cycle, indefinitely. The server sends no per-cycle message.** The server still
has to tick its *own* simulation (damage application, ammo consumption,
capacitor drain) but it owes the HUD nothing per cycle.

### 3.2 Where the duration number comes from `[V]` `smb:1812`

`GetEffectTiming`:

```python
rampStartTime = self.GetRampStartTime()
durationInMilliseconds = 0.0
durationAttributeID = getattr(self.def_effect, 'durationAttributeID', None)
attribute = dogma.data.get_attribute(durationAttributeID) if durationAttributeID is not None else None
item = self.stateManager.GetItem(self.def_effect.itemID)
if item is None: return (0, 0.0)
if attribute:
    durationInMilliseconds = self.stateManager.GetAttribute(self.def_effect.itemID, attribute.name)
if not durationInMilliseconds:
    durationInMilliseconds = getattr(self.def_effect, 'duration', 0.0)
return rampStartTime, durationInMilliseconds
```

Two sources, in order:

1. **The godma attribute named by the effect's `durationAttributeID`** —
   client-side FSD for the attribute *id and name*, server-pushed for the
   *value*. `def_effect.durationAttributeID` comes off the primed effect row
   (column 14 of `__effectheader__`, `godma:1210ff`), which
   `RefreshItemEffects` filled from `dogmaeffects` (`godma:1989`).
2. **Fallback: the `duration` field of the `OnGodmaShipEffect` message**
   (`godma:1737` stored it on the row).

**Resolution chain, concretely, typeID → milliseconds:**

```text
typeID 486 (200mm AutoCannon I)
  typedogma[486].effects          -> ('projectileFired', isDefault=True)
  dogmaeffects by name            -> effectID 34, effectCategory 2,
                                     durationAttributeID 51
  dogmaattributes[51].name        -> 'speed'
  GetAttribute(moduleItemID, 'speed')
    -> attributesByItemAttribute[moduleItemID]['speed']
    -> which the server set from shipInfo[moduleItemID].attributes[51]
    -> typedogma[486].speed = 3750.0
  => 3750 ms
```

The same chain on the real server: `bdl:1021` `GetEffectDuration` is
`duration = self.GetAttributeValue(environment.itemID, effect.durationAttributeID)`
(or `-1` when the effect has no duration attribute), and it raises
`UserError('EffectCantHaveNegativeDuration', {...})` for a negative value
(`bdl:1028`).
**That is CCP's own server code, shipped in the client** — this is the most
load-bearing corroboration in the document.

### 3.3 Send `startTime = None` `[V]`

`smb:1765` `GetRampStartTime`:

```python
rampTimers = uicore.layer.shipui.sr.rampTimers
if moduleID not in rampTimers:
    now = blue.os.GetSimTime()
    default = getattr(self.def_effect, 'startTime', now) or now
    rampTimers[moduleID] = (True, default)
return rampTimers[moduleID]
```

`... or now` — a **falsy** `startTime` (so `None`, or `0`) makes the client
anchor the ramp on its own `GetSimTime()`. This sidesteps the sim-clock epoch
question entirely and is the single cheapest correctness decision in this
feature.

If instead you send a real `startTime`, note what the client does with it
(`smb:1692`): `startTimeAdjustment = now - startTime`, discarded if it exceeds
`const.SEC`; and a `startTime` in the *future* gives a negative `portionDone`
fed straight to `SetRampValues`. The ramp dict is cleared by `IdleRamps`
(`smb:1656`), so each fresh activation re-anchors.

**Units, for when we do send times** `[V]`: blue time is 100 ns ticks
(`const.MSEC = 10000`, `const.SEC = 10000000` at
`out/pyc/carbon/common/lib/const.pyc:277-278`; `dogma.const.dgmTauConstant =
10000` at `out/pyc/dogma/const.pyc:103`). `duration` is **milliseconds** as a plain
`int` (`DBTYPE_I4` in `__effectheader__`), `startTime`/`t`/`actualStopTime` are
**blue FILETIMEs** (`DBTYPE_FILETIME`). `service::filetime_now()` already
exists in the server.

---

## 4. Capacitor

### 4.1 One attribute, and the client predicts recharge but not drain

The capacitor the player sees is read by polling, not pushed:
`capacitorContainer.UpdateThread` sleeps 500 ms wallclock and calls
`Update()`, which reads `controller.GetCapacitorCapacity()` and
`GetCapacitorCapacityMax()` `[V]`
(`out/pyc/eve/client/script/ui/inflight/shipHud/capacitorContainer.pyc`).

`[V]` `asc` `GetCapacitorCapacity` / `GetCapacitorCapacityMax`:

```python
def GetCapacitorCapacity(self):     return self._GetShipItem().charge
def GetCapacitorCapacityMax(self):  return float(self._GetShipItem().capacitorCapacity)
```

So the whole capacitor is **two godma attributes on the ship item**:
`charge` (attributeID **18**, `dogma/const.pyc:229` `attributeCapacitorCharge`)
and `capacitorCapacity` (**482**). `docs/port-tracker.md` and
`service.rs::capsule_dogma_row` already seed both.

### 4.2 `charge` is a *charged* attribute: the client simulates recharge `[V]`

`dogmaattributes` has a `chargeRechargeTimeID` column
(`findings/corpus-dogmaattributes.tsv`, offset +32). `StateManager.InitializeData`
(`godma:1821`) builds `chargedAttributeTauCaps[attrName] = (tauName, capName)`
from it, and `UpdateAttribute` (`godma:2045`) / `ApplyAttributeChange`
(`godma:2948`) promote any such attribute into
`chargedAttributesByItemAttribute[itemID][name] = [value, time, tau/5.0, cap]`.
`GetAttribute` then returns `GetChargeValue(...)` instead of the stored number
(`godma:2186`):

```python
def GetChargeValue(self, oldVal, oldTime, tau, Ec, newTime=None):   # godma:2413
    if Ec == 0: return 0
    newTime = newTime or blue.os.GetSimTime()
    sq = math.sqrt(max(oldVal / Ec, 0))
    timePassed = min(oldTime - newTime, 0) / float(const.dgmTauConstant)
    return (1.0 + (sq - 1.0) * math.exp(timePassed / tau)) ** 2 * Ec
```

For the Rifter (587): `capacitorCapacity = 250.0`, `rechargeRate = 125000.0` ms,
so `tau = 25000` `[V]`. The identical formula is in the shipped shared engine
(`out/pyc/dogma/attributes/chargedAttribute.pyc`, `Update`), which is a second
independent copy.

**So the answer to "which path updates the capacitor":**

- **Recharge: client-side prediction.** From a `(value, time)` pair plus
  `capacitorCapacity` and `rechargeRate`. The server only has to **not
  contradict** it. At `value == capacity` the exponential term drops out, so
  seeding full with `time = None` is a fixed point — which is exactly what
  `service.rs:2500ff` already relies on.
- **Drain: a godma attribute push, or nothing.** The client does *not*
  subtract `capacitorNeed` on activation. There is no `attributeCapacitorNeed`
  consumer anywhere in the client's own activation path — the identifier
  appears in only two files in all 12,774 (`dogma/const.pyc` and
  `dogma/attributes/groups.pyc`) `[V]`. `bdl:1017 ConsumeResources` is
  `pass` in the shipped base class; the real node overrides it.

### 4.3 The drain push `[V]`

```text
OnModuleAttributeChange(ownerID, itemKey, attributeID, time,
                        newValue, oldValue, wallclockTime, scatterAttr=True)
```

`godma:1575`, seven positional parameters plus one default. For a drain:
`ownerID = charID`, `itemKey = shipID`, `attributeID = 18`,
`time = <blue filetime of the drain>`, `newValue = charge - capacitorNeed`.

The batched form is `OnModuleAttributeChanges(changes)` (`godma:1620`), where
each change is an **eight-element tuple**
`('OnModuleAttributeChange', ownerID, itemID, attributeID, time, newValue,
oldValue, wallclockTime)`. `[V]` two sources: `godma:1640`'s
`OnModuleAttributeChange_(*(each[1:] + (0,)))` plus `cdl`'s own
`eventName, ownerID, itemID, attributeID, time, newValue, oldValue, _ = change`
(`clientDogmaLocation.pyc`, `OnModuleAttributeChanges`). **`clientDogmaLocation`
also listens to the plural event and unpacks exactly eight** — a seven-element
change raises there even if godma copes.

Gates to clear `[V]`:

- `godma:1591` drops the change unless `itemKey == session.charid or ownerID
  == session.charid`. For a ship attribute, **`ownerID` must be the
  character**, not the ship.
- `_IsAttributeChangeRelevant` (`godma:2036`) drops a change whose
  `wallclockTime` is older than the last one seen for that
  `(itemKey, attributeID)`. `0`/`None` always passes.
- `godma:2948` early-returns on `oldValue == newValue` after re-reading the
  current value — so a redundant push is free, and a push that merely restates
  the predicted value is a no-op.
- `time` is what becomes `oldTime` in `GetChargeValue`. **Send the drain's own
  timestamp**, not zero: a zero `oldTime` makes `timePassed` hugely negative
  and the predicted value snaps to the cap.

**Minimum viable answer for tonight: do not push the capacitor at all.** The
module cycles; the bar stays full. Nothing breaks, nothing contradicts.

---

## 5. Effects on a target, and on observers — `OnSpecialFX`

`OnGodmaShipEffect` does **not** draw anything in space. The only in-space
consumer is `ship.pyc:164`, and it handles mining harvesting state alone. The
beam, the projectile tracer and the turret animation all come from
`svc.FxSequencer`.

### 5.1 It is both a destiny action and a notification `[V]`

- **As a notification:** `OnSpecialFX` is in `FxSequencer.__notifyevents__`
  (`fx:344`, class body at `fx:332`). A plain `macho.Notification` reaches it.
- **As a destiny action:** `Park.OnSpecialFX`
  (`out/pyc/eve/client/script/remote/michelle.pyc:2087`) re-scatters to the
  same event, and `docs/archive/destiny-and-the-ballpark.md` §4.2 lists `OnSpecialFX`
  in `nonDestinyCriticalFunctions` — dispatched **without** time
  synchronisation. `SetState`'s `bag.effectStates` is replayed through it
  (ibid. §4.3).

**Use the notification.** It needs no destiny plumbing and reaches the same
handler. `[I]` — both paths end in `sm.ScatterEvent('OnSpecialFX', ...)`, and
`michelle.pyc:2087-2093` does literally nothing else (except fanning out a
list-valued `moduleID`).

### 5.2 Signature `[V]` `fx:441` and `michelle.pyc:2087`

```python
OnSpecialFX(shipID, moduleID, moduleTypeID, targetID, otherTypeID, guid,
            isOffensive, start, active,
            duration=-1, repeat=None, startTime=None, timeFromStart=0,
            graphicInfo=None)
```

Nine required, five defaulted. `findings/push-events.tsv` records the same
14-name list from six independent call sites (`fxSequencer`, `spaceMgr`,
`subwaySvc`, `tactical`, `targetMgr`, both cameras) — **seven sources agree
on the field order.**

| field | value for a gun firing |
| --- | --- |
| `shipID` | the shooter's ship itemID (a ball) |
| `moduleID` | the module's itemID, or a **list** of them for a weapon bank |
| `moduleTypeID` | the module's typeID |
| `targetID` | the target's itemID (a ball), or `None` |
| `otherTypeID` | the charge's typeID for a gun; `None` otherwise `[A]` |
| `guid` | `dogmaeffects[effectID].guid`, e.g. `'effects.ProjectileFired'` |
| `isOffensive` | `dogmaeffects[effectID].isOffensive` |
| `start` | `1` to start, `0` to stop |
| `active` | `1` for a repeating effect, `0` for a one-shot |
| `duration` | ms, the same number as §3.2 |
| `repeat` | cycle count; `fx:519-522` turns `None`/`<=0` into 1 and otherwise adds 1 |
| `startTime` | **`None`** — a non-`None` `startTime` more than 15 s in the past makes `fxSequencer` discard the trigger outright for any guid in `FX_LONG_ONESHOT_GUIDS` (`fx:485-487`) |
| `timeFromStart` | `0` |
| `graphicInfo` | `None` |

### 5.3 What `AddTrigger` requires, or silently drops `[V]` `fx:598`

- `guid` must be a dotted string with **exactly two parts** (`fx:525-531`),
  else "No guid in trigger" and return. `None` logs a warning and returns.
- The **ship's ball must exist in the ballpark** and have a loaded `model`,
  else the trigger is dropped or deferred into `delayedtriggers`
  (`fx:618`, `fx:637-647`).
- If `targetID` is not `None`, the **target's ball must exist** too, same
  rules (`fx:632`). So: for a targeted effect, both balls must already be in the
  ballpark — which is the same precondition target locking needed.
- `GetClassification(guid)` (from `out/pyc/clienteffects/`) may return `None`;
  the code copes (`fx:491-498`, `mergeFlags = [0]`) and still adds the
  activation. An unknown guid is therefore not fatal.
- `start == 1 and guid in self.disabledGuids` returns early (`fx:477`) — a
  user setting, not something we control.

### 5.4 Observers

`OnSpecialFX` is addressed to everyone who should see it. Our `Push` trait
already has `audience() -> Option<Vec<i64>>` returning **character** ids
(`notify.rs`), which is exactly the right shape: the audience of a gun firing
is every character with a session in that solar system. The `fxSequencer`
makes no distinction between the shooter's copy and an observer's — same
message, same handler.

---

## 6. Charges and ammo

### 6.1 Ammo in a module is a **sublocation**, not an item `[V]`

This is the finding that matters, because it contradicts the natural
assumption.

`asc` `GetCharges` is `return self._GetShipItem().sublocations`, and
`slots:284 InitSlots` keys them by `charge.flagID` and passes each to
`ModuleButton.SetCharge`. `godma:2186 GetAttribute` resolves
`'sublocations'` to `sublocationsByLocationFlag[shipID]` filtered by
`IsShipFittingFlag`.

A sublocation's **itemKey is a 3-tuple** `(locationID, flagID, typeID)`
`[V]` `godma:1863` (`itemLocationID, itemFlag, itemTypeID = itemKey`). So loaded ammo is addressed as
`(shipID, 27, 185)` — "the EMP S in high slot 0" — and **carries no itemID of
its own**.

In `shipInfo` it is one more dict entry whose key is that tuple, and whose
`attributes` **must** contain `attributeQuantity = 805`: `godma:1904`
logs `'Godma:UpdateItem - QUANTITY MISSING'` and refuses otherwise.

### 6.2 Rounds remaining `[V]`

- `smb:342 SetCharge(charge)` reads `charge.stacksize`, `charge.typeID`,
  `charge.itemID`.
- `godma:2191-2192`: `GetAttribute(subLocationKey, 'stacksize')` is rewritten
  to `'quantity'`, i.e. **attributeID 805**.
- `smb:363 UpdateChargeQuantity` writes `FmtAmt(charge.stacksize)` into the
  little label, and hides it entirely for crystal groups.

**So rounds remaining = attribute 805 on the sublocation.** Decrementing it is
one `OnModuleAttributeChange` with `itemKey = (shipID, flagID, typeID)`.

**An empty gun** is `newValue == 0`: `godma:3040-3059` (the
`attributeQuantity` branch of `ApplyAttributeChange`) deletes the sublocation from
`sublocationsByLocationFlag` and `sublocationsByKey`, scatters `OnItemChange`
with `{ixStackSize: oldValue}`, and `godma:1620 OnModuleAttributeChanges`
suppresses every *other* change for that same key in the same batch
(`qtyChanges`) — so if you batch, put the zero-quantity change in with the
rest and the client will do the right thing.

Independently, `smb:1510` calls `AutoReload()` when an effect stops with
`self.quantity == 0`.

### 6.3 Reloading `[V]`

```text
OnChargeBeingLoadedToModule(itemIDs, chargeTypeID, time)      # 3 args
```

`smb:750` (module buttons; `time` is milliseconds —
`params['time'] = time / 1000` and `DoReloadAnimation(time)`) and
`out/pyc/eve/client/script/parklife/turret.pyc` `OnChargeBeingLoadedToModule(moduleIDs,
chargeTypeID, reloadTime)` (plays the turret reload animation).
`chargeTypeID = None` **cancels** the animation (`smb:752`-area).
`findings/push-events.tsv` has both declarations.

A second, state-based path exists and is what survives a relog `[V]`
`godma:2666`:

```python
def GetReloadTimes(self, moduleID):
    nextCycle = self.GetAttributeValueByID(moduleID, const.attributeNextActivationTime)   # 2729
    if nextCycle is None or nextCycle < blue.os.GetSimTime(): return
    duration = self.GetAttributeValueByID(moduleID, const.attributeReloadTime)            # 1795
    return (nextCycle - duration * const.MSEC, duration)
```

**Trap:** `IsModuleReloading` is `GetReloadTimes(...) is not None`
(`godma:2662`), and `ModuleButton.Click` **returns immediately** when it is
true (`smb:1076-1078`). A `nextActivationTime` (2729) left in the future makes
the module permanently unclickable with no message. Do not set that attribute
until reload is implemented.

For the 200mm AutoCannon I: `reloadTime = 10000.0` ms, `chargeSize = 1.0`,
`chargeGroup1 = 83.0` (Projectile Ammo S) `[V]`.

### 6.4 "No charges" is raised **client-side** `[V]` `smb:1539`

```python
relevantEffect = self.GetRelevantEffect()
if relevantEffect is None:
    raise UserError('NoCharges', {'launcher': (const.UE_TYPEID, typeID)})
```

`GetRelevantEffect` (`smb:1515`) returns the *charge's* default effect for
`useMissiles` and for `warpDisruptSphere`-with-a-charge, and otherwise
`self.def_effect`. So **a launcher with no ammo never reaches the server** —
the client refuses first. That is why `Light Missile Launcher I`'s own
`useMissiles` effect has `guid = ''` (`[V]` `dogmaeffects[101]`): the visual
guid comes from the loaded missile type, not the launcher.

### 6.5 What we already model

| thing the client needs | already in our model? |
| --- | --- |
| module as an item with slot flag | **yes** — `Item{owner,location,flag,quantity,singleton}` |
| `singleton = true` on the module | **yes** (`item.rs:51`) |
| ammo as a sublocation `(ship, flag, typeID)` with attribute 805 | **no** — this is a new shape, but it is derivable from an `Item` whose location is the module and whose flag is a slot flag |
| `reloadTime`, `chargeSize`, `chargeGroup1` on the module | **client FSD** — we only need to serve them in `attributes` |
| reload progress | **no** — needs attribute 2729, and §6.3's trap says not to fake it |

---

## 7. Failure cases

### 7.1 What the client refuses before the server sees anything `[V]`

| situation | what happens | where |
| --- | --- | --- |
| module reloading | silent return; the click does nothing | `smb:1076` |
| no `def_effect` (passive module, e.g. Damage Control I) | `prompt_player_fsd_dialog('CustomNotify', {'notify': GetByLabel('UI/Inflight/ModuleRacks/TryingToActivatePassiveModule')})` | `smb:1084-1087` |
| module offline | `prompt_player_fsd_dialog('ClickOffllineModule')`, or `'ClickOffllineGroup'` for a bank master (CCP's own typo, two `l`s) | `smb:1088-1093` |
| already activating in godma | silent return | `smb:1095-1097` |
| **category-2 effect with no lock** | **no call at all.** `target.OrderTarget(self)`, `BlinkIcon`, `waitingForActiveTarget = 1` — the cursor turns into a target cursor and the module fires when the player picks one (`smb:1627 OnStateChange` on `state.activeTarget`) | `smb:1541-1547` |
| offensive effect needing a confirmation | `consider.DoAttackConfirmations(targetID, effect)` can veto | `smb:1560-1562` |
| launcher with no ammo | `UserError('NoCharges', {'launcher': (UE_TYPEID, typeID)})` | `smb:1539` |

**So "activating with no lock" is not a server error case at all** — the
client turns it into a target order. The server will simply never be called.

### 7.2 Refusing at the call: raise a `UserError`

Our server already raises `eveexceptions.UserError(msg, dict)` with marshal
tag `0x22` (`docs/re/errors.md` §1.2, `service.rs:5254`). `ServiceCallGPCS`
re-raises it immediately, and the generic handler does
`prompt_player_fsd_dialog(exc.msg, exc.dict)` (`docs/re/errors.md`:115), so
**a raised key is shown with no extra work.**

The right keys, from `findings/user-errors.tsv` (key · fsd id · loc args). The `(tag, value)`
pairs below use `UE_TYPEID = 4` and `UE_GROUPID = 7` `[V]`
(`out/pyc/eveexceptions/const.pyc:5,9`):

| case | key | id | args the dialog wants |
| --- | --- | --- | --- |
| effect already running | `EffectAlreadyActive2` | 3252 | `modulename` → `(UE_TYPEID, typeID)` |
| still in reactivation delay | `ModuleReactivationDelayed2` | 2307 | loc: `delay` (timeinterval), `item`; **dict must also carry `timeSinceLastStop` (ms) and `itemID`** |
| module not online | `ModuleNotOnline` | 1231 | `module` → `(UE_TYPEID, typeID)` |
| already active (modal) | `ModuleAlreadyActive` | 1224 | none |
| target gone | `DeniedActivateTargetNotPresent` | 777 | none |
| out of range | `TargetTooFar` | 1632 | `module` → `(UE_TYPEID, typeID)`, `distance` (numeric), `targetname` |
| out of range, with both numbers | `TargetLostNotWithinRange` | 1625 | `targetGroupID`, `actualRange`, `desiredRange` |
| negative duration (a data bug) | `EffectCantHaveNegativeDuration` | — | `modulename`, `duration` — CCP's own, `bdl:1028` |
| mining with no asteroid locked | `CannotActivateModuleRequireAsteroidLock` | 5020 | none |
| reloading | `CantActivateReloadingModule` | 328 | `moduleTypeID` |

`EffectAlreadyActive2` is CCP's own choice here: `bdl:926` and `bdl:931` raise
exactly that, with exactly `{'modulename': (const.UE_TYPEID, dogmaItem.typeID)}`
`[V]`. **Prefer it over inventing one.** `findings/user-errors.tsv` also flags
it as the only key that does *not* make `clientDogmaLocation.OnlineModule`
revert its optimistic state — relevant to onlining, not to activation.

**`ModuleReactivationDelayed2` is handled specially** `[V]` `smb:1582`:

```python
except UserError as e:
    if e.msg == 'ModuleReactivationDelayed2':
        self.SetReactivationTime(e.dict)
    raise
```

and `SetReactivationTime` (`smb:1606`) reads `errorDict['timeSinceLastStop']`
and `errorDict['itemID']`, computes `startTime = GetSimTime() - timeSinceLastStop * MSEC`,
and draws the cooldown ring. So that dict needs **four** keys: the two loc
args plus `timeSinceLastStop` and `itemID`.

### 7.3 Stopping an effect with a reason: the `error` field

`OnGodmaShipEffect`'s `error` is `(dialogKey, paramsDict)` and
`shipUI.ProcessShipEffect` feeds it straight to `prompt_player_fsd_dialog`
`[V]`. That is the channel for "your laser shut off because the target left
range": send `start=0, active=0, error=('TargetTooFar', {...})`. The server
does not need a second message.

### 7.4 Out of capacitor

`[V]` The client has the key: `NotEnoughEnergy` (259233), "{effectname}
requires {need} units of charge. The capacitor has only {got} units." with
three args, `effectname`, `need` and `got` (numbers, one decimal). This
replaces the earlier `[A]` that said there was no such key and the module
idled silently (it looked for a `NotEnoughCapacitor*` key and found only
`NotEnoughCapacitorForOnline`, which is onlining's).

So, on insufficient capacitor:

- **At activation** `activate` raises `UserError('NotEnoughEnergy', {effectname,
  need, got})` and starts nothing (every path still ends in a reply or a
  raise, §7.5).
- **Mid-run**, at a cycle boundary the capacitor cannot pay, the module stops
  and its `OnGodmaShipEffect(start=0, active=0, error=('NotEnoughEnergy',
  {...}))` carries the same tuple (§7.3).

`[A]` that retail raises at activation rather than idling the module, and
that `effectname` is the module's type name. Falsified by a retail capture
of a silent idle on an out-of-cap click, or of a different `effectname`.
Code: `modules.rs` `not_enough_energy`, `activate`, `advance`.

### 7.5 The failure case that will actually bite unattended code

**Not replying at all.** `godma:2439`'s `TryLockItem` is released in a
`finally` that only runs when the RPC returns or raises. A dropped call leaves
the module locked in `invCache` with no timeout and no message. Every
`dogmaIM.Activate` must terminate in a reply or a `UserError`.

### 7.6 Activation rules the server enforces (module-rules)

- **Warp shutdown** `[V]` The effect table's `isWarpSafe` column (85 of the
  3427 effects are safe; afterburners and microwarpdrives are not) rides in
  `typedata.tsv` as the last `effect` column (`Effect::warp_safe`, `None` for
  an old extract: nothing is stopped). `modules::tick` stops a running
  non-safe effect once its pilot's warp is active (`Motion::warp_active_at`,
  the instant `targeting` drops locks) with error `EffectDeactivationWarping`
  (259511, no args).
- **`maxGroupActive`** `[V]` Attribute 763, and the hull's 909 / 910 pair
  (the group the hull limits and the limit): `modules::check_group_active`,
  the client's `_CheckIfTooManyModulesInGroupAreActive`. Active fitted
  modules of the group, a stopping one included, `>=` the limit refuse with
  `EffectCrowdedOut{module: bare type id, count}`.
- **Reactivation delay** `[V]` Attribute 669 (ms): a module whose effect
  ended less than that long ago is refused with
  `ModuleReactivationDelayed2{timeSinceLastStop (ms), itemID, delay, item}`;
  the client draws the cooldown ring from the first two. `[A]` `delay` is the
  time left in blue ticks and `item` a `(UE_TYPEID, id)` pair. The last-stop
  time is cleared when the module leaves its ship or the ship dies.

---

### 7.7 Local tank: the cap booster and the ancillary repairers (local-tank)

- **Cap booster (effect 48)** is a `remote_rep` local row whose amount is the
  **loaded charge's** attribute 67 (`capacitorBonus`) [V: the effect has no
  modifier rows]. The charge is consumed when the cycle starts, so
  `modules` records what each cycle fired (`Fired`: charge type, rounds taken,
  `chargeRate`) and `remote_rep` reads it by cycle index.
- **Ancillary repairers (4936, 5275, 6651, 6652)** activate and cycle with
  nothing loaded (`is_ancillary`). `fire_rounds` takes `min(rate, loaded)`;
  an AAR repairs `base x (1 + (mult - 1) x taken / rate)` [A: the proportional
  blend; falsifier: a retail log of a half-fed cycle]; an empty ASB pays its
  capacitor. Switched off empty with charges in the cargo, the stop (at the
  cycle boundary, not the click: a running module cannot be loaded) loads the
  first fitting stack through `load_one` and starts the reload (1795).
- **A charge that does no damage** (paste, cap charges) is not in the weapon
  table, so `load_one` reads its own `chargeSize` (128) for the fit test.
- Not done (deferred [A]): armour reps landing at cycle end, the Reactive
  Armor Hardener.

### 7.8 Neutralizers, nosferatu and the NPC capacitor (neut-nos)

- **Player 6187 / 6197** are `ewar::Kind::Neut` / `Nos` (buff slots
  `ewEnergyNeut`, `ewEnergyVampire` [V]). They act once per source cycle, at
  `falloff x resistance x min(1, sig / 2451)`. The player modules carry no
  2138, so the target's 2045 is the resistance [V typedata].
- **Neut** drains attribute 97 x weight. **Nos** moves `min(90 x weight,
  target cap, room in the source)` and does nothing while the source holds at
  least the target's capacitor, unless 1945 > 0 [A: GJ, not percent;
  falsifier: a live nosferatu on a fuller-by-GJ ship].
- **An NPC has a capacitor** (482, 55 of its type, via the hull table) in the
  same store as a ship's. A player neut or nos drains it; it pays each
  repair, tackle and neut cycle the effect's `dischargeAttributeID` and does
  nothing when it cannot [A].
- **NPC neuts**: 6691 (942 cycle, 98 range, 97 amount, 931 chance) and 6756
  (2519 cycle, 2520 range, 2521 falloff, no chance; **[A] amount is 97**).
  `NEUT` joined `npc_gen::SUPPORTED`. 6882 (NPC nosferatu) is not done: its
  types fly as rats without it [A].

## 8. Falsifiable tests

Properties, not our implementation. Each one can be written against a
transcript or a live client.

**T1 — the activate call is a bound-object call on `dogmaIM`.**
Click a fitted gun with a lock. Exactly one outbound call appears, its
destination is a bound object whose service is `dogmaIM` and whose bind key is
`(solarsystemID, 5)`, its method is `Activate`, and it has 4 positional
arguments of types `(int, str, int|None, int)`.
*Fails if* it is a plain service call, or the arity is not 4.

**T2 — the reply's value is never read.**
Reply with `None`, then with `True`, then with a `KeyVal`. The client's
subsequent behaviour is byte-identical in all three.
*Fails if* any reply shape changes the HUD.

**T3 — no priming, no effect.**
Send a well-formed `OnGodmaShipEffect` for an `itemID` that was never in
`shipInfo`. Nothing changes and nothing raises client-side.
*Fails if* the button reacts — which would mean §2.3 is wrong and the row is
created lazily.

**T4 — one message, unbounded cycling.**
Send exactly one `OnGodmaShipEffect(start=1, active=1, duration=3750,
repeat=1000, startTime=None)` and then nothing for five minutes. The ramp
keeps sweeping, with a period of 3750 ms ± the frame rate, for the whole five
minutes.
*Fails if* the ramp stops, or if its period is not the duration we sent — the
latter would mean the attribute (not the field) won, which is §3.2's point 1
and also fine, but then the *attribute* must be 3750.

**T5 — the duration comes from the attribute first.**
Serve `shipInfo[module].attributes[51] = 1000.0` while sending
`duration = 3750` in the message. The ramp period is **1000 ms**.
*Fails if* it is 3750 — which would mean `GetEffectTiming`'s order is reversed.

**T6 — `startTime=None` anchors on the client's clock.**
Send `startTime=None`. `portionDone` starts at ~0 and the first sweep is a
full one. Then send a `startTime` one hour in the past; the behaviour is
identical within one duration (the `portionDone > 1` branch normalises it).
Then send one one hour in the *future*: the ramp misbehaves.
*Fails if* `None` is rejected or produces a jump.

**T7 — `environment` is read at indices 0, 2 and 3 only.**
Vary indices 1, 4, 5 and 6 arbitrarily (with `[6]` still the real effectID,
which `dogma.data.get_effect` needs at index-1 of the *message*, not here).
The button behaves identically. Then set `environment[2]` to anything other
than `session.shipid`: `ProcessShipEffect` is never chained and the button
does **not** go active.
*Fails if* index 2 does not gate the HUD.

**T8 — capacitor recharge needs no pushes.**
Serve `charge = 125.0`, `capacitorCapacity = 250.0`, `rechargeRate = 125000.0`
on the ship, with `time = None`, and push nothing further. The bar rises
along `(1 + (sqrt(0.5) - 1) e^{-t/25000})^2 · 250` with t in ms, reaching
90% of capacity at t = 43.5 s and 75% at t = 19.6 s.
*Fails if* the bar is static, or if it rises linearly.

**T9 — a `charge` push with a stale timestamp snaps the bar.**
Push `attributeID = 18` with `time = 0`. The displayed value jumps to the cap
rather than to `newValue`.
*Fails if* it shows `newValue` — which would mean `charge` is not being
promoted to a charged attribute, i.e. that `chargeRechargeTimeID` is not set
on attribute 18 in this build's data.

**T10 — the visual needs `OnSpecialFX` and the HUD does not.**
Send `OnGodmaShipEffect` alone: the button cycles, nothing happens in space.
Send `OnSpecialFX` alone: the gun animates, the button stays idle.
*Fails if* either message does both.

**T11 — `OnSpecialFX` is dropped without balls.**
Send it with a `targetID` that is not in the ballpark. Nothing is drawn, and
the client logs "target not in ballpark. Trigger ignored".
*Fails if* something is drawn anyway.

**T12 — a module with no `durationAttributeID` does not animate.**
Pick an effect whose `durationAttributeID` is `None` and send
`duration = -1`. `GetEffectTiming` returns 0 and `DoActivateRampsThread`
returns immediately — the button goes active (glow) but no ramp sweeps.
*Fails if* a ramp appears.

**T13 — a non-singleton module is invisible.**
Serve a fitted module with `singleton = false`. It does not appear on the HUD
and `UpdateItem` logs `Godma:UpdateItem.skip3`.
*Fails if* it appears — which would mean `godma:1939`'s guard is read wrong.

**T14 — `nextActivationTime` makes the module unclickable.**
Serve attribute 2729 on the module with a future blue filetime. Clicking does
nothing, silently.
*Fails if* the click goes through.

**T15 — a refusal is shown by key.**
Raise `UserError('EffectAlreadyActive2', {'modulename': (4, 486)})` from
`dogmaIM.Activate` (`UE_TYPEID = 4` `[V]`
`out/pyc/eveexceptions/const.pyc:5`; `UE_GROUPID = 7`, line 9). The client shows the log-line notification for fsd dialog 3252 naming
the module, and the module's item lock is released.
*Fails if* nothing is shown, or the item stays locked.

**T16 — ammo count is attribute 805 on a tuple key.**
Serve a sublocation `(shipID, 27, 185)` with `attributes = {805: 100.0}`.
The gun's button shows "100". Push `OnModuleAttributeChange(charID,
(shipID,27,185), 805, t, 99.0, 100.0, 0)`; it shows "99". Push it to `0.0`;
the charge icon reverts to the module's own icon and the quantity label hides.
*Fails if* the label never appears — most likely because `attributes` lacked
805 and `UpdateItem` logged `QUANTITY MISSING`.

---

## 9. Open questions, ranked by cost if wrong

1. **The sim-clock epoch.** `blue.os.GetSimTime()` is a *simulated* clock the
   client maintains and that the server rebases with `DoSimClockRebase`. This
   lane did not establish whether it equals `service::filetime_now()` on
   session start. **Cost if wrong: zero for the minimum spec**, because
   `startTime = None` sidesteps it (§3.3) — but it is load-bearing for
   `actualStopTime`, for reload progress (2729), and for any `charge` push
   whose `time` must be comparable to the client's clock. **Settle it before
   pushing a capacitor drain.** Cheapest experiment: push a `charge` change
   with `time = filetime_now()` and watch whether the bar lands on `newValue`
   (epoch agrees) or snaps to the cap (it does not) — that is T9.

2. **`ConsumeResources` is empty in the shipped base class** (`bdl:1017`), so
   the *exact* capacitor arithmetic (does `capacitorNeed` come off at cycle
   start or cycle end? is it scaled by anything?) is node-only code we do not
   have. **Cost if wrong: a capacitor that drains at the wrong rate.** Visible
   but not broken. `[A]` subtract `capacitorNeed` once per cycle at cycle
   start.

3. **`otherTypeID` in `OnSpecialFX`.** `[A]` the charge's typeID for a gun,
   `None` otherwise. The field is `discard`ed by five of the seven consumers
   in `findings/push-events.tsv` and only `spaceMgr` and `fxSequencer` read
   it. **Cost if wrong: the wrong tracer colour, or none.** Falsified by a
   capture of a real server firing a gun.

4. **Weapon banks.** `slots:284` skips any module for which
   `dogmaLocation.IsModuleSlave(itemID, shipID)` is true, and
   `turret.ProcessShipEffect` redirects through
   `dogmaLocation.IsInWeaponBank`. Both read `wbData`, which
   `service.rs`'s `shipState` currently sends as `None` — and `SetWeaponBanks`
   opens `if data is None: return`. **So no module is ever a slave today**,
   which is the behaviour we want. **Cost if wrong: zero now**; it becomes a
   question the day grouping is implemented.

5. **Whether the local dogma engine needs telling.**
   `clientDogmaLocation` does **not** listen to `OnGodmaShipEffect` (its
   `__notifyevents__` at `cdl`'s class body is nine names, none of them
   godma effect events) `[V]`. So the client's *own* dogma simulation does not
   learn that the afterburner is running, and will not apply its speed bonus
   locally. **Cost if wrong: module bonuses do not show in the fitting
   window's simulated stats.** Not visible in the activation feature. The
   real path for a bonus is an `OnModuleAttributeChange` on the bonused
   attribute, which is the same machinery as §4.3.

6. **`t` versus `startTime`.** Both are blue filetimes and this lane could not
   find a read of `t` that affects anything visible (`godma:1750` puts it in
   `effectState.time`; `TryRecordLastStopTime` passes its *own*
   `GetSimTime()`). `[A]` send the same value for both, or `None` for
   `startTime` and `filetime_now()` for `t`. **Cost if wrong: nothing
   observed.**

---

## 10. One-paragraph summary for whoever builds this

Put the fitted module in `dogmaIM.GetAllInfo`'s `shipInfo` as a singleton item
with a slot flag and an `attributes` dict that contains its duration
attribute. Accept `dogmaIM.Activate(itemID, effectName, target, repeat)` on the
existing `(solarsystemID, 5)` bound object and return `None`. Push
`OnGodmaShipEffect(itemID, effectID, t, 1, 1, [itemID, charID, shipID,
targetID, None, [], effectID], None, durationMs, repeat, None, None)` to
`svc.godma`. The module's button goes active and cycles forever on the
client's own clock. Push `OnSpecialFX(shipID, moduleID, moduleTypeID,
targetID, otherTypeID, guid, isOffensive, 1, 1, durationMs, repeat, None, 0,
None)` to `svc.FxSequencer` and the gun fires in space, for the shooter and
every observer. Mirror both with `start=0, active=0` on
`dogmaIM.Deactivate(itemID, effectName)`. Everything else — the duration, the
effect's name, category, guid and capacitor cost — is in the client's own
shipped data, read with `tools/dogma.py`.
