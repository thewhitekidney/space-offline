# NPC behaviours, and what it takes for a bot to be a player

Lane `npc-behaviors`, client build **V24.01 / 3619879**. Read-only: nothing in
the server tree was touched, no cargo, no gate, no commit.

Marks are the house convention. **[V]** read out of this client's own
bytecode or shipped data with a file and a line; **[I]** inferred, with the
inference stated; **[V2]** a third party's decode of a *different* build;
**[A]** an assumption we choose to ship, each one with the observation that
would falsify it. Python line numbers are `tools/dis.py` numbers — the
client's own `co_lnotab` — never uncompyle6's.

---

## 0. Verdict on the hypothesis

> *Our server already drives player ships entirely through destiny actions and
> service pushes, so a bot should need no new client-facing machinery at all —
> just a server-side driver issuing the same commands a player's client would.*

**Confirmed for flight, and confirmed for a stronger reason than the one in the
brief: CCP's own NPCs do exactly this.** The shipped `behaviors` package drives
its entities by calling `ballpark.GotoPoint`, `ballpark.GotoDirection`,
`ballpark.SetSpeedFraction`, `ballpark.StopEx`, `ballpark.WarpTo` and
`ballpark.ChangeBallPosition` — the *same server-side verbs* our
`destiny::Action` already encodes and our `space_state::command` already maps
the client's `Cmd*` calls onto **[V]**, §7.1. A ball record on the wire has no
owner, no character, no "is NPC" bit and no ball-class id (`destiny.rs:176-192`
**[V]**), and the client's sole NPC/player discriminator is a **numeric range
test on the slim item's `ownerID`** — `IsNpcObject` is literally
`not IsPlayerOwner(slimItem.ownerID)`, and `IsPlayerOwner` is
`90000000 <= ownerID <= 2147483647` with no lookup
(`bracketVarious.py:436-438`, `idCheckers.py:318-319`) **[V]**. So a bot given a
player-range character id, a player corp and a player ship type is, to the
client, a player. Nothing new is needed to make it fly, appear, be named, be
listed in the overview, or be removed.

**The exceptions are all about combat, and there are four.** None is
NPC-specific — each is equally missing for two humans shooting each other, so
each is a feature we owe the product anyway, not a tax on bots:

1. **`OnTarget` / `OnTargets`** — the only way the client learns it is being
   locked (`targetMgr.py:655`, `:660-669`) **[V]**. Without it,
   `state.threatTargetsMe` never lights and a bot can lock a player invisibly.
2. **`OnSpecialFX`** (14 positional parameters, `michelle.py:2087`,
   `tactical.py:413`) — the only way the client learns a module fired at it;
   it is what sets `state.threatAttackingMe` (`tactical.py:434`, `:448`) and
   draws the turret and missile effects **[V]**.
3. **`OnDamageStateChange(shipID, damageState)`** (`michelle.py:2070-2077`)
   **[V]** — health bars after the bootstrap. `AddBalls2`'s `extraBallData`
   pair sets them once; this is the only thing that changes them.
4. **`OnSlimItemChange(itemID, newSlim)`** (`michelle.py:2006-2011`) **[V]** —
   a whole-slim replacement, which is how CCP mutates per-ball fields in flight
   (`ballpark.UpdateSlimItemField`, `behaviors/actions/ballparks.py:813`
   **[V]**). Needed only if a bot's standings/name/flags change while it is on
   screen.

**Two negatives worth as much as the positives.** `extraBallData` needs **no
NPC marker key at all** (§3.3). And the bound object `entity`, the only
client-facing surface the `behaviors` package has, is called **nowhere outside
`eve/devtools/` and `nodegraph/`** **[V]** — a GM debug window, not gameplay
(§1.5). An ordinary client never asks the server anything about an NPC.

**On reusing CCP's design: yes, and it is a behaviour tree, not a state
machine.** 178 modules, 368 registered task classes, five composites, ten
decorators, a blackboard with six scopes, and a data format
(`behaviortrees` FSD) that does **not** ship. We get the whole engine and
vocabulary and none of CCP's tuned trees — except the 13 hand-written Python
archetypes that *do* ship, which the shipped `spaceComponentsByType` data keys
into **by name**, verified (§2.5). That match is the strongest single piece of
evidence in this document that `behaviors` is live server code.

---

## 1. What the `behaviors` package actually is

### 1.1 Where it lives and how big it is

`out/pyc/behaviors/`, **178 `.pyc` files, 1.5 MB** **[V]**. Subpackages:
`actions/` (35 modules), `conditions/` (17), `monitors/` (19), `composites/`,
`decorators/`, `trees/` (46 modules across 8 subpackages), `utility/` (22),
`registry/`, `blackboards/`, `const/`, `loaders/`, `timers/`, `groups/`,
`debugging/`, plus `behaviortree.pyc`, `tasks.pyc`, `status.pyc`, `entity.pyc`,
`storage.pyc`, `targetEvaluation.pyc`, `exceptions.pyc`.

