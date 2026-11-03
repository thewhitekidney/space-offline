# R-WH: wormholes, an implementation spec

Research lane R-WH of W12 (`docs/content/plan.md` §6.2). It feeds **CP54a**
(statics and signatures) and **CP54b** (jumps, mass and decay). It writes no
code.

Everything here was read from build 3619879 (`tq-offline`): bytecode via
`tools/dis.py` (the client's own `co_lnotab` line numbers), dogma via
`tools/dogma.py`'s `Dogma` class, and static data via `tools/fsd.py` and
`tools/fsd_dump.py`, all run in `tools/analysis.sh`. Inputs: C1 §2.1,
C3 §4, `docs/re/jump-gates.md`, C2 §2.

**Markers.** **[V]** means read from this client's bytecode or data. **[I]**
is inferred from [V] facts; the falsifier is named. **[A]** is an assumption
we choose to ship (the falsifier is named, and it must never be pinned in a
test, per ruling 8). **unknown** is not settled here.

---

## 0. The answer, in short

- **The jump is a plain service call, not a bound call:**
  `sm.RemoteSvc('wormholeMgr').WormholeJump(itemID)`, wrapped in
  `sessionMgr.PerformSessionChange`, which discards the reply. The client
  approaches to **5,000 m** first (`const.maxWormholeEnterDistance`) [V].
- **The pushes are the gate jump's pushes.** There are two changes:
  - the departure FX guid is `effects.JumpOutWormhole`, whose
    `otherTypeID` slot carries the **destination wormhole class**;
  - the session change carries `constellationid` and `regionid` whenever
    they change, which is every k-space/J-space crossing and most J-to-J
    jumps.
  
  One new push exists, `OnWormholeJumpCancel()`, which takes no arguments
  [V].
- **A wormhole is a ball of group 988** whose slim item carries
  `wormholeSize`, `wormholeAge`, `maxShipJumpMass`, `nebulaType`,
  `otherSolarSystemClass` and `isDestTriglavian`. Three of these are
  **small enumerations, not physical values** (§3) [V].
- **The six dogma attributes per type are [V].** Mass is in kg. Lifetime is
  in minutes [I]. K162 (30831) has **no dogma row at all** [V], so a K162
  takes every number from its partner.
- **CCP models each wormhole as an archetype-38 dungeon.** The room holds
  the wormhole object and one `Deadspace Signature` (19728) [V-data], so a
  wormhole signature is a C3 site with a **fixed** dungeon per type (§5).
- **The client ships no statics** [V: no table has them; EvEJS's table is
  authored, `evejs-data.md` §4.3]. However, the dungeon-id blocks group the
  wormhole types into **one block of nine per source class**, and each block
  reaches classes 1-9 once (§6.1) [I]. That gives the proposed
  `statics.tsv` (§6.3) [A].

---

## 1. The wormhole types and their attributes [V]

### 1.1 Identity

| fact | value | source |
|---|---|---|
| group | `groupWormhole = 988`; 132 types, **all `published = False`**, `radius = 3000.0`, `mass = 0.0` | `inventorycommon/const.pyc`; `types.fsdbinary` [V] |
| K162 | typeID **30831**, "Wormhole K162", **no `typedogma` row** | `dogma.Dogma.of_type(30831)` raises `KeyError` [V] |
| non-interactable | `typeNonInteractableWormhole = 84329`, `…Open = 87593` (scenery; not group 988's jump path) | `inventorycommon/const.pyc` [V] |
| archetype | `dunArchetypeWormhole = 38` | `appConst.pyc:574` [V] |
| scan attribute | `siteTypeWormhole = attributeScanWormholeStrength` = attribute **1908** `scanWormholeStrength` | `probescanning/explorationSites.pyc:20`, `dogmaattributes` [V] |
| class constants | `INVALID 0`, `HIGH_SEC 7`, `LOW_SEC 8`, `ZERO_SEC 9`, `TRIGLAVIAN 25` | `appConst.pyc:1896-1900` [V] |
| id ranges | systems `31000000..31999999`, constellations `21000000..21999999`, regions `11000000..11999999` (`IsWormholeSystem` etc.) | `inventorycommon/const.pyc`, `idCheckers.pyc:92-101` [V] |

### 1.2 The attributes

| id | name | unit | meaning (CCP's own `description`) |
|---|---|---|---|
| 1381 | `wormholeTargetSystemClass` | none | destination class (1-6 C1-C6, 7 HS, 8 LS, 9 NS, 12 Thera, 13 shattered, 14-18 Drifter, 25 Pochven, −1 Pochven-internal, 10/11 QA) |
| 1382 | `wormholeMaxStableTime` | 3 "Time" | "maximum amount of time a wormhole will stay open" |
| 1383 | `wormholeMaxStableMass` | 2 "Mass" (kg) | "maximum amount of mass a wormhole can transit before collapsing" |
| 1384 | `wormholeMassRegeneration` | 2 (kg) | "amount of mass a wormhole regenerates per cycle" |
| 1385 | `wormholeMaxJumpMass` | 2 (kg) | "maximum amount of mass that can transit a wormhole in one go" |
| 1457 | `wormholeTargetDistribution` | none | "distribution ID of the target wormhole distribution" |

All six are `published = False` with `defaultValue = 0.0` [V]. Other
attributes exist but **no type uses them**: `wormholeTargetRegion1..9`
(1386-1394), `…Constellation1..9` (1395-1403) and `…System1..9` (1404-1412)
[V, unused on all 132 rows].

- **Lifetime is in minutes [I].** Unit 3 is "Time", which reads as seconds,
  but the values are 270, 720, 960, 1440 and 2880. As seconds these would
  be 4.5 to 48 *minutes*, and every wormhole would be gone before anyone
  could scan it. As minutes they are 4.5 h, 12 h, 16 h, 24 h and 48 h.
  **Falsified by** nothing in the client: the info window shows only an age
  bucket (§3).
- **Regeneration is non-zero only on the frigate holes** (E004, L005, Z006,
  M001, C008, G008, Q003, A009), where it is `3e9` against a max of `3e9`
  [V]. The "cycle" is **unknown** [I]. Since regeneration ≥ max, treat these
  holes as never collapsing by mass.
- **`wormholeTargetDistribution`** is 296 + (class − 1) for classes 1-9 on
  the main set (296..304) [V-data]. What a distribution *is* stays
  **unknown**, because no client table is keyed by it. CP54a does not need
  it; `statics.tsv` replaces it.

### 1.3 The values that v1 uses

`dest` is attr 1381, `life` is 1382 in minutes, `max` is 1383, `jump` is
1385, and `dun` is the archetype-38 dungeons holding the type alone (§5)
[V-data]. Regeneration is 0 on every row below.

**J-space source sets** (one block of nine per source class, §6.1):

| src | type | code | dest | life | max | jump | dun |
|---|---|---|---|---|---|---|---|
| C1 | 30660 | H121 | 1 | 960 | 5e8 | 6.2e7 | 3639 |
| C1 | 30661 | C125 | 2 | 960 | 1e9 | 6.2e7 | 3640 |
| C1 | 30662 | O883 | 3 | 960 | 1e9 | 6.2e7 | 3641 |
| C1 | 30663 | M609 | 4 | 960 | 1e9 | 6.2e7 | 3642 |
| C1 | 30664 | L614 | 5 | 1440 | 1e9 | 6.2e7 | 3643 |
| C1 | 30665 | S804 | 6 | 1440 | 1e9 | 6.2e7 | 3644 |
| C1 | 30666 | N110 | 7 | 1440 | 1e9 | 6.2e7 | 3645 |
| C1 | 30667 | J244 | 8 | 1440 | 1e9 | 6.2e7 | 3646 |
| C1 | 30668 | Z060 | 9 | 960 | 1e9 | 6.2e7 | 3647 |
| C2 | 30671 | Z647 | 1 | 960 | 5e8 | 6.2e7 | 3630 |
| C2 | 30672 | D382 | 2 | 960 | 2e9 | 3.75e8 | 3631 |
| C2 | 30673 | O477 | 3 | 960 | 2e9 | 3.75e8 | 3632 |
| C2 | 30674 | Y683 | 4 | 960 | 2e9 | 3.75e8 | 3633 |
| C2 | 30675 | N062 | 5 | 1440 | 3e9 | 3.75e8 | 3634 |
| C2 | 30676 | R474 | 6 | 1440 | 3e9 | 3.75e8 | 3635 |
| C2 | 30677 | B274 | 7 | 1440 | 2e9 | 3.75e8 | 3636 |
| C2 | 30678 | A239 | 8 | 1440 | 2e9 | 3.75e8 | 3637 |
| C2 | 30679 | E545 | 9 | 960 | 2e9 | 3.75e8 | 3638 |
| C3 | 30680 | V301 | 1 | 960 | 5e8 | 6.2e7 | 3621 |
| C3 | 30681 | I182 | 2 | 960 | 2e9 | 3.75e8 | 3622 |
| C3 | 30682 | N968 | 3 | 960 | 2e9 | 3.75e8 | 3623 |
| C3 | 30683 | T405 | 4 | 960 | 2e9 | 3.75e8 | 3624 |
| C3 | 30684 | N770 | 5 | 1440 | 3e9 | 3.75e8 | 3625 |
| C3 | 30685 | A982 | 6 | 1440 | 3e9 | 3.75e8 | 3626 |
| C3 | 30686 | S047 | 7 | 1440 | 3e9 | 3.75e8 | 3627 |
| C3 | 30687 | U210 | 8 | 1440 | 3e9 | 3.75e8 | 3628 |
| C3 | 30688 | K346 | 9 | 960 | 3e9 | 3.75e8 | 3629 |
| C4 | 30689 | P060 | 1 | 960 | 5e8 | 6.2e7 | 3612 |
| C4 | 30690 | N766 | 2 | 960 | 2e9 | 3.75e8 | 3613 |
| C4 | 30691 | C247 | 3 | 960 | 2e9 | 3.75e8 | 3614 |
| C4 | 30692 | X877 | 4 | 960 | 2e9 | 3.75e8 | 3615 |
| C4 | 30693 | H900 | 5 | 1440 | 3e9 | 3.75e8 | 3616 |
| C4 | 30694 | U574 | 6 | 1440 | 3e9 | 3.75e8 | 3617 |
| C4 | 30695 | D845 | 7 | 1440 | 5e9 | 3.75e8 | 3618 |
| C4 | 30696 | N290 | 8 | 1440 | 3e9 | 3.75e8 | 3619 |
| C4 | 30697 | K329 | 9 | 960 | 3e9 | 3.75e8 | 3620 |
| C5 | 30698 | Y790 | 1 | 960 | 5e8 | 6.2e7 | 3603 |
| C5 | 30699 | D364 | 2 | 960 | 1e9 | 3.75e8 | 3604 |
| C5 | 30700 | M267 | 3 | 960 | 1e9 | 3.75e8 | 3605 |
| C5 | 30701 | E175 | 4 | 960 | 2e9 | 3.75e8 | 3606 |
| C5 | 30702 | H296 | 5 | 1440 | 3.3e9 | 2e9 | 3607 |
| C5 | 30703 | V753 | 6 | 1440 | 3.3e9 | 2e9 | 3608 |
| C5 | 30704 | D792 | 7 | 1440 | 3e9 | 1e9 | 3609 |
| C5 | 30705 | C140 | 8 | 1440 | 3.3e9 | 2e9 | 3610 |
| C5 | 30706 | Z142 | 9 | 960 | 3.3e9 | 2e9 | 3611 |
| C6 | 30707 | Q317 | 1 | 960 | 5e8 | 6.2e7 | 3594 |
| C6 | 30708 | G024 | 2 | 960 | 2e9 | 3.75e8 | 3595 |
| C6 | 30709 | L477 | 3 | 960 | 2e9 | 3.75e8 | 3596 |
| C6 | 30710 | Z457 | 4 | 960 | 2e9 | 3.75e8 | 3597 |
| C6 | 30711 | V911 | 5 | 1440 | 3.3e9 | 2e9 | 3598 |
| C6 | 30712 | W237 | 6 | 1440 | 3.3e9 | 2e9 | 3599 |
| C6 | 30713 | B520 | 7 | 2880 | 3e9 | 1e9 | 3600 |
| C6 | 30714 | C391 | 8 | 2880 | 3.3e9 | 2e9 | 3601 |
| C6 | 30715 | C248 | 9 | 960 | 3.3e9 | 2e9 | 3602 |

**K-space source sets.** The low/null block is used twice, as dungeons
3567-3575 and 3576-3584. The high-sec block is dungeons 3585-3593:

| src | type | code | dest | life | max | jump |
|---|---|---|---|---|---|---|
| LS, NS | 30579 | Z971 | 1 | 960 | 1e8 | 6.2e7 |
| LS, NS | 30583 | R943 | 2 | 960 | 7.5e8 | 3.75e8 |
| LS, NS, HS | 30584 | X702 | 3 | 1440 | 1e9 | 3.75e8 |
| LS, NS, HS | 30642 | O128 | 4 | 1440 | 1e9 | 3.75e8 |
| LS, NS | 30643 | N432 | 5 | 1440 | 3.3e9 | 2e9 |
| LS, NS | 30646 | U319 | 6 | 2880 | 3.3e9 | 2e9 |
| LS, NS | 30647 | B449 | 7 | 960 | 2e9 | 1e9 |
| LS, NS | 30648 | N944 | 8 | 1440 | 3.3e9 | 2e9 |
| LS, NS | 30649 | S199 | 9 | 960 | 3.3e9 | 2e9 |
| HS | 30579, 30583 | Z971, R943 | 1, 2 | (as above) | | |
| HS | 30644 | M555 | 5 | 1440 | 3e9 | 1e9 |
| HS | 30645 | B041 | 6 | 2880 | 3e9 | 1e9 |
| HS | 30657 | A641 | 7 | 960 | 2e9 | 1e9 |
| HS | 30658 | R051 | 8 | 960 | 3e9 | 1e9 |
| HS | 30659 | V283 | 9 | 960 | 3e9 | 1e9 |

**Left out of v1:**

- **Thera:** T458, M164, L031, Q063, V898, E587, F353, F135 (34338,
  34366-34372).
- **Frigate holes:** E004…Q003 and A009 (34134-34140, 34439).
- **Drifter:** S877…R259 (35650-35654).
- **Pochven:** C729 ×28, X450, R081, F216 ×2, U372, I078, L687, O546, R327,
  O753.
- **Other types:** J244/J377/J492 variants (73748, 73749, 87827), and the
  QA pair (32894, 32895).

Their rows are in `typedogma` like the rest [V]; adding them is a table row,
not code.

---

## 2. The jump call and the pushes

### 2.1 The call [V]

```
double-click / menu on a group-988 ball
  menusvc.EnterWormhole(itemID)                                   menusvc.pyc:3571
    GetCloseAndTryCommand(itemID, movementFunctions.RealEnterWormhole, (itemID,),
                          interactionRange=const.maxWormholeEnterDistance)   # 5000 m, appConst.pyc:1735
  movementFunctions.RealEnterWormhole(itemID)                     movementFunctions.pyc:637
    :638  isHighSec = securitySvc.is_effective_high_sec_system(session.solarsystemid)
    :640  if isHighSec and prompt('WormholeJumpingFromHiSec', YESNO, suppress=ID_YES) != ID_YES: return
    :643  log 'Wormhole Jump from', session.solarsystemid2, 'to', itemID
    :644  sessionMgr.PerformSessionChange(GetByLabel('UI/Inflight/EnterWormhole'),
                                      sm.RemoteSvc('wormholeMgr').WormholeJump, itemID)   # reply POP_TOP'd
```

- **Service `wormholeMgr`, method `WormholeJump`, one positional int: the
  wormhole ball's itemID.** It is not bound, and there is no `shipID`
  argument. The reply is discarded [V], so answer `None`.
- **The client's own range check** is approach-then-command at 5,000 m. As
  with gates, `jump.rs` does no server-side range check (bots). If CP54b
  adds one, the refusal is `NotCloseEnoughToEnterWormhole {maxdist}` [V
  key].
- **The high-sec prompt is client-side** and needs no server work [V].
- **A second `wormholeMgr` method exists:** `GetWormholePolarization(itemID)`
  (`spacecomponents/client/components/jumpPolarization.pyc:17`) [V]. Answer
  **`None`**, because `None` → `ClearPolarization()` (`:18-19`) [V]. A tuple
  is `(endTime, duration)`, compared with `GetSimTime()` (`:21-22`). Which
  types carry the `jumpPolarization` space component is **unknown**
  (`spacecomponentsbytype` has no confirmed columns). Route the method
  anyway, so that it never raises.

### 2.2 The exchange: `jump-gates.md` §1 with these changes

| # | dir | message | wormhole-specific |
|---|---|---|---|
| 1 | C→S | `wormholeMgr.WormholeJump(holeID)` (service call) | replaces `beyonce.CmdStargateJump` |
| 2 | S→C | `DoDestinyUpdate` `('OnSpecialFX', (shipID, None, None, holeID, destClass, 'effects.JumpOutWormhole', False, True, True, -1, None, None, 0, (destSystemID,)))` to jumper + origin observers | see below |
| 3 | S→C | `RemoveBalls([shipID])`, `jump::REMOVE_AFTER` later | unchanged |
| 4 | S→C | `SessionChangeNotification`: `solarsystemid`, `solarsystemid2`, plus `constellationid` / `regionid` **whenever they differ** (`jump-gates.md` §1 [V]) | J-space has its own 33 regions and 334 constellations (11xxxxxx / 21xxxxxx) [V-data], so every k↔J jump changes both; `jump.rs`'s "no region change" shortcut must go |
| 5 | S→C | `CallRsp` `None` | discarded [V] |
| 6-13 | | park rebind, `SetState` at the far hole, `AddBalls2` to the far system's pilots | `jump::take_arrival` with the far hole in place of the gate |

The step-2 tuple is pinned as follows:

- **The tuple is the 14-argument `OnSpecialFX` that `jump.rs` already sends**
  (`the_fx_has_fourteen_arguments_and_the_destination_last`). The client's
  handler signature is `OnSpecialFX(self, shipID, moduleID, moduleTypeID,
  targetID, otherTypeID, guid, isOffensive, start, active, duration, repeat,
  startTime, timeFromStart, graphicInfo)` (`spaceMgr.pyc:605`, argcount 15
  with `self`) [V].
- **`targetID` must be the wormhole ball.** The effect reads
  `ballIDs[1]` as `wormholeID`, takes its ball's `targetNebulaPath`, plays
  `'Activate'` on its model, and `GetItem(wormholeID)` feeds the
  transition's `nebulaType` (`Jump.pyc:758-773`, `:444`) [V].
- **`otherTypeID` is the destination wormhole class**, looked up in a dict
  of class labels, 0..9, 12, 13 and 25, defaulting to "Space"
  (`spaceMgr.pyc:626-648`). It drives the "Jumping through wormhole into
  <class>" HUD line. It is [V] that the slot is a class; that it is the
  *destination* class is [I], falsified by the HUD naming the origin class.
- **`graphicInfo[0]` must be the destination system id.**
  `JumpTransitionWormhole._PlayMidCurves:475` calls
  `IsTriglavianSystem(self.graphicInfo[0])` whenever the model has a
  `destination_tspace` curve set [V]. `None` would raise a `TypeError` in
  the tunnel, so send `(destSystemID,)`, as the gate does.
- **Optional arrival FX: `effects.WormholeActivity`.** Its `ballIDs[0]` is
  the hole, and it plays `'Activate'` (`Jump.pyc:923-936`) [V]. Send it to
  the far system's pilots, with `shipID = farHoleID` and the other slots as
  in the departure FX [I]. **Falsified by** nothing visible if it is
  omitted. It is cosmetic.

### 2.3 Collapse during the tunnel [V]

`OnWormholeJumpCancel()` is a **no-argument** notify event: argcount 1, only
`self` (`spaceMgr.pyc:122`, listed in `__notifyevents__` at `:69`) [V]. It:

1. clears the HUD indication;
2. aborts the in-flight transition, by replaying `effects.JumpOutWormhole`
   on `session.shipid` with `start=0` (`:128-133`);
3. shows the dialog `CustomInfo {info: GetByLabel('UI/Wormholes/EnterCollapsed')}`.

Use it when a hole collapses between step 2 and step 4. Then restore the
ship's ball in the origin park, put it back in the roster, and **send no
session change**. [A]: v1 can avoid the case entirely, because the jump that
empties the mass still passes (§4.3), so only a lifetime expiry inside the
4 s window triggers it.

### 2.4 Refusals: `UserError` keys that exist in `dialogs.static` [V]

| key | args | when |
|---|---|---|
| `WormholeTooSmall` | none | ship mass > `wormholeMaxJumpMass` |
| `WormholeHasCollapsed` | none | the hole is gone (a stale click) |
| `NotCloseEnoughToEnterWormhole` | `maxdist` | only if a server range check is added |
| `CantEnterWormholeWhileCloaked` | none | cloaked (only once cloaks exist) |
| `CantEnterWormholeWhileModuleActive` | `moduleType` | not in v1 |
| `CantEnterWormholeGlobalCriminalFlag` | `time` | crimewatch's criminal timer, if CP3b's rule wants it [A] |
| `WormholeTooFrequentTravel` | `time` | polarization, not in v1 |
| `WormholeTargetNotLoaded`, `WormholeLoading` | none | "try again in a moment"; the jump is **already being armed**: answer with this, not a second arm |
| `ShipEnteringWormhole`, `ShipEnteringWormhole2` | none | a warp or command arrives while the jumper is in the tunnel |

The body texts are in the dialogs (e.g. `WormholeTooSmall`: "The wormhole is
too small to carry a ship of your size.") [V]. Raise these through
`movement_guard::Refusal { user_error, args }`, as `jump.rs` does.

---

## 3. The wormhole ball and its slim item

`foo.SlimItem.__getattr__` returns `None` for any missing name that does not
start with `__` (`eve/common/script/util/slimItem.pyc:44-45`) [V]. A missing
field therefore never raises an `AttributeError`, but it reaches the
consumers below as `None`. That **crashes the model setup** for
`wormholeSize`. Send every field.

| field | type | consumers [V] | value |
|---|---|---|---|
| `itemID`, `typeID` | int | everywhere | the hole's ball id; its type (a §1.3 code, or 30831 for a K162) |
| `ownerID` | int | brackets | `1` [A], as for celestials (`jump-gates.md` §7.6) |
| `wormholeSize` | **float in {1.0, 0.7, 0.4}** | `clientControllerRegistry.WormholeSetup:164-168`: `{1.0:1.0, 0.7:0.7, 0.4:0.4}[slimItem.wormholeSize]` [V bytecode, `dis.py`: the dict display loads value, then key, so the keys are 1.0, 0.7, 0.4; wh-fix corrected the 1.0/2.0/3.0 first read], a bare subscript with no `try` anywhere up the chain (`ChangeControllerStateFromSlimItem:586-602`, `SpaceObject.SetControllerVariablesFromSlimItem:213-217`) | `1.0` while `mass_left / max > 0.5`, `0.7` while `> 0.1`, else `0.4` [A thresholds] |
| `wormholeAge` | int 0..4 | info window `infoContentBase:2102-2111` (≥4 Lingering, ≥3 AboutToClose, ≥2 ReachingTheEnd, ≥1 StartedDecaying, ≥0 New); controller `WH_Age = min(age, 2)` (`:154-160`) | remaining > 24 h → 0; > 4 h → 1; > 1 h → 2; else 3. 4 is never sent in v1 [A] |
| `maxShipJumpMass` | int 1..4 | info window `:2120-2127` against `const.WH_SLIM_MAX_SHIP_MASS_{SMALL,MEDIUM,LARGE,VERYLARGE}` = 1..4 (`appConst.pyc:1902-1905`); controller `WH_ShipMass` | from attr 1385: ≤ 5e6 → 1; ≤ 6.2e7 → 2; ≤ 4.1e8 → 3; else 4. The constants are [V]; the cut points are [I] (the data has exactly 5e6, 6.2e7, 3.75e8 / 4.1e8, 1e9 / 2e9) |
| `nebulaType` | int, a **graphicID** | `Wormhole.Assemble:21` → `spaceMgr.GetNebulaTextureForType` → `fsdGraphicIDs.GetGraphicFile(nebulaType)` (`spaceMgr.pyc:1073-1075`); `JumpTransitionWormhole.Prepare:444` | the **destination region's `nebulaID`** from `regions.static` (e.g. 10000001 → 11799) [I]. The client's own scene for a system is `cfg.GetNebula(…, regionID)` → `mapRegionCache[regionID].nebulaID` (`eveCfg.pyc:708-715`, `sceneManager.GetResPathForLocation:641-646`) [V]. `regions.tsv` now carries it as `nebula_id` (lane ci-fix-d5, `content.py:universe`; [V] present on all 114 rows; needs a bundle regeneration) |
| `otherSolarSystemClass` | int | info window class text, `{0..9, 12..18, 25}` (`infoContentBase:2080-2101`) | the far system's class (for a K162, the static's own system class) [I] |
| `isDestTriglavian` | bool | controller `SetControllerVariable('isDestTriglavian', …)` (`:171`) | `False` in v1 (class 25 is out) [I]: `None` into a controller variable is untested |

**There is no `wormholeSize` conflict** (corrected by wh-fix, #154). The controller's dict keys are 1.0, 0.7 and 0.4, so the same value feeds the info window (`infoContentBase:2114-2119`: `< 0.5` is "critically disrupted", `< 1` is "reduced"): bucket 0.7 reads reduced, 0.4 reads critical. The earlier text here (keys 1.0/2.0/3.0, a conflict, send 1.0/2.0/3.0) misread the dict; sending 2.0 or 3.0 raised a `KeyError` in model setup.

**Updates** go out as `destiny::SlimItemChange` to the system. The client
handles them in `Wormhole.OnSlimItemUpdated:55-59`, which reruns the
controller setup [V]. Send one when a size or age bucket changes, not on
every jump.

**Removal** (collapse or expiry): `Wormhole.Explode` returns **12,000 ms**
and sets controller variables (`wormhole.pyc:41-52`) [V]. Send it like a
ship death: `('TerminalPlayDestructionEffect', (holeID, 3))` plus
`('RemoveBalls', ([holeID],))` in one stamp (`damage-death.md` §1) [I]. A
plain `RemoveBalls` is the fallback.

---

## 4. Mass, lifetime and K162

### 4.1 State per hole pair (CP54a's `wh` delta)

`delta_kinds.rs` already reserves `("wh", "wormhole")` with the shape
`system_id key k -> dest_system expires_at mass_left`. That covers the pair:

- `system_id` and `k` are the static's home system and slot;
- `dest_system` is the far end;
- `expires_at` is the spawn time plus `wormholeMaxStableTime` minutes [I
  unit];
- `mass_left` starts at `wormholeMaxStableMass`.

**The K162 has no field. It is derived:** it exists iff
`mass_left < wormholeMaxStableMass` [A]. Every jump debits mass (§4.3), so
"after the first jump" follows from the shape alone. This needs no new
delta column. For the frigate holes (regeneration ≥ max, out of v1),
`mass_left` must be debited and then refilled at rollover, so that the rule
still holds.

### 4.2 K162 behaviour [A] throughout, except where marked

- **Before the first jump:** only the static exists, as a ball plus a
  signature in its home system. The far system has nothing. This is the
  plan's CP54a test.
- **The first jump in either direction** creates the K162 (type 30831
  [V]):
  - its ball, at an R2 anchor (C3 §4.3) in `dest_system`;
  - its archetype-38 signature, with dungeon 3663, the lowest of the 11
    K162 dungeons [V-data] [A choice];
  - a `SlimItemChange`-free `AddBalls2` to the far system's pilots.
- **A K162 jump** goes to the static's home system and debits the same
  `mass_left`. A K162 has no attributes of its own [V], so it uses the
  static's 1383, 1385 and 1382 [I]. Its slim item carries the static's
  `maxShipJumpMass` bucket and the *home* system's class as
  `otherSolarSystemClass`.
- **Arrival position** (both directions): the far hole's position plus a
  seeded offset of 2,000-5,000 m [A], outside the 3,000 m radius. This
  replaces `jump::arrival_position(gate, i)`.
- **Collapse** (mass or time) removes **both** balls and both signatures in
  one rollover step. The static's slot then respawns with `generation + 1`
  and a fresh destination draw (§6.2), immediately [A].
  **Falsified by** humans finding a system with no exit for long stretches.

### 4.3 The jump rule (CP54b)

1. **Refuse when the ship is too heavy:** if the hull's `mass` (attr 4
   from `types.fsdbinary`, [V], e.g. Rifter 1,067,000) is greater than
   `wormholeMaxJumpMass`, raise `WormholeTooSmall`. Fitted modules (a
   propulsion module's mass add) are ignored in v1 [A].
2. Run `movement_guard::JUMP_GUARDS` (scram etc., CP57) for the jumper.
3. Arm the jump exactly as `jump::cmd_stargate_jump` does, with the §2.2
   payloads and `dest = the far hole`.
4. **Debit `mass_left -= hull mass` at arm time.** If the result is ≤ 0, the
   jump still passes and the pair collapses once the session change has gone
   out [A] (Tranquility folklore: "the last ship through gets through").
   **Falsified by** a human who expects to be stranded.
5. Pairs whose `expires_at` has passed collapse in `wormhole_jump::tick`.

**Plan seam note [I].** `movement_guard::Guard` is
`fn(&Registry, char) -> Option<Refusal>`. It has no way to see *which* hole
is being entered, so `wormhole_jump::too_heavy` cannot be a plain
`JUMP_GUARDS` row as CP54b's row says. Two options:

- (a) check the mass inline in `WormholeJump`, before the guards. This is
  recommended, and it keeps S12 unchanged;
- (b) give the guard the target through a world field set just before
  `check`.

Option (a) also keeps a scram refusal from depending on hole state.

**Routing.** `wormholeMgr` is a new service name. CP54b needs a `DOMAINS`
(or service registry) row for `wormholeMgr` with `WormholeJump` and
`GetWormholePolarization`, which its §6.2 row does not list. **One more
coordinator hunk.**

---

## 5. Signatures

**CCP models a wormhole as a dungeon** [V-data]. There are 170
archetype-38 dungeons, all named "Unstable Wormhole" (2 are "Test
Wormhole"). All of them have `entryTypeID = 19728` (Deadspace Signature).
Their `difficulty` is absent on 164, 0 on one and 1 on five. **120 of them
hold exactly two objects: one group-988 type plus one `Deadspace
Signature`** (19728). The other 50 are Pochven and Drifter pockets with
spawners and scenery. The two-object ones map one-to-one onto the §1.3 types
(column `dun`). K162 has 11 of them (3663-3671, 9888, 13998).

So a wormhole's signature is a C3 site record (shape S1, C3 §4.4) with:

| S1 field | value |
|---|---|
| `archetype` | **38** [V] |
| `dungeon_id` | the type's `dun` from §1.3 (the lowest id when there are several) [V-data] [A choice] |
| `entry_type` | 19728 [V] |
| `scan_attr` | **1908** `scanWormholeStrength` [V] |
| `difficulty` | 1 when the dungeon has none [A]. The C2 shape wants an int ≥ 1 |
| `faction` | `None` |
| `anchor` | the hole ball's position (the room-0 origin; the wormhole object sits at it) [I] |
| `target_id` | the population scheme's `ABC-123` (C3 R3) |

On the wire it is C2 §2.2's `SignatureInfo`: `position` plus deviation,
`targetID`, `difficulty`, `dungeonID=None`, `archetypeID=None` and
`deviation` (metres), pushed as
`OnSignalTrackerSignatureUpdate(ssid, added, removed)` by CP17/CP29 [V
shapes, C2].

- **Probe results** disclose `strengthAttributeID = 1908`, so the scanner
  labels the result "Wormhole" (`explorationSites.pyc:28`) [V].
- At a perfect result, `typeID` is the hole's type and `groupID` is 988.
- The signal strength uses `scan.rs`'s formula, which is [A] for every
  site. Wormhole types carry no `signatureRadius` (552) and no 1908 value
  [V: their six attributes are all they have], so these are inputs we pick:
  `signatureRadius` as for a relic site at difficulty 1 [A].

**Plan conflict [I].** CP54a's test says the signature "appears through
population as archetype 38". But C3 §4.2's draw picks a *random* candidate
dungeon, and here the dungeon is *fixed by the static's type*.
Recommendation:

- `wormhole.rs` owns its slots (its own `SLOT`) and hands finished S1
  records to the signal tracker through the same interface population
  uses;
- `population.tsv` gets **no** archetype-38 row, which C3 §4.1 already says
  for wormhole bands;
- if the plan insists on a population row, it must carry a `fixed`
  dungeon from `wormhole.rs`, and that is a new rule in `population.rs`.

---

## 6. J-space in the static data, and the statics table

### 6.1 What exists [V-data]

From `out/content/universe/{systems,regions,celestials}.tsv` (CP3b's
extract of `systems.static` and `solarsystemcontent.static`):

| ids | `wormhole_class_id` → count |
|---|---|
| 31000000-31999999 (2,604 systems) | C1 358, C2 537, C3 506, C4 523, C5 531, C6 118, 12 (Thera, 31000005) 1, 13 (shattered) 25, 14-18 (Drifter) 1 each |
| 30000000-30999999 (5,485 systems) | 7: 1,193; 8: 687; 9: 3,188; 25 (Pochven): 27; 10: 6; 11: 7; **empty: 377** |
| 32xxxxxx, 34xxxxxx | 19-23 (abyssal/void), out of scope |

- **J-space systems carry their class on their own row** [V]. The
  377 k-space systems with an empty class get `INVALID = 0` from the
  client's own `GetLocationWormholeClass` (`eveCfg.pyc:704-706`: `getattr(system, 'wormholeClassID',
  INVALID)`) [V]. For a *destination draw* (§6.2), fall back to the
  system's region `wormhole_class_id`, and then to the CP3b security band
  (≥ 0.45 → 7, > 0.0 → 8, else 9) [A].
- **J-space regions** (`11000001`-`11000033`) carry a class too. A-R
  regions are C1, B-R are C2, and so on; G-R00031 is Thera (12) and
  H-R00032 is class 13 [V-data].
- **J-space has no gates and no belts.** Celestials for the 2,604 systems
  are 2,604 stars, 21,789 planets, 107,519 moons, 4 stations (Thera), and
  **0** `gate` and **0** `belt` rows [V-data]. A wormhole is the only way
  in or out, and C3's belt rats have nowhere to spawn.
- **Security** is −0.99 on all 2,604 J-space rows [V-data]. CP3b's
  `flags_apply` is therefore false and no CONCORD applies. `chat.rs`
  already names a `wormhole` channel scope.
- **The static assignment is absent from the client** [V]:
  - `typedogma` has the types but no per-system slot;
  - no static table carries "system → static" (the `resfileindex` lists no
    such table);
  - `wormholeTargetSystem1..9` are unused;
  - EvEJS's `explorationWormholeStatic` is an authored spec
    (`evejs-data.md` §4.3) and must not be ported.
- **What the client does give is the grouping [I].** The two-object
  dungeons come in **contiguous blocks of nine**, and each block holds
  exactly one type per destination class 1..9:
  - 3639-3647 for C1;
  - 3630-3638 for C2;
  - 3621-3629 for C3;
  - 3612-3620 for C4;
  - 3603-3611 for C5;
  - 3594-3602 for C6;
  - 3585-3593 for HS;
  - 3567-3575 and 3576-3584 for LS and NS.
  
  The source class of a block follows from its jump-mass profile. The C1
  block is all 6.2e7, since a C1 admits nothing bigger. C5/C6 reach
  2e9-class holes. **Falsified by** a block whose mass profile contradicts
  its source class. None does. This matches the publicly known Tranquility
  sets, e.g. C1 = H121, C125, O883, M609, L614, S804, N110, J244, Z060.

### 6.2 The rule (G-WH) [A] throughout

For each system `s` with class `c`, at each rollover:

1. `n = STATICS_PER_SYSTEM[c]`:
   - C1, C2, C3, C5, C6: **1**;
   - C4: **2**;
   - 12, 13 and 14-18: **0**;
   - HS, LS and NS: **0.02** (a per-system probability, seeded per system
     and day, the C3 §4.1 fraction convention).
   
   This is a Rust const, not a table column: the plan fixes `statics.tsv`'s
   columns. **Falsified by** humans saying "J-space is unreachable" (raise
   k-space) or "a maze" (lower it).
2. **Slot `k` < n picks its row**:
   `rows_of(c)[splitmix64(hash(s, k, generation)) % len]`, without
   replacement across one system's slots (C4's two statics differ). `rows_of(c)` is
   `statics.tsv` filtered on `class == c`.
3. **The destination system:** `systems_of(row.dest_class)` sorted by id,
   indexed by `splitmix64(hash(s, k, generation, 1)) % len`, excluding `s`
   itself. `systems_of` uses §6.1's effective class and excludes Pochven
   (25), 10, 11, 12-18 and every system without a `celestials.tsv` star.
4. **The hole ball** is placed by C3 R2's anchor (seeded), and its
   archetype-38 signature is registered (§5).
5. **Generation** increments on collapse or expiry (§4.2), so a restart
   reproduces every live pair, and the `wh` delta carries what has changed
   since then.

**Sizing check.** About 2,580 J-space systems give about 3,100 live pairs,
or 6,200 records at most once every K162 is open, plus about 100 k-space
wanderers a day. That is records only, and the balls are lazy, as in C3
§4.5. Cheap.

### 6.3 Proposed `content/wormholes/statics.tsv` [A]

The columns are the plan's (§4.4 of `plan.md`): `class static_type_id
dest_class why`. Each row's `why` names its evidence. The **membership**
(which types a class may draw) is [I] from §6.1. **Drawing uniformly** among
them is [A].

```
#class	static_type_id	dest_class	why
1	30660	1	H121: C1 block (dungeon 3639), all-6.2e7 jump mass [I]; uniform draw [A]
1	30661	2	C125: C1 block 3640 [I]
1	30662	3	O883: C1 block 3641 [I]
1	30663	4	M609: C1 block 3642 [I]
1	30664	5	L614: C1 block 3643 [I]
1	30665	6	S804: C1 block 3644 [I]
1	30666	7	N110: C1 block 3645 [I]
1	30667	8	J244: C1 block 3646 [I]
1	30668	9	Z060: C1 block 3647 [I]
2	30671	1	Z647: C2 block 3630 [I]
2	30672	2	D382: C2 block 3631 [I]
2	30673	3	O477: C2 block 3632 [I]
2	30674	4	Y683: C2 block 3633 [I]
2	30675	5	N062: C2 block 3634 [I]
2	30676	6	R474: C2 block 3635 [I]
2	30677	7	B274: C2 block 3636 [I]
2	30678	8	A239: C2 block 3637 [I]
2	30679	9	E545: C2 block 3638 [I]
3	30680	1	V301: C3 block 3621 [I]
3	30681	2	I182: C3 block 3622 [I]
3	30682	3	N968: C3 block 3623 [I]
3	30683	4	T405: C3 block 3624 [I]
3	30684	5	N770: C3 block 3625 [I]
3	30685	6	A982: C3 block 3626 [I]
3	30686	7	S047: C3 block 3627 [I]
3	30687	8	U210: C3 block 3628 [I]
3	30688	9	K346: C3 block 3629 [I]
4	30689	1	P060: C4 block 3612 [I]
4	30690	2	N766: C4 block 3613 [I]
4	30691	3	C247: C4 block 3614 [I]
4	30692	4	X877: C4 block 3615 [I]
4	30693	5	H900: C4 block 3616 [I]
4	30694	6	U574: C4 block 3617 [I]
4	30695	7	D845: C4 block 3618 [I]
4	30696	8	N290: C4 block 3619 [I]
4	30697	9	K329: C4 block 3620 [I]
5	30698	1	Y790: C5 block 3603 [I]
5	30699	2	D364: C5 block 3604 [I]
5	30700	3	M267: C5 block 3605 [I]
5	30701	4	E175: C5 block 3606 [I]
5	30702	5	H296: C5 block 3607 [I]
5	30703	6	V753: C5 block 3608 [I]
5	30704	7	D792: C5 block 3609 [I]
5	30705	8	C140: C5 block 3610 [I]
5	30706	9	Z142: C5 block 3611 [I]
6	30707	1	Q317: C6 block 3594 [I]
6	30708	2	G024: C6 block 3595 [I]
6	30709	3	L477: C6 block 3596 [I]
6	30710	4	Z457: C6 block 3597 [I]
6	30711	5	V911: C6 block 3598 [I]
6	30712	6	W237: C6 block 3599 [I]
6	30713	7	B520: C6 block 3600 [I]
6	30714	8	C391: C6 block 3601 [I]
6	30715	9	C248: C6 block 3602 [I]
7	30579	1	Z971: HS block 3585 [I]; k-space wanderer at STATICS_PER_SYSTEM 0.02 [A]
7	30583	2	R943: HS block 3586 [I]
7	30584	3	X702: HS block 3587 [I]
7	30642	4	O128: HS block 3588 [I]
7	30644	5	M555: HS block 3589 [I]
7	30645	6	B041: HS block 3590 [I]
7	30657	7	A641: HS block 3591 [I]
7	30658	8	R051: HS block 3592 [I]
7	30659	9	V283: HS block 3593 [I]
8	30579	1	Z971: LS/NS block 3567/3576 [I]
8	30583	2	R943: LS/NS block 3568/3577 [I]
8	30584	3	X702: LS/NS block 3569/3578 [I]
8	30642	4	O128: LS/NS block 3570/3579 [I]
8	30643	5	N432: LS/NS block 3571/3580 [I]
8	30646	6	U319: LS/NS block 3572/3581 [I]
8	30647	7	B449: LS/NS block 3573/3582 [I]
8	30648	8	N944: LS/NS block 3574/3583 [I]
8	30649	9	S199: LS/NS block 3575/3584 [I]
9	30579	1	Z971: LS/NS block [I]
9	30583	2	R943: LS/NS block [I]
9	30584	3	X702: LS/NS block [I]
9	30642	4	O128: LS/NS block [I]
9	30643	5	N432: LS/NS block [I]
9	30646	6	U319: LS/NS block [I]
9	30647	7	B449: LS/NS block [I]
9	30648	8	N944: LS/NS block [I]
9	30649	9	S199: LS/NS block [I]
```

- **Which of 3567-3575 and 3576-3584 is LS and which is NS is unknown.** The
  two blocks hold identical types, so it does not matter.
- **The loader's checks** (CP54a test material):
  - every `static_type_id` is group 988 in `typedogma`;
  - its attr 1381 equals `dest_class`;
  - its `class` has a two-object archetype-38 dungeon in the block named in
    `why`.
  
  A row failing any of these is dropped and logged once, never a panic.
  These checks are **[V] rules** over the fixture.

---

## 7. Test list for CP54a and CP54b

Every test runs on fixture rows (`content::fixture()`), never on CCP's
data. No [A] value is pinned: the thresholds, counts and offsets are
asserted as rules ("the bucket is monotone in mass left"), not as numbers.

**CP54a (statics and signatures).**

1. A fixture system of class N with one row of class N spawns one static.
   Its type is that row's and its destination has effective class
   `dest_class` (the plan's test).
2. The draw is deterministic: the same `(system, k, generation)` gives the
   same type and destination across two `Registry` builds.
3. A C4-like fixture class with n = 2 never draws the same row twice.
4. The `wh` delta round-trips. An expired pair is gone after rollover, and
   its slot reappears with `generation + 1` and a destination that may
   differ (the plan's test).
5. The signature is archetype 38 with `scan_attr` 1908 and
   `dungeon_id` = the type's fixed dungeon. It appears in
   `OnSignalTrackerSignatureUpdate` for the home system only.
6. **The K162 exists iff `mass_left < max`.** Before any jump, the far
   system has no ball and no signature. After one, it has both (the plan's
   test).
7. The slim item carries all eight §3 fields. `wormholeSize` is always in
   {1.0, 0.7, 0.4}, `wormholeAge` in 0..3 and `maxShipJumpMass` in 1..4
   [V domains]. Both buckets are monotone in mass left and time left.
8. `nebulaType` equals the destination region's `nebulaID` column (fixture
   region).
9. The `statics.tsv` loader drops a row whose type is not group 988, or
   whose attr 1381 differs from `dest_class`.

**CP54b (jumps, mass and decay).**

1. `wormholeMgr.WormholeJump(holeID)` answers `None`. It then emits, in
   order:
   - `OnSpecialFX` with guid `effects.JumpOutWormhole`, `targetID = holeID`,
     `otherTypeID = dest class`, 14 args, last `(destSystemID,)`;
   - `RemoveBalls([ship])`;
   - a session change carrying `solarsystemid` and `solarsystemid2`, plus
     `constellationid` and `regionid` for a fixture pair in different
     regions, and **not** `shipid`.
   
   [V shapes]
2. Arrival is at the far hole, outside its 3,000 m radius, and the far
   system's pilots receive `AddBalls2`.
3. The jump debits the hull's `mass` [V attribute] from `mass_left`, and the
   pair's two holes report the same `mass_left`.
4. A hull with `mass > wormholeMaxJumpMass` is refused with
   `UserError('WormholeTooSmall', {})` [V key]. Nothing is pushed and no
   mass moves.
5. A jump through a hole id that does not exist is refused with
   `WormholeHasCollapsed` [V key].
6. When `mass_left` reaches ≤ 0 the jump passes and the pair collapses after
   it. Both balls go (`TerminalPlayDestructionEffect` + `RemoveBalls`) and
   both signatures go.
7. A pair past `expires_at` collapses in `tick`. When it collapses during an
   armed jump, `OnWormholeJumpCancel` with **zero** arguments goes to the
   jumper, and no session change follows [V arity].
8. `wormholeMgr.GetWormholePolarization(id)` answers `None` [V-safe].
9. `JUMP_GUARDS` run for a wormhole jump: a fixture guard's refusal is
   raised, and no mass is debited.

**The human check** (one relaunch): warp to a scanned static, jump, and
confirm three things:

- the tunnel plays with the right class text;
- the model looks right at each size bucket, which settles the
  `wormholeSize` conflict;
- `Show Info` names class, age and size without an error in the client log.

---

## 8. unknown

1. ~~`wormholeSize`'s real domain~~ settled: 1.0, 0.7, 0.4 (§3).
2. **The `wormholeMaxStableTime` unit** is labelled "Time" but reads as
   minutes. Nothing client-side confirms it (§1.2).
3. **`wormholeMassRegeneration`'s cycle** and **`wormholeTargetDistribution`'s
   meaning**: no client table is keyed by them.
4. **Which types carry the `jumpPolarization` space component.**
   `spacecomponentsbytype` has no confirmed columns. Answering `None` is
   safe regardless.
5. **Whether `otherTypeID` in the departure FX is the destination or the
   origin class**: [I] destination.
6. **Which dungeon block is LS and which is NS** (identical contents; it
   does not matter).
7. **`isDestTriglavian = None` vs `False`**: send `False`.