Every module's embedded build path is
`C:\BuildAgent\work\...\eve\release\V24.01\packages\behaviors\<file>.py`
**[V]** — the shared `packages/` root, not `eve\client\script\`.

### 1.2 It is server code. Three independent proofs.

**(a) It imports packages that do not ship.** The package's non-`behaviors`
imports include `ballpark.*` (23 distinct modules:
`ballpark.entities.entitywrapper`, `ballpark.messenger.const`,
`ballpark.helpers.aggression`, `ballpark.locationgenerator`,
`ballpark.warpDisruptionBalls`, `ballpark.fleets.fleet_types`, …),
`npcs.server.npcspawning.*`, `npcs.server.tags`, `npcs.server.rewards.*`,
`npcs.server.standings`, `eve.server.script.mgt.fighters.*`,
`spacecomponents.server.messages`, `lootutils.grouploot` and
`spawnlists.data` **[V]**. Checked against `out/pyc/`:

| package | ships? |
|---|---|
| `ballpark` | **no** (only `ballparkCommon`) |
| `npcs/server` | **no** (`npcs/client`, `npcs/common` do) |
| `spacecomponents/server` | **no** (`client`, `common` do) |
| `eve/server` | **no** |
| `lootutils`, `spawnlists` | **no** |

`find out/pyc -type d -name server` returns **zero directories** **[V]**: CCP's
packaging strips `*/server/*`. So `behaviors/actions/combat.py` cannot be
imported in the client at all — the first `from ballpark.entities… import` is
an `ImportError`. **Dead code in the client by construction.**

**(b) The strip is by directory name, and `behaviors` has no `server`
subdirectory.** Every other mixed package in the tree uses the
`client`/`common`/`server` triple and lost its `server` leg. `behaviors` is
flat, so the whole package slipped through the same filter. That is the
mechanism, and it is **[I]** — but it is the only hypothesis consistent with
zero shipped `server/` directories and one shipped package full of
`*.server.*` imports.

**(c) It calls the server's own object model.** `BehaviorEntity`
(`behaviors/entity.py:41`) subclasses `ballpark.entities.entitywrapper.
EntityWrapper` and sets `__destiny_ball_class_id__ = 58` **[V]** — a *server*
ball class selector, which is confirmed by the shipped
`spaceComponentsByType.destinyServerBallClass = {classID: i32}` component
(`findings/pyd-schemas.tsv:1532`) **[V]**. Nothing on the wire carries a ball
class (§3.1).

### 1.3 Two leaf modules are genuinely shared, and that is why it ships

Exactly two `behaviors` modules are referenced from code that *is* client-side
**[V]**:

- **`behaviors.const.combat`** — imported by
  `spacecomponents/client/components/entitystandings.py` (for
  `FW_ATTACK_NOT_SAME_FACTION`, `FW_ATTACK_ALL_ENEMIES`,
  `FW_ATTACK_INSURGENCY_ENEMIES`) and by
  `eve/client/.../bracketVarious.py:7`. It is a pure constants module with no
  imports at all.
- **`behaviors.blackboards.scopes.Scope`** — on the **marshal whitelist**
  (`whitelist.pyc`, `carbon/common/lib/whitelist.pyc`) **[V]**, i.e. CCP
  allows that namedtuple to cross the wire. `scopes.py` is written to be
  importable on either side: its `idCheckers` import is wrapped in
  `try/except ImportError` with a stub fallback **[V]**.

So the honest classification is: **a server-side package with two
deliberately-shared leaves, shipped whole because the packaging filter keys on
a directory name the package does not have.** Treat `const/` and
`blackboards/scopes.py` as shared; treat everything else as a leaked server
implementation we may read as a design document.

### 1.4 The data does not ship

`behaviors/storage.py:34` **[V]**:

```python
def CreateBehaviortreeStorage(cache='behaviortrees.static'):
    return fsdlite.EveStorage(data='behaviortrees', cache=cache, mapping=(), indexes=[], coerce=int)
```

There is **no `behaviortrees` resource** among the client's 203
`res:/staticdata/*` entries **[V]** (`tools/analysis.sh python3
tools/fsd_dump.py list --client /client`; the only `behavior` hit is
`character_avatarbehaviors.fsdbinary`, which is paperdoll idle animations).
Neither do `spawnlists`, any spawn table, or any loot table. **We get the
engine and the vocabulary; CCP's authored trees stay on their server.**

### 1.5 The one client-facing surface, and it is a GM tool

`eve/devtools/script/behaviortools/clientdebugadaptors.py` **[V]**:

```
Connect():     moniker.GetEntityLocation().EnableBehaviorDebugging(itemID) -> treeData   (:76-83)
Disconnect():  moniker.GetEntityLocation().DisableBehaviorDebugging(itemID)              (:84-96)
push:          OnBehaviorDebugUpdate(entryKey, events, taskStatuses, tasksSeen, blackboards)  (:32, :98)
```

`moniker.GetEntityLocation()` is `Moniker('entity', session.solarsystemid2)`
with a session check on `solarsystemid2` (`eveMoniker.py:100-103`) **[V]**.
`GetEntityLocation` is referenced by **four** files in the whole client:
`eveMoniker` itself, `clientdebugadaptors`, `nodegraph/client/getters/
blackboard.pyc`, and `behaviors/actions/entities.pyc` (server side) **[V]**.

**Consequence for us:** the `entity` bound object is optional. If we ever want
the client's behaviour-tree debugger as a GM tool it is three methods and one
push, and it is the only reason a normal client would ever name an NPC's
behaviour.

---

## 2. The design it encodes

### 2.1 It is a behaviour tree with a task queue, not a state machine

`BehaviorTree` (`behaviortree.py:28`) **[V]**:

- holds `behaviorId`, `scopeType`, a `deque` of pending tasks, one `_rootTask`,
  `_isResetRequested`, `_isResetBlocked`, and a `signals.Messenger`;
- `Tick(context)` (`:66-78`): `ProcessResetRequest()`, push a `None` end
  marker, then `while self.Step(context)` until the marker pops;
- `Step(context)` (`:80-93`): `popleft()`; if it is the marker, stop; else
  `task.Tick(self, context)`;
- `StartTask` is `appendleft` (`:54`) — **this tick**;
  `StartTaskNextTick` is `append` (`:57`) — **after the end marker**. That one
  pair is the whole scheduler.
- `RequestReset(requestedBy)` (`:105`) sets a flag; `Reset()` (`:119`) clears
  the queue, `CleanUp()`s the root and restarts it. A `BlockReset` from a
  blocking task defers it (`:95-103`).
- `Release()` (`:38`) cleans up the root and drops the messenger.
- Five message names on the internal messenger: `OnRequestReset`, `OnStep`,
  `OnReset`, `OnTickDone`, `OnBlockReset` (`:13-18`) **[V]**.

**The design idea worth stealing:** monitors do not poll. A `MonitorTask`
subscribes to an event, and when the event fires it calls
`behaviorTree.RequestReset()`, which throws the whole tree away and
re-evaluates from the root on the next tick (`tasks.py:198-227`) **[V]**. So
"react to being shot" costs one subscription and one flag, not a condition
evaluated every tick. A Rust implementation wants the same shape: an event
channel per entity, and a `dirty` flag that re-runs the tree from the root.

### 2.2 Six statuses, and each one is a policy object

`status.py` **[V]** — a class per status whose `OnUpdated(task)` *is* the
scheduling decision:

| status | `OnUpdated` does |
|---|---|
| `TaskInvalidStatus` | raises `InvalidStatusError` |
| `TaskSuccessStatus` | `task.Exit()` |
| `TaskFailureStatus` | same as success (it subclasses it) |
| `TaskRunningStatus` | `behaviorTree.StartTaskNextTick(task)` |
| `TaskSuspendedStatus` | nothing — the task waits for an event |
| `TaskBlockingStatus` | `StartTaskNextTick(task)`, and blocks resets |

`Task.Tick` (`tasks.py:42-48`): if status is `Invalid`, bind
`(behaviorTree, context)` as **weak proxies** and call `OnEnter()`; then
`Update()`; then `self.status.OnUpdated(self)`. Exit notifies the parent
composite through an exit observer held as a *weak callable proxy*
(`SetExitObserver`, `:54`) **[V]**. A Rust port replaces the weakrefs with
indices into an arena; the status table ports literally.

### 2.3 Five composites, ten decorators

`composites/__init__.py` **[V]**:

- `Composite.OnEnter` (`:34`): raise if no subtasks, set **Suspended**, start
  `GetInitialTask()`. The parent is suspended while a child runs; the child's
  exit observer wakes it.
- **`Sequence`** (`:60`): child failed → fail; last child succeeded → succeed;
  else start the next child.
- **`PrioritySelector`** (`:75`): child succeeded → succeed; last child failed
  → fail; else next child. (This is a *fallback* selector.)
- **`WeightedSelector`** (`weighted.py:13`): `GetInitialTask` picks one child
  by `evemath.weighted_choice` over `attributes.subTaskWeights`, and raises if
  the weight count and child count disagree. Its own status *is* the chosen
  child's.
- **`Decorator`** (`decorators/__init__.py`): a composite restricted to one
  child, whose status it adopts.

Decorators, all **[V]**:

| decorator | behaviour |
|---|---|
| `Not` | swaps success and failure |
| `ForceSuccess` / `ForceFailure` | pins the status |
| `Uninterruptible` | enters Blocking and blocks resets until it exits |
| `Invulnerable` | `ballpark.MakeInvulnerablePermanently(myBall.id, reason)` on enter, cancel on exit/cleanup |
| `Cloaked` | `ballpark.CloakShip(myItemId)` / `UncloakShip` around the child |
| `OneShot` | succeeds immediately if a blackboard flag is set; else sets it and runs |
| `RepeatWhileFailing` | retries the child up to `maxRepetitions`, cleaning it up between tries |
| `CooldownTimer` | fails while a blackboard timer channel holds a value; starts the timer on the first pass |
| `RandomCooldownTimer` | as above with `random.randint(minTimeoutSeconds, maxTimeoutSeconds)` |

### 2.4 The blackboard: six scopes, addresses, channels

`blackboards/scopes.py` **[V]**. `Scope = namedtuple('Scope', ['scopeType','id'])`
and `ScopeTypes = {Item:'item', EntityGroup:'entity_group', Dungeon:'dungeon',
Tale:'tale', SpawnPointPool:'spawn_point_pool', NodeGraph:'node_graph'}`.
A scope's id comes from the behaviour context by a fixed mapping
`CONTEXT_ID_BY_SCOPE = {item:'myItemId', entity_group:'myEntityGroupId',
dungeon:'myDungeonSpawnId', tale:'myTaleId', spawn_point_pool:
'mySpawnPointPoolId', node_graph:'myNodeGraphInstanceId'}`.

An **address** is a `(scopeType, channelName)` pair; resolving it yields a
*message channel* on the blackboard for that scope, and a channel supports
`SendMessage(value)`, `GetLastMessageValue()`,
`GetLastMessageValueWithMaxAge(maxAge)`, `AddObserver`, `RemoveObserver`
(`tasks.py:79-105`) **[V]**. Channels are cached per task
(`context.task_channel_cache`).

`const/blackboardchannels.py` is the authored channel vocabulary — 40 names
**[V]**, e.g. `(EntityGroup,'COMBAT_TARGETS_SET')`,
`(EntityGroup,'GROUP_PRIMARY_TARGET')`, `(EntityGroup,'COMMANDER')`,
`(Item,'MY_COMBAT_TARGET')`, `(Item,'COMBAT_MIN_OPTIMAL_RANGE')`,
`(Item,'WARP_SCRAMBLED')`, `(Item,'UNSPAWN_ENTITY')`,
`(EntityGroup,'GUARD_OBJECT_ITEM_ID' / '_POSITION' / '_ORBIT_RANGE' /
'_AGGRESSION_RANGE')`, `(EntityGroup,'REINFORCEMENT_REQUESTED')`,
`(EntityGroup,'STANDINGS_HOSTILE_THRESHOLD')`.

**The reason this matters for a Rust port:** the blackboard is how one entity's
tree coordinates with its *group's* tree. "Who is the fleet's primary target"
is one `EntityGroup`-scoped channel, written by the group tree and read by
every member tree. That is the whole of EVE's NPC fleet coordination.

### 2.5 Two loaders, and the shipped data keys into the shipped Python

`loaders/fsd_behavior_loader.py:95` **[V]** builds a tree from FSD:

```
behavior_tree_data = BehaviortreeStorage().get(behavior_id)   # keyed by int
root  = behavior_tree_data['root']
node  = {'taskClass': <dotted class name>, 'uuid': <str>, 'attributes': {...}, 'subTasks': [...]}
```

`_instantiate_tree` looks `taskClass` up in the registry, builds a namedtuple
of that class's declared attributes (always including `name`), converts each
FSD value with the attribute's own `convert_fsd_to_attribute`, and recurses
into `subTasks`. A `taskClass` ending in `ReferenceSubtree` instead loads
another tree by `behaviorTreeId` with **parameter overrides**: the referenced
tree declares `parameters[{parameterName, attributeOverrideList:[{taskUuid,
attributeName}]}]`, and the caller supplies values which are applied to those
`(uuid, attribute)` pairs. A `_subtree_stack` raises `CircularReferenceError`.
**That is the whole authoring model: trees, subtrees, and named parameters that
override specific attributes of specific nodes.**

`loaders/scripted_behavior_loader.py:30` **[V]** is the other loader: a dict
of `scopeType -> {behavior_id -> factory}` with **23 entries naming 22
distinct hand-written Python archetypes** (`sleep` appears in both scopes):

- *Item* scope (19): `sleep`, `scan`, `reinforce`, `harvest`,
  `Roaming Sleeper Scout`, `Roaming Sleeper Scout In Hive System`,
  `Drifter Battleship`, `Drifter Battleship In Hive System`,
  `Drifter Commander`, `Dungeon Boss`, `Incursion Dungeon Drifter`,
  `Incursion Dungeon Seeker`, `Incursion Roaming Drifter`, `fighter`,
  `Shadow Of The Serpent`, `dps`, `tackler`, `antilogistics`, `antidps`.
- *EntityGroup* scope (4): `sleep`, `Incursion Group`,
  `Incursion Drifter Group`, `Shadow Of The Serpent Group`.

It also has `convert_script_to_fsd` (`:63`), which serialises a Python tree
into the FSD node shape — i.e. CCP's own migration path from scripted to
data-driven trees.

**The verification that settles §1.** The shipped
`spaceComponentsByType.fsdbinary` has a `behavior` component on **2,419 of
11,238 types** whose `behaviorName` is a string. Decoded with the offsets in
`findings/pyd-schemas.tsv:1434-1435`, **1,321 are decimal-digit strings** (FSD
tree ids) and the non-digit values are **[V]**:

```
'', '2026 ', 'Drifter Battleship', 'Drifter Commander',
'Incursion Roaming Amarr Navy', 'Roaming Sleeper Scout',
'Shadow Of The Serpent', 'antidps', 'antilogistics', 'dps',
'fighter', 'sleep', 'tackler'
```

Ten of those thirteen are **exactly** keys of
`ScriptedBehaviorLoader.behaviors_by_scope_by_id[Item]`. Live data in the
client points by name at Python classes in the client. That is not a leftover.
(`''` and `'2026 '` — note the trailing space — and
`'Incursion Roaming Amarr Navy'` match nothing; **[I]** they are authoring
errors or reference an FSD tree by name, and `BehaviorDoesNotExistError` is
what the server would raise. Cheap to not care about.)

This also corrects a hedge in `docs/re/dungeon-model.md` §5.1, which said the
non-digit `behaviorName`s were a lead and the trees were "where the next
person should look". They are the destination, and the join is by exact
string.

### 2.6 The task vocabulary: 368 registered classes

`registry/registrator.py:9` **[V]**: `register_task(task_type, task_class,
attributes, short_description, detailed_description)` where `task_class` is a
**dotted string**, resolved lazily by `__import__`
(`taskregistry.py:get_class_from_name`). Five types: `composite`, `action`,
`condition`, `decorator`, `monitor`. Counts **[V]**:

| type | count |
|---|---|
| action | 236 |
| condition | 68 |
| monitor | 51 |
| decorator | 10 |
| composite | 3 |
| **total** | **368** |

Actions by module (top): `ballparks` 51, `blackboards` 27, `combattargets` 17,
`entities` 16, `effects` 16, `entityspawning` 13, `group` 9, `damage` 8,
`logistics` 7, `combat` 7, `awareness` 7, `timer` 6, `dungeon` 5,
`mathtasks` 4, `fleet` 4, `combatnavigation` 4; then `reinforcements`,
`objectspawning`, `fighterabilities`, `combatmodules`, `battlefield` (3 each)
and eleven singletons including `tanking`, `tale`, `structures`, `rewards`,
`resourcewars`, `reference_subtree`, `operations`, `map`, `hauling`, `guard`,
`eventlogger`, `drones`, `randomized`.

An attribute is declared with a typed descriptor from `fsd.attributes`
(**which does ship**: `out/pyc/fsd/attributes.pyc` carries
`BehaviorTreeAttribute`, `BehaviorItemTreeAttribute`,
`BehaviorGroupTreeAttribute`, `NpcSpawnTableIdAttribute`,
`NpcTagListAttribute`, `EntitiesAndQuantitiesAttribute`,
`DungeonArchetypeListAttribute` **[V]**). Common kinds seen in the registry:
`BlackboardAddressAttribute`, `BlackboardAddressListAttribute`,
`IntegerAttribute`, `FloatAttribute`, `BooleanAttribute`, `StringAttribute`,
`InventoryCategorySetAttribute`, `InventoryGroupSetAttribute`,
`InventoryTypeSetAttribute`, `EveTypeListSelectAttribute`,
`FwAttackMethodAttribute`, `OwnerIdToSpawnlistIdDictionaryAttribute`. Each
carries `index` (editor ordering) and `is_optional`; a missing non-optional
attribute is `TaskAttributeMissingError` at load time, which is the right
place for it.

### 2.7 Roles are the archetype tuning knob

`const/behaviorroles.py` **[V]** — ten roles:
`dps`, `anti-dps`, `anti-dps (with primary)`, `anti-logistic`, `logistic`,
`tackler`, `ewar`, `ewar` (generic), `commander`, `dreadnought` — and a
`COMBAT_EFFECTS_BY_ROLES` table mapping each role to the dogma **effect ids**
it is allowed to use (`effectBehaviorWarpDisrupt`,
`effectNpcBehaviorWebifier`, `effectBehaviorWarpScramble`,
`effectBehaviorTargetPainter`, `effectNpcBehaviorGuidanceDisruptor`,
`effectNpcBehaviorTrackingDisruptor`, `effectNpcBehaviorEnergyNeutralizer`,
`effectNpcBehaviorEnergyNosferatu`, `effectBehaviorSensorDampener`,
`effectBehaviorECM`, `effectNpcBehaviorRemoteArmorRepairer`,
`effectNpcBehaviorRemoteCapacitorTransmitter`,
`effectNpcBehaviorRemoteShieldBooster`, `effectNpcBehaviorMicroJumpAttack`,
`effectNpcBehaviorSmartBomb`, `effectEntityChainLightning`). Two further
tables: `COMBAT_DISTRIBUTE_EFFECTS_BY_ROLE` (tackler, anti-dps, ewar spread
their ewar across targets instead of stacking) and
`COMBAT_IGNORE_PRIMARY_BY_ROLE` (anti-dps ignores the fleet primary).

The role is published on the entity's own blackboard by
`CreateRegisterEntityRole(role)` (`trees/role.py:15`) and read back by
`FindTarget`, `GetCombatEffectsForRole`, `GetTargetToOrbit` and
`GetFleetCombatOrbitRangeByRole` through the `ROLE_ADDRESS` channel **[V]**.
**This is the archetype knob: one string per entity, which selects both its
weapon set and its station-keeping range.**

Target selection is a named strategy, nine of them
(`const/combat.py`) **[V]**: `'zero'`, `'cost'`, `'random'`, `'module'`,
`'size'`, `'threat'`, `'range'`, `'sigRadius & velocity'`,
`'towing tow game objective'`; the default in the combat trees is `'threat'`.
Implementations are in `behaviors/targetEvaluation.py`.

Tuning constants in the same file, all **[V]** and all worth copying as
defaults: MWD distance factors 1.5 (short) / 2.0 (long) with a 5,000 m
threshold and a 1,500–20,000 m clamp; a commander-orbit velocity factor of
0.75; `COMBAT_MAX_EFFECTIVE_FALLOFF_DISTANCE_FACTOR` 0.75;
`COMBAT_MIN_EFFECTIVE_COMBAT_RANGE_FACTOR` 0.3;
`COMBAT_MAX_EFFECTIVE_DISTANCE_FACTOR` 1.5; `COMBAT_MIN_EFFECTIVE_RANGE`
30,000 m; `THREAT_MAX_DISTANCE_FROM_GUARD_OBJECT` 1,500,000 m.

### 2.8 One archetype, read end to end — this is the shape to reimplement

`trees/miners/basicdpstree.py:13` **[V]**, the `'dps'` archetype, is a
`PrioritySelector` of seven children in priority order:

```
PrioritySelector "Basic Dps Tree"
  1 CreateRegisterEntityRole(ROLE_DPS)      # publish my role, always fails through
  2 CreateTargetSwitchingBehavior()         # 60 s cooldown, re-pick the group primary
  3 CreateEntityCombatBehavior()            # the fight
  4 CreateUnspawnBehavior()
  5 CreateReinforceLocationBehavior()
  6 CreateReGroupBehavior()
  7 WaitAction("Do nothing")
```

`CreateEntityCombatBehavior` (`trees/combat.py:156`) is a `Sequence`:

```
Sequence "Combat"
  WaitForAggressiveAct(combatTargetsAddress=COMBAT_TARGETS_SET)   # subscribe, succeed
  HasAnyEffectCondition("Can I fight?", effectIds=COMBAT_EFFECTS) # 20 effect ids
  MaxLockedTargetsChangedMonitor()
  BlackboardMessageMonitor(COMBAT_TARGETS_SET)
  CreateSelectTargetBehavior(...)       # keep target if still valid+in bubble, else FindTarget
  CreateGetCombatNavigationBehavior()   # role -> effects -> min optimal/falloff -> orbit+attack range
  CreateAttackTargetBehavior()          # below
```

`CreateAttackTargetBehavior` (`trees/combat.py:203`) in order **[V]**:

```
ForceSuccess[ Sequence[ Not[target == MY_COMBAT_TARGET], StopCombatEffectsOnTarget ] ]
ClearValueAndResetIfTargetCloaks
TargetLeavingParkMonitor(clearValue=True)
BlackboardCopyMessageAction(target -> MY_COMBAT_TARGET)
BlackboardMessageMonitor(target)
TargetLostMonitor(target)
TargetChangingBubbleMonitor(target, resetOnEnter=False)
ModifyOrbitVelocity(orbitVelocity)
OrbitAtDistanceAction(COMBAT_ORBIT_TARGET, COMBAT_ORBIT_RANGE, blocking=False, blockUntilRange=COMBAT_ATTACK_RANGE)
ManageMicroWarpDriveToTarget(microWarpDriveDuration=5000, microWarpDriveChance=1.0)
PrioritySelector[ HasAvailableTargetCapacity, UnlockTarget ]
RepeatWhileFailing(maxRepetitions=7)[ AcquireTargetLock(target) ]
ActivateCombatEffectsOnTarget(effectIds=COMBAT_EFFECTS, repeat=1000)
WaitAction()
```

Read that list as the specification for "a bot fights": *drop effects if the
target changed; abandon the target if it cloaks, leaves the park, or leaves the
bubble; orbit at a range derived from the role and the weapon's own optimal and
falloff; MWD into range; free a target slot if full; retry the lock seven
times; fire everything; wait.* Every verb in it maps to §7's table.

Other archetypes worth lifting verbatim: `trees/guardobject.py:19` (orbit a
guarded object, watch its proximity sensor, warp back if out of bubble),
`trees/patrol.py:75` (pick an unexamined object within 100 km on a 90 s timer,
orbit it at 10 km, repeat), `trees/travel.py` (warp to a blackboard
coordinate, then optionally unspawn), `trees/explore.py:17`
(`FindPointToExplore` on a 1,800 s interval).

---

## 3. Does an NPC ball differ from a player ball on the wire?

**No, in the ball record; barely, in the slim dict; not at all in a way that
needs a new key.**

### 3.1 The ball record: identical, and it has no identity at all

Our `destiny::Ball` (`server/crates/eve-server/src/destiny.rs:176-192`) is the
complete record the client's native reader consumes: `ball_id`, `mode`
(+ its tail), `radius`, `position`, `flags`, `mass`, `cloaked`, `harmonic`,
`corporation_id`, `alliance_id`, optional 36-byte physics block,
`formation_id`. **There is no owner, no character, no type, no NPC bit and no
ball-class id** **[V]**. CCP's own `__destiny_ball_class_id__` (§1.2c) is a
*server* class selector carried in server FSD
(`destinyServerBallClass.classID`), never on the wire **[I]**, and the
inference is strong: a field the reader does not read cannot be sent.

So `AddBalls2`'s blob is byte-for-byte the same job for a bot as for a human.

### 3.2 The slim dict: five keys, and only `ownerID` decides NPC-ness

`AddBalls2` replaces the `SlimItem` instance dict wholesale
(`michelle.py:1698`), and `docs/lanes/addballs.md` §5.1 already settled the
five keys we send: `itemID`, `typeID`, `ownerID`, `charID`, `corpID`, with
`groupID`/`categoryID`/`radius` derived client-side from `typeID` and every
other attribute returning `None` from `SlimItem.__getattr__`.

The NPC test, in full **[V]**:

```
bracketVarious.py:435  def IsNpcObject(slimItem):
             :436      if slimItem.categoryID == const.categoryEntity and slimItem.groupID == const.groupLargeCollidableObject:
             :437          return False
             :438      return not IsPlayerOwner(slimItem.ownerID)

idCheckers.py:318      def IsPlayerOwner(ownerID):
            :319          return minPlayerOwner <= ownerID <= maxPlayerOwner     # 90000000 .. 2147483647
```

and the id ranges, from `inventorycommon/const.py` **[V]**:

| range | meaning |
|---|---|
| `<= 9999` | system |
| `500000 .. 599999` | faction |
| `1000000 .. 1999999` | NPC corporation |
| `90000000 .. 97999999` | player character (gen 2) |
| `98000000 .. 98999999` | player corporation |
| `2100000000 .. 2129999999` | player character (gen 3) |
| `90000000 .. 2147483647` | **`IsPlayerOwner`** |
| `100000000 .. 9e18` | player item |
| `> 9000000000000000000` | "fake item" |

**So the discriminator is a range test on one integer, and we choose the
integer.** Give a bot `ownerID = charID = <90000000-range id>` and
`corpID = <98000000-range id>` and the client's NPC branches are all skipped.

### 3.3 The `extraBallData` dict needs no NPC marker

Settled negative **[V]**. The brief's guess — that an NPC needs a marker key in
the `extraBallData` dict — is wrong. There is no such key, in either direction:
nothing marks a ball as an NPC and nothing marks one as a player. Every NPC
branch in the client keys off `ownerID`'s range, or off `categoryID`/`groupID`
derived from `typeID`, or off a **space component on the typeID** read from the
client's own shipped FSD (§4.2). The dict is the same five keys.

What the dict *can* carry for an NPC-flavoured ball, each read by exactly one
consumer and each `None`-safe if omitted **[V]**:

| key | read by | effect if omitted |
|---|---|---|
| `name` | `ProcessBallAdd:1635` → `cfg.evelocations.Hint`, then `uix.GetSlimItemName:493` | falls back to `evetypes.GetName(typeID)` |
| `hostile_response_threshold` / `friendly_response_threshold` | `entitystandings._get_icon_color_and_hint_from_entity_standing_response:63` | logged as an error, then the **type's** FSD thresholds are used — not a crash |
| `fwAttackMethod` | `bracketVarious.py:120-125` | faction-warfare colouring skipped |
| `group_standings_disabled` | `bracketVarious.py:132` | standings colouring applies |
| `dungeonNameID` | `uix.GetSlimItemName` beacon branch | ordinary name |
| `nameID` | `ProcessBallAdd:1628-1631`, fake items only | ordinary name |
| `bounty` | `SlimItem.__getattr__` returns `0` | zero bounty |

### 3.4 The two places the client *does* branch, and what each costs

**(a) Owner priming.** `Park.__init__:1013` sets
`__categoryRequireOwnerPrime__ = (categoryShip, categoryDrone)` = `(6, 18)`
**[V]**, and `ProcessBallAdd:1615-1620` only primes `ownerID`/`charID`/`corpID`
for those two categories. NPC ships are `categoryEntity = 11`, so an NPC ball
is **not** owner-primed. Cost: nothing, because §4.1 shows the client already
has every NPC owner name locally. A bot in a `categoryShip` hull takes the
player path and is primed exactly like a human.

**(b) Naming.** `uix.GetSlimItemName:487` **[V]**:

```
488  if slimItem.categoryID == const.categoryShip or HasNpcPilotComponent(slimItem.typeID):
489      if not slimItem.charID or (slimItem.charID == session.charid and slimItem.itemID != session.shipid):
490          return evetypes.GetName(slimItem.typeID)
491      if idCheckers.IsCharacter(slimItem.charID):
492          return cfg.eveowners.Get(slimItem.charID).name
...
493+ ... asteroid/cloud -> type name; mineable -> output type; orbital; disrupted gate
     locationname = cfg.evelocations.Get(slimItem.itemID).name   # Hinted at :1635
     ... else return evetypes.GetName(slimItem.typeID)
```

So a ball is named after its **pilot** when `categoryID == categoryShip`
*or* its type carries the `npcpilot` space component, and after its **type**
otherwise. That second disjunct is CCP's own "player-like NPC" switch, and it
is real data: 48 types carry `npcpilot = {characterIds: dict<str,i32>}`
(`findings/pyd-schemas.tsv:1328,1620`; decoded count **[V]**).

### 3.5 The overview filters on `groupID`

`slimItemDisplayChecker.ShouldDisplayItem:26` **[V]**, in order: not my own
ship; a preset override; `typeID not in OVERVIEW_IGNORE_TYPES`;
**`slimItem.groupID in presetSvc.GetValidGroups(preset)`**; not filtered by
state; structures only if the proximity tracker says visible. `groupID` is
derived from `typeID` client-side. **A bot in a player hull is listed by the
default preset for the same reason a human is** — same group. An NPC entity
type is listed iff the active preset includes its group, which is a client-side
setting we do not control and do not need to.

---

## 4. What the client must be told about an NPC that it is not told about a player

### 4.1 Nothing. The client already knows every NPC owner. [V]

This is the strongest result in §4 and it closes the brief's "an unanswered
lookup is a timeout" worry outright.

`eveCfg.LoadEveOwners:717` **pre-populates `cfg.eveowners.data` locally**
before any call, from shipped FSD **[V]**: every **faction** (`iter_factions`),
every **NPC corporation** (`iter_npc_corporations`,
`npccorporations.fsdbinary`, 713 KB) and every **NPC character**
(`iter_npc_characters`, `npccharacters.fsdbinary`, 1.87 MB), each as a real
`blue.DBRow` of `[ownerID, name, typeID, gender, nameID]`.
`eveCfg.LoadCrpTickerNames:602` does the same for every NPC corporation's
ticker.

And `cfg.Recordset._Prime:763` subtracts already-cached keys before fetching
**[V]**:

```
777  keysIAlreadyHave = set()
778  for key in keysToGet:
779      if key in self.data: keysIAlreadyHave.add(key)
...      keysToGet -= self.knownLuzers
         keysToGet -= keysIAlreadyHave
```

`Recordset.Hint:693` writes straight into `self.data` **[V]**, which is why
`ProcessBallAdd:1635`'s location hint also costs no fetch.

**Therefore:** an NPC ball's `ownerID` (an NPC corp), its `charID` (an NPC
character, if any) and its corp ticker **never reach the server**. The only ids
that produce a `config.GetMultiOwnersEx` / `GetMultiCorpTickerNamesEx` are
player-range ones — which is exactly the set our `multi_owners`
(`service.rs:2820-2843`) and `multi_corp_tickers` (`:2850-2866`) already
resolve, for any id in `config.characters`.

### 4.2 Everything else about an NPC is in the client's own static data

| the client wants | where it gets it | server call? |
|---|---|---|
| type name | `evetypes.GetName(typeID)`, `types.fsdbinary` + localisation | no |
| `groupID`, `categoryID`, `radius` | derived from `typeID` (`slimItem.py:26-34`) | no |
| icon / bracket art | `spaceObjectIcon`, type+group | no |
| NPC corp name & ticker | pre-primed, §4.1 | no |
| NPC character name | pre-primed, §4.1 | no |
| corp → faction | `npccorporations.get_corporation_faction_id` (client FSD) | no |
| hostility thresholds | `spaceComponentsByType.entityStandings` (client FSD) | no |
| "is this type AI-driven" | `HasBehaviorComponent(typeID)` → `spaceComponentsByType.behavior` | no |
| my standings to a faction | `sm.GetService('standing').GetStandingWithSkillBonus` | **yes**, `standingMgr`, already answered |
| health bars | `extraBallData` pair, then `OnDamageStateChange` | **push** |

The one genuine service dependency is `standingMgr`, called once per
charid/corpid session change by `standingsvc.__RefreshStandings:118` **[V]**
(`GetNPCNPCStandings`, `GetCharStandings`, `GetCorpStandings`). We already
answer all three — `service/character.rs:1313`, `:1329`, `:1337`, with
`GetNPCNPCStandings` noted as "the strictest empty-value caller we have met"
(`service.rs:7353`). **No new call.**

### 4.3 The three FSD component schemas you need if you want CCP's own NPC flavour

From `spaceComponentsByType.fsdbinary` (11,238 rows, stride 1,832), offsets
from `findings/pyd-schemas.tsv`, values decoded in this lane **[V]**:

```
record+64    behavior           { behaviorName: str @+0, groupBehaviorID: i32 @+8 (opt bit 0x1 @+12) }
                                present on 2,419 types; optional bit 0x100 at word +1816
record+536   npcpilot           { characterIds: dict<str,i32> @+0 }
                                present on 48 types; optional bit 0x2000000 at word +1820
record+1404  entityStandings    { friendlyResponseThreshold: f32 @+0, hostileResponseThreshold: f32 @+4 }
                                present on 7,427 types; optional bit 0x1 at word +1820
record+336   entityBounty       { bountyMultipliersByOwner: dict<str,f64> }
record+1396  dynamicEntityName  { dummyParameter: u8 }      # a marker; the name arrives via UpdateSlimItemField
```

One sample row each **[V]**:

```
typeID 84294  behavior.behaviorName = '2043'   npcpilot present
typeID 33886  behavior.behaviorName = '120'
typeID 16857  entityStandings.friendly = 11.0  .hostile = -11.0
```

`entityStandings` threshold histogram over all 7,427 rows **[V]**:
`(friendly, hostile) = (11.0, 11.0)` on **5,498**; `(11.0, -11.0)` on 1,543;
`(-11.0, -11.0)` on 295; `(5.0, -5.0)` on 80; `(3.0, 0.0)` on 6;
`(2.0, -0.0)` on 3; `(8.0, -8.0)` on 2. Standings run −10..+10, so `11.0` means
"always" and `-11.0` means "never". §5.2 reads the consequence.

---

## 5. Aggression, and the things only a server can decide

### 5.1 What the client renders, and from what

Hostility is **three independent channels**, and only the third is purely
server-driven.

**(a) Standings colour — client-computed, server-supplied inputs.**
`bracketVarious.GetIconColor:107` **[V]**, in branch order:

```
113  if slimItem.categoryID in (categoryShip, categoryDrone):
         if slimItem.itemID == session.shipid: own-ship colour
118  elif IsNpcObject(slimItem) and slimItem.typeID:
         iconColor = ICON_COLOR_NEUTRAL
120      fwAttackMethod = getattr(slimItem, 'fwAttackMethod', None)
122      if fwAttackMethod and session.warfactionid: ... fw colouring
132      if fwAttackMethod is not None and HasBehaviorComponent(typeID) and slimItem.group_standings_disabled is True: pass
135      elif HasEntityStandingsComponent(typeID):
136          iconColor, sort, hint = get_icon_color_sort_value_and_hint_for_owner_standing(slimItem, session.charid)
```

**Note the order: the ship/drone branch comes first.** A ball in a
`categoryShip` hull *never reaches the NPC branch*, whatever its `ownerID`.
A bot in a real hull is coloured by the ordinary player path.

For an entity, `get_icon_color_sort_value_and_hint_for_owner_standing`
(`entitystandings.py`, `_get_icon_color_and_hint_from_entity_standing_response:63`)
**[V]**: `standings = standing.GetStandingWithSkillBonus(get_npc_faction(ownerID), charid)`;
`standings <= hostile_response_threshold` → red (sort 1,
`Tooltips/Overview/HostileNPC`); `>= friendly_response_threshold` → blue;
else white (`Tooltips/Overview/NeutralNPC`). Then, **if not already red**, it
checks `stateSvc.GetState(itemID, state.threatAttackingMe)` and goes red if
set.

**(b) The 70 state flags** (`parklife/states.py`, version 5) **[V]**. The ones
a fight needs: `targeting` 4, `targeted` 5, `threatTargetsMe` 7,
`threatAttackingMe` 8, plus the ewar flags `flagWarpScrambled` 38,
`flagWebified` 39, `flagECMd` 40, `flagSensorDampened` 41,
`flagTrackingDisrupted` 42, `flagTargetPainted` 43, `flagEnergyLeeched` 46,
`flagEnergyNeut` 47, `flagGuidanceDisrupted` 55, `flagWeaponsTimer` 67, and
the crimewatch flags `flagSuspect` 50, `flagCriminal` 51,
`flagLimitedEngagement` 52, `flagOutlaw` 9.

**Which of those the client sets by itself, given the right pushes** **[V]**:

- `threatTargetsMe` ← `targetMgr.OnTargetByOther(otherID)` / `…Lost`
  (`targetMgr.py:1118-1119`, `:1146-1147`), reached from
  `OnTarget('otheradd'|'otherlost', tid, reason)`.
- `threatAttackingMe` ← `tactical.OnSpecialFX` (`tactical.py:434` set,
  `:448` clear) when `targetID == session.shipid and isOffensive`.
- the ewar flags ← the same `OnSpecialFX` stream, by `guid`.

**So we do not send state flags. We send two notifications and the client
derives the flags.** That is a large saving and it is verified.

**(c) The safety — already answered, already permissive.**
`crimewatchSvc.Initialize:81` defaults `self.safetyLevel =
const.shipSafetyLevelFull` (= 2) and `OnSubscriptionChanged` overwrites it from
`eveMoniker.CharGetCrimewatchLocation().GetClientStates()[3]` **[V]**. We
answer that bound call (`service/space.rs:270-296`) with safetyLevel **`0`** =
`shipSafetyLevelNone`, so **the client imposes no safety gate on a player
attacking anything** — which is what a private sandbox wants.
`SetSafetyLevel(level)` is a write we currently ignore; §9 ranks it.

### 5.2 The default NPC is red, for free

Put §4.3's histogram together with our empty standings tables
(`GetCharStandings` → `{}`, `service/character.rs:1329`) **[V]**:
`get_standing_between_npc_and_player` returns `0.0`, and
`0.0 <= hostileResponseThreshold = 11.0` is true for **5,498 of 7,427** typed
entities. So those types render **red, hostile, sort order 1**, with no server
involvement at all. 1,543 more are `(11.0, -11.0)` → neutral white.

`npcs/client/entitystandings.is_npc_hostile:20` is
`hostile_response_threshold > 0.0` **[V]**, so the `11.0` group are also
"hostile NPC targets" for the objective and node-graph code.

### 5.3 What only the server can decide, and therefore what it must send

| decision | wire |
|---|---|
| who the bot has locked | `OnTarget('otheradd', bot_ship_id)` to the victim; `OnTargets([...])` to batch |
| the bot stopped locking | `OnTarget('otherlost', bot_ship_id, reason)` |
| the player's own locks succeeded | `OnTarget('add', tid)`, `('lost', tid, reason)`, `('clear',)` |
| a module fired | `OnSpecialFX(shipID, moduleID, moduleTypeID, targetID, otherTypeID, guid, isOffensive, start, active, duration=-1, repeat=None, startTime=None, timeFromStart=0, graphicInfo=None)` |
| damage landed | `OnDamageStateChange(shipID, damageState)`, same 3-sequence `((shield, tau, _), armour, hull)` as `extraBallData` |
| a ball died | `RemoveBalls` with `isRelease` — `docs/lanes/addballs.md` §3.3 |
| standings/flags changed mid-flight | `OnSlimItemChange(itemID, newSlim)` |

`OnTargets(targets)` is `for each in targets: self.OnTarget(*each[1:])`
(`targetMgr.py:655-657`) **[V]** — note the `[1:]`: each element's **first**
item is dropped, so the element is `(something, what, tid, reason)`. Getting
that wrong is the obvious first bug.

### 5.4 CCP's own aggression model, which we should copy

`WaitForAggressiveAct` (`monitors/combat.py:18`) **[V]** subscribes the entity
to two server-internal events on its own item messenger —
`MESSAGE_CRIMEWATCH_EVENT_STARTED` and `MESSAGE_INDIRECT_OFFENSE_EVENT` — and
on either one adds the aggressor to the **group-scoped**
`COMBAT_TARGETS_SET` channel and calls `behaviorTree.RequestReset()`. The
aggressor id is laundered through `crimewatch.util.GetAggressorId(categoryID,
sourceOwnerID, aggressorID)`, which **maps a drone or fighter back to its
controller's ship** (`crimewatch/util.py`, `GetShipIDForCharID`) **[V]**.

Two design points to lift: aggression is **shared at the group level, not the
individual**, and a drone's aggression counts as its owner's.

---

## 6. Spawning

### 6.1 What the shipped data models

Authored PvE spawns live in `res:/staticdata/dungeons.fsdbinary` (52 MB; 5,516
dungeons / 8,483 rooms / 260,242 objects / 30,610 entities) and are already
decoded — `docs/re/dungeon-model.md` §5.1, `findings/corpus-dungeons.tsv`. The
spawn rule is **two inline structs on a dungeon object**, not a separate table.

Schema, offsets struct-relative, from `findings/pyd-schemas.tsv:396-433`
**[V]** (`dungeons` loader; the `*` names also have a **[V2]** second witness
at build 3475087, see `dungeon-model.md` §5.1a):

```
entity  (dict on the object, 30,610 rows)
  +0   behaviorName       str   opt 0x2    # FSD tree id as a decimal string, OR a ScriptedBehaviorLoader key
  +8   groupBehaviorName  str   opt 0x8    # group tree, 23 distinct values
  +16  notes              str   opt 0x10
  +24  objectiveTargetGroup str opt 0x40
  +32  additionalLootId   i32   opt 0x1
  +36  entityID           i32
  +40  entityNameID       i32   opt 0x4    # message id; 4,578/4,578 resolve
  +44  npcGroupingID      i32   opt 0x20   # NOT a typeID -- dungeon-model.md §5.5
  +48  objectID           i32
  +52  triggerSpawn       u8    opt 0x80   # true on 9,355 of 30,610

guardCommand  (inline struct on the object, 39,956 rows, at +64 of the object record, opt 0x8 @ +148)
  +0   aggressionRange        f32  m   # 250,000 on 14,522 objects; 30,000 on 6,824
  +4   entityMaxRespawnDelay  i32  s
  +8   entityMinRespawnDelay  i32  s   # 3,600 on ~26,000; 86,400 on ~11,000
  +12  jitterRadius           i32  m   opt 0x1   # exactly three values: 500 / 1,500 / 2,500
  +16  objectMaxRespawnDelay  i32  s
  +20  objectMinRespawnDelay  i32  s
  +24  orbitRange             f32  m   # 1,500 on 24,195 objects; 5,000 on 9,748
  +28  entityDelaySpawn       u8
  +29  entitySpawnOnce        u8
  +30  objectDelaySpawn       u8
  +31  objectTriggerSpawn     u8       # true on 8,069 objects
```

`tools/fsd.py` itself carries only `CFSD_COLUMNS["dungeons"]`,
`CFSD_ROOM_COLUMNS` (`roomID` @0, `objects` dict @8/@40) and
`CFSD_OBJECT_COLUMNS` (`typeID` @148); the two structs above are in
`findings/pyd-schemas.tsv` and were decoded by the `dungeon-model` lane.

**Not shipped, and we author it ourselves [V]:** `behaviortrees` (the trees),
any spawn table (`npcs.server.npcspawning.spawntableloader`,
`spawnlists.data`), any loot table (`dungeon-model.md` §6 — eight fields
across five tables reference loot table ids and no table of loot tables
exists), and `npcGroupingID`'s target table.

### 6.2 How a population comes into being, in CCP's model

Three doors, all **[V]**:

1. **Dungeon placement.** A dungeon's rooms carry objects; an object with an
   `entity` dict spawns NPCs when the room is instantiated, unless
   `triggerSpawn`/`objectTriggerSpawn` gates it on a trigger. Respawn is the
   `entityMin/MaxRespawnDelay` pair. The entity's AI is
   `behaviorName` (item scope) and `groupBehaviorName` (group scope).
2. **A behaviour spawns more behaviour.** The 13 `entityspawning` actions
   (§6.3) let a running tree spawn entities, including reactive spawning that
   scales to the player fleet in the system
   (`ReactiveSpawning`, 24 attributes, with `shouldMatchPlayerPoints`,
   `spawnPointThreshold`, `minSpawnIntervalSeconds`, a tracked type list and a
   threat range).
3. **Per-type default AI.** `spaceComponentsByType.behavior` gives 2,419 types
   a default `behaviorName`, so *any* spawn of that type has AI without the
   spawner saying so.

### 6.3 The minimum for "three frigates orbit this gate"

In CCP's vocabulary it is **one action plus one tree** **[V]**:

```
SpawnNewEntities(
    behaviorTreeName      = <item tree>,          # e.g. 'dps'
    behaviorTreeGroupName = <group tree, optional>,
    entities              = {<frigate typeID>: 3},   # EntitiesAndQuantitiesAttribute
    spawnNewGroup         = True,
    spawnPositionAddress  = <blackboard coord>,
    deploymentRange       = <metres, default npcspawning.DEFAULT_DEPLOYMENT_OFFSET_METERS>,
    guardObjectAddress    = <blackboard item id>,    # the gate
    bountyPayOutMultiplier= 1.0)
```

then the item tree is `CreateGuardObjectBehavior` (`trees/guardobject.py:19`):

```
PrioritySelector "Guard Object"
  Sequence "Orbit Guard Object"
    IsBallPresentInMyBubbleCondition(GUARD_OBJECT_ITEM_ID)
    OrbitAtDistanceAction(orbitTargetAddress=GUARD_OBJECT_ITEM_ID,
                          orbitRangeAddress=GUARD_OBJECT_ORBIT_RANGE, blocking=False)
    MonitorSpecificProximitySensor(objectListAddress=COMBAT_TARGETS_SET,
                                   includedCategories=[categoryShip, categoryDrone, categoryFighter],
                                   excludedGroups=[groupCapsule],
                                   proximitySensorAddress=GUARD_OBJECT_ITEM_ID)
    WaitAction("Wait until disturbed")
  WarpToNewLocation(GUARD_OBJECT_POSITION)      # fallback: warp back if out of bubble
```

**In our terms**, with no behaviour-tree engine at all, the same thing is:
three `Character` rows in `config.characters`; three ship ids; three
`Presence` records in the roster at positions `gate ± deploymentRange`; one
`BallsAdded` fanned to everyone in the system; and one `ActionUpdate` carrying
three `Action::Orbit { ball, target: gate_id, range: 1500.0 }`. That is
**today's code**, §7.

### 6.4 One correction to carry forward

`docs/re/dungeon-model.md` §5.1 left `behaviorName`'s non-digit values as an
open lead. §2.5 of this document closes it: they are `ScriptedBehaviorLoader`
keys, matching ten for thirteen exactly. The lane note records the amendment;
this document does not edit that file.

---

## 7. The bot driver's interface

This is the part meant to still be read in six months. **A server-side bot
driver needs no new client-facing machinery for flight — only an internal API,
and most of it exists.**

### 7.1 CCP's NPC verbs are our destiny verbs

`behaviors` calls the server's own ballpark. The full method set it uses
**[V]** (census over all 178 modules):

```
movement:    GotoPoint  GotoDirection  SetSpeedFraction  StopEx  WarpTo
             EntityWarpStart  ChangeBallPosition  MissileFollow  SetBallMassive
state:       CloakShip  UncloakShip  IsCloaked  HasPendingCloak
             MakeInvulnerablePermanently  CancelCurrentInvulnerability  IsInvulnerable
             IsWarping  IsWarpScrambled  IsExploding  IsBallProtected  SetTargetTracking
space query: GetBall  GetBallOrNone  GetBallPosition  HasBall  CheckBall
             GetBallIdsAndDistInRange  GetItemsInBubble  GetShipsInBubble
             GetShipsInSameBubble  InSameBubble  GetBubbleAtCoordinates
             DistanceBetween  GetSurfaceDist  SetBubbleEvents
warp maths:  AdjustDstByJitter  ApplyWarpDisruptors  GetWarpDestinationPoint
             GetClosestDisruptorAndIntersectionInWarpPath
lifecycle:   ExplodeEx  UnspawnEntityBall  RemoveItemFromPark
slim:        GetSlimItem  UpdateSlimItemField  UpdateSlimItemFieldMulti  slims
inventory:   GetInvItem  inventory2  componentRegistry  dogmaLM  messenger  eventMessenger
```

and the dogma manager **[V]**:

```
targeting:  AddTargetEx(ship, target)  RemoveTarget(ship, target)  GetTargets(ship)
            GetTargetsEx(ship)  IsTargeting(ship, target)  HasPendingTarget(ship, target)
modules:    ActivateWithContext(ownerID, moduleID, effectID, targetid=, repeat=, context=)
            StopEffect(effectID, itemID, forced=True)  IsItemActive
            GetActiveEffectData(item, effect)  GetActiveEffectEnvironment(item, effect)
attributes: GetAttributeValue(item, attr)  SetAttributeValue
            dogmaStaticMgr.GetTypeAttribute2(type, attr)  dogmaStaticMgr.TypeHasEffect(type, effect)
misc:       IsShipPiloted(itemID)  GetFittedModulesGroupIdsForShip  GetCloudFactor
            RegisterEffectCallback / UnregisterEffectCallback
```

The relevant observation for us: **every one of those is a server-internal
call, and the movement five are precisely what `space_state::command` already
produces from the client's `Cmd*`** (`space_state.rs:385-416`) **[V]**:

| CCP server verb | our `destiny::Action` | our mapper |
|---|---|---|
| `GotoPoint` | `GotoPoint { ball, point }` | `CmdGotoPoint` |
| `GotoDirection` | `GotoDirection { ball, direction }` | `CmdGotoDirection` / `CmdSteerDirection` |
| `SetSpeedFraction` | `SetSpeedFraction { ball, fraction }` | `CmdSetSpeedFraction` |
| `StopEx` | `Stop { ball }` | `CmdStop` |
| (follow) | `FollowBall { ball, target, range }` | `CmdFollowBall` |
| (orbit) | `Orbit { ball, target, range }` | `CmdOrbit` |
| `ChangeBallPosition` | `SetBallPosition { ball, point }` | — (server-only, as CCP's is) |
| — | `SetBallVelocity`, `SetMaxSpeed` | — |
| `WarpTo` / `EntityWarpStart` | **missing** — `Mode::Warp` is `[V2]` only | `CmdWarpToStuff` in `UNIMPLEMENTED` |

### 7.2 The operations a bot driver needs, and where each one stands today

The driver's whole job is to produce a sequence of these. Nothing here requires
a client.

| # | operation | how, in our code today | status |
|---|---|---|---|
| 1 | **exist as an identity** | a `Character` row in `config.characters` (`service.rs:283-316`): `character_id` in `90000000..`, `name`, `corporation_id` in `98000000..`, `ship_type_id`. This is what makes `multi_owners` (`:2820`) resolve the name and `multi_corp_tickers` (`:2850`) the ticker | **exists** |
| 2 | **appear in space** | `space_state::Presence::arriving(character_id, ship_id, solar_system_id, motion)` into `Roster::arrive` (`space_state.rs:476`, `:518`) | **exists** |
| 3 | **be seen by everyone there** | `destiny::BallsAdded::new(to, stamp, …)` (`destiny.rs:1816`) with the five-key `Slim` (`:805`) and a `DamageState` pair (`:1060`); audience from `Roster::others_in_system` (`:540`) | **exists** |
| 4 | **be in a newcomer's bootstrap** | `destiny::bootstrap_for` (`:1290`) already builds the whole system from the roster | **exists** |
| 5 | **set a destination / fly** | `destiny::Action::{GotoPoint, GotoDirection, SetSpeedFraction, Stop, FollowBall, Orbit}` in an `ActionUpdate::new(to, stamp, actions)` (`:1712`), fanned to the system. One distinct stamp per notification or the client calls `OnFatalDesync` | **exists** |
| 6 | **have its position be true later** | `Presence.since` + `Motion::advanced(hull, dt)` (`space_state.rs:189`) integrates on read; `Motion::after(action)` (`:106`) folds the command in | **exists** |
| 7 | **align / warp / jump a gate** | `Mode::Warp`'s ball tail is **[V2]** only and `CmdAlignTo`/`CmdWarpToStuff*`/`CmdStargateJump` are in `UNIMPLEMENTED` (`space_state.rs:430-437`). A bot is confined to sub-warp manoeuvres until that lands | **missing** |
| 8 | **leave** | `destiny::BallsRemoved::new(to, stamp, balls)` (`:1922`), one argument only; `Roster::leave` (`:524`) | **exists** |
| 9 | **lock target X** | server-side list of locks per ship, cap from the hull's dogma; push `OnTarget('otheradd', bot_ship_id)` to X's owner | **missing push** |
| 10 | **drop a lock** | `OnTarget('otherlost', bot_ship_id, reason)` | **missing push** |
| 11 | **activate module in slot N at X** | CCP's `dogmaLM.ActivateWithContext(ownerID, moduleID, effectID, targetid, repeat)`. We have no dogma location server-side. Client-visible part is `OnSpecialFX(...)` (14 positionals) | **missing** |
| 12 | **deactivate** | same `OnSpecialFX` with `start=False` | **missing** |
| 13 | **deal / take damage** | `OnDamageStateChange(shipID, damageState)` using the existing `DamageState::to_pyvalue` (`destiny.rs:1076`) | **missing push** |
| 14 | **die** | `BallsRemoved` with a truthy `isRelease` + wreck item — `addballs.md` §3.3 warns truthy `isRelease` is the one to fear | **partly** |
| 15 | **change a visible field in flight** | `OnSlimItemChange(itemID, newSlim)`; CCP's own equivalent is `ballpark.UpdateSlimItemField` | **missing push** |
| 16 | **be in a fleet** | `fleet.rs` / `service/fleet.rs` exist; a bot would need to be a fleet member for broadcasts and fleet damage (`OnFleetDamageStateChange`, `michelle.py:2080`) | **partly** |
| 17 | **dock / undock** | `docking.rs`, `destiny::undock_motion` (`:1238`), `UNDOCK_OFFSET` 20,000 m. Note §27.6: two ships undocking from one station land on the same point | **exists, cosmetic bug** |
| 18 | **be reasoned about by the driver** | `Roster::in_system` (`:529`), `Motion::advanced`, `space_state::distance` (`:266`), `destiny::hull::find(type_id)` (`:715`) for radius/mass/max velocity/agility | **exists** |

**Five of the eighteen are missing, and four of those five are `OnTarget`,
`OnSpecialFX`, `OnDamageStateChange` and `OnSlimItemChange` — pushes that two
humans shooting each other need identically.** The fifth, warp, is a ball-mode
tail we have decided not to encode on `[V2]` evidence, with a known cheap way
out (`docs/lanes/round2-spec.md` §8: warp a ball and let the client's own
writer tell us).

### 7.3 The shape to give the driver

Three pieces, and the middle one is the only new concept:

1. **A tick.** CCP's is `BehaviorTree.Tick(context)` per entity, called by the
   ballpark. Ours wants one thread or one timer stepping every bot in a system,
   because `Registry::pending` is drained only from `Dispatch::on_payload`
   (`docs/lanes/two-in-space.md`) — **a bot that acts while every human is idle
   produces a push that sits in the queue until some connection sends a
   payload.** That asymmetry is the first thing a bot driver will hit, and it
   is already written down.
2. **A per-bot decision function** returning a list of operations from §7.2.
   Start as a flat `PrioritySelector` of closures; adopt §2's engine only when
   a tree needs authoring by data rather than code.
3. **A blackboard**, if and only if bots must coordinate. `(scope, channel)`
   keyed `HashMap` with a last-value-plus-timestamp per channel reproduces
   §2.4 in about fifty lines, and the `EntityGroup` scope is what makes "focus
   fire the fleet primary" a one-line read.

### 7.4 The decision that is actually load-bearing: what id a bot gets

**Give bots player-range identities.** `ownerID = charID` in
`90000000..97999999`, `corpID` in `98000000..98999999`, a real
`categoryShip` hull. Consequences, each **[V]**:

- `IsNpcObject` false, so every NPC branch in the UI is skipped;
- `GetIconColor`'s first branch (ship/drone) takes it, so standings and
  faction-warfare colouring never run;
- `__categoryRequireOwnerPrime__` matches, so the pilot and corp are primed
  through calls we already answer;
- `GetSlimItemName:492` returns `cfg.eveowners.Get(charID).name` — the bot's
  own name, in the bracket and the overview;
- the default overview preset lists it with the hull's group;
- the client can show-info it, add it to the address book, and look up its
  public info — which is `charMgr.GetPublicInfo`, already answered
  (`service/character.rs:104-185`).

The cost is that each bot is a `Character` row. The alternative — CCP's
`npcpilot` route with an NPC-corp `ownerID` and an NPC-character `charID` —
is cheaper (names come from the client's own `npccharacters.fsdbinary`) but it
puts the ball through the NPC branches, where the standings colour and the
overview group are decided by shipped FSD rather than by us. **[A]** We ship
the player-identity route for bots meant to be mistaken for players, and keep
`npcpilot` in mind for scenery NPCs. Falsified if a client-side check we have
not found refuses a player-range `ownerID` for a ball whose type has a
`behavior` component, or if `GetPublicInfo` for a bot turns out to need a row
we cannot synthesise.

---

## 8. Falsifiable tests

Properties, not our implementation. Each names the observation that fails it.

**T1 — a bot ball and a player ball differ only in the ids.** Build two
`AddBalls2` appends from the same hull type, one with
`ownerID = charID = 90000001`, one with `ownerID = 1000126, charID = 3019500`.
Assert the **ball blobs are byte-identical** and the slim dicts differ in
exactly `ownerID` and `charID`. *Fails if* any encoder path branches on the
owner.

**T2 — `IsNpcObject` is a pure range test.** For `ownerID` in
`{1, 9999, 500000, 1000126, 89999999, 90000000, 2147483647, 2147483648}`,
`IsPlayerOwner` is true exactly on `[90000000, 2147483647]`. *Fails if* any
value needs a `cfg` lookup, which would make a bot's NPC-ness depend on a
served row.

**T3 — a newcomer's bootstrap contains every bot in the system.**
`bootstrap_for` for a joining human, with N bots in the roster, yields
`N + 1 + stations` balls and `every_slim_has_a_ball()`. *Fails if* bots are
held in a separate collection from `Presence`.

**T4 — one bot's manoeuvre is one notification with one stamp.** A batch of
K bot actions at one tick produces one `DoDestinyUpdate` whose entries all
carry the same stamp, and `ActionUpdate::new` refuses a mixed batch. *Fails
if* a per-bot loop emits a notification each, which also burns K stamps.

**T5 — no unanswered call is introduced by a bot.** Run a session with bots in
space and assert the server's `unanswered` log is empty. *Fails if* a bot's
`ownerID`, `charID` or `corpID` reaches a service we do not implement — the
concrete prediction of §4.1 and the cheapest possible check of it.

**T6 — an NPC corp owner never reaches the server.** Send a ball with
`ownerID = 1000126` and assert **no** `GetMultiOwnersEx` arrives naming it.
*Fails if* §4.1's reading of `LoadEveOwners` + `_Prime` is wrong, which would
mean every NPC we ever spawn costs a round trip.

**T7 — being locked is visible only via `OnTarget`.** Lock a human from a bot
with no push and assert `stateSvc.GetState(bot_ship, threatTargetsMe)` is
falsy; send `OnTarget('otheradd', bot_ship)` and assert it becomes truthy.
*Fails if* the client infers locks from something else, which would make
exception 1 unnecessary.

**T8 — health bars need the pair form.** A bare-dict `extraBallData` yields
`UI_HIDDEN` gauges; the `(dict, damageState)` tuple yields three full bars.
(Already pinned by `destiny.rs`'s `a_full_health_ship_reads_as_three_full_bars`
and `a_bare_float_shield_is_a_hidden_bar_not_an_empty_one`; restated because a
bot spawner is the most likely place to regress it.)

**T9 — the archetype join is by exact string.** Every non-digit
`spaceComponentsByType.behavior.behaviorName` is either a key of
`ScriptedBehaviorLoader.behaviors_by_scope_by_id[Item]` or one of the three
known misses. *Fails if* a fourth miss appears, which would mean the loader
chain is not "FSD by int, Python by name".

**T10 — the NPC default colour is client-side.** With all standings tables
empty, a ball of a type whose `entityStandings.hostileResponseThreshold` is
`11.0` renders red. *Fails if* it renders white, which would mean the client
needs a standings row we are not sending.

**T11 — the `entity` bound object is never called in ordinary play.** A full
session — login, undock, fly, fight bots, dock — produces no call to
`Moniker('entity', solarsystemid2)`. *Fails if* some UI path we missed resolves
an NPC's behaviour, which would promote §1.5 from GM tool to requirement.

**T12 — a bot acting while all humans idle is still seen within 250 ms.**
Queue a bot action with no inbound payload and assert the frame reaches an idle
observer. **This one is expected to fail today** —
`docs/lanes/two-in-space.md` §1.3 — and that is the point: it is the
pass/fail boundary for "bots exist while nobody is playing", which is the end
goal's whole premise.

---

## 9. Open questions, ranked by what it costs to be wrong

1. **The ticker asymmetry (T12).** `Registry::pending` drains only on
   `Dispatch::on_payload`, so a server-driven entity that acts between inbound
   payloads is invisible until someone's client speaks. **Cost if wrong: the
   feature.** A game that can run for years with bots must act when nobody is
   looking. Cheapest resolution: make the push ticker drain `pending`, which is
   a coordinator-scale change to `push.rs`/`service.rs`, both hot files.
2. **Warp (`Mode::Warp` tail, `CmdAlignTo`, `CmdStargateJump`).** **[V2]**
   only. **Cost if wrong: bots cannot leave a grid**, so "roaming" and
   "patrol" archetypes, and every travel tree, are unreachable. Resolution is
   known and cheap: once a push path exists, put a ball in warp and let the
   client's own writer emit the bytes (`round2-spec.md` §8).
3. **`OnSpecialFX`'s 14 positionals and `OnTargets`' `[1:]`.** I read the
   signatures and the client's consumers, but **I never saw a real server frame
   for either.** The `[1:]` in particular means each element of `OnTargets` has
   a leading field whose meaning I did not establish. **Cost if wrong: a
   silently dropped combat event**, which is the class of bug this project has
   paid for three times. Resolution: one capture of TQ-shaped frames, or
   build it and read the client's log.
4. **Dogma server-side.** Operations 11–13 need a server dogma location with
   fitted modules, effect cycles and damage application. We have none. **Cost
   if wrong: nothing breaks, but combat does not exist** — for humans either.
   This is the largest missing subsystem the end goal implies and it is not a
   bot problem.
5. **`SetSafetyLevel(level)` is a write we ignore.** We answer `GetClientStates`
   with safetyLevel `0`. If a player toggles safety the client will call
   `CharGetCrimewatchLocation().SetSafetyLevel(n)`. **Cost if wrong: one
   unanswered call and a UI that snaps back**, low. Resolution: store it per
   character and echo it.
6. **`npcGroupingID`** (dungeon `entity` +44) points at a table that does not
   ship. **Cost if wrong: we misread CCP's authored encounters** when importing
   dungeon spawns. Low, because we author our own populations; `dungeon-model.md`
   §5.5 already flags it as the trap.
7. **`npcpilot.characterIds` decoding.** I confirmed the field exists on 48
   types and is a `dict<str,i32>`, but did **not** decode the dict's contents,
   so I cannot say whether the ids are per-type pools, per-faction, or indices.
   **Cost if wrong: low** — it only matters if we take the `npcpilot` route in
   §7.4 instead of the player-identity route.
8. **`behaviors/debugging/behaviortreemonitor.pyc` and
   `groups/`, `timers/`, `targetEvaluation.py`** I read only by name and
   signature, not line by line. The nine target evaluators in particular
   (`const/combat.py`'s `ALL_TARGET_EVALUATORS`) have implementations in
   `targetEvaluation.py` that a combat implementer will want. **Cost if wrong:
   low**, and the fix is one afternoon of reading.
9. **Whether `boot.role` matters anywhere we serve.** `idCheckers` branches on
   `boot.role == 'server'` in `IsEvePlayerCharacter` and `IsPlayerCorporation`
   **[V]**, and `eveCfg.EveOwners.__getattr__` branches on
   `boot.role != 'client'`. On a real client `boot.role == 'client'`, so these
   are dead there — but it is a reminder that shared files know about both
   sides and a copied reading can be the wrong side's. **Cost if wrong: a
   subtle misread of a shared file.** No action; recorded.

---

## 10. Scope note

Everything above was read from `out/pyc/` and from the client's shipped static
data, in the analysis container, read-only. The decompiled copy of the
`behaviors` package used for reading lives in this session's scratchpad and is
not in the tree. No Rust was written or compiled, no test was run, nothing was
committed, and the only files this lane wrote are this one and
`docs/lanes/npc-behaviors.md`.
