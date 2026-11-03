<!-- Provenance: produced by a read-only research lane on 2026-10-19 from this
     client's bytecode (V24.01, 3619879; `findings/push-events.tsv` re-anchored on 3662168 on
     2026-10-31, issue #19: 3,177 rows both, 14 rows moved) with tools/pyc27.py. No Rust was
     written, no server code changed, nothing was run against a client. The
     [V]/[I] marks are the house convention: [V] read from bytecode, [I]
     inferred and unsettled. The companion artifact is
     findings/push-events.tsv, which this document explains and prioritises. -->

# The push events this client already handles

`docs/reference/push-path.md` settles **how** to send a server-initiated `Notification`.
This document settles **what to put in it**: the set of notify handlers that
exist on this client, their signatures, and what each handler does to each
argument — read off the disassembly, not off decompiled source.

The reason this is the specification and not a survey: **every inventory,
fitting, market and industry mutator discards its return value** (`POP_TOP`).
The client learns what happened from a push or not at all. The receive side is
already wired — a service declares `__notifyevents__`, `sm.RegisterNotify`
files it under each name, and `sm.notify[name]` is what `BroadcastStuff`
dispatches into. So the handlers *are* the contract, and their parameter names
are the best documentation of a push's payload that will ever exist.

Contents: 1 how it was produced, 2 the columns, 3 the numbers, 4 what the
mechanism makes true, 5 the priority ordering, 6 the payloads we cannot fake
cheaply, 7 the inert events (safe probes), 8 limits and unknowns.

---

## 1. How it was produced

Everything below is `tools/pyc27.py` — this repo's own read-only Python-2.7
`.pyc` reader and 2.7 disassembler — driven from the host's `python3`
(`pyc27.py` is pure Python 3 and stdlib-only, so no container and no install is
needed for it; `uncompyle6` runs in the analysis container and was used only to
*check* results, never to produce them).

The reconciliation starts from the two directions the lane brief names, and the
disagreement between them is where the findings are:

```sh
# every declaration (binary files: -a, or grep calls it a match and prints nothing)
grep -ral "__notifyevents__" out/pyc/ | wc -l          # 953 files
grep -ral "RegisterForNotifyEvent" out/pyc/ | wc -l    # 37 files

# a single row can always be re-derived by hand, which is the property that matters:
tools/analysis.sh uncompyle6 out/pyc/eve/client/script/ui/shared/neocom/wallet/walletSvc.pyc
```

The generator was eight throwaway scripts in the lane's scratchpad
(`index.py`, `bases.py`, `extend.py`, `reg.py`, `emitters.py`, `shapes.py` +
`gen2.py`/`gen3.py`). They are not in the repo, per the lane rules, so the
algorithm is specified here in enough detail to rewrite them; each stage is a
single pass over the 12,774 `.pyc` files with `pyc27.load_pyc` + `pyc27.walk` +
`pyc27.dis`.

1. **Index every class body.** A 2.7 class body is the code object containing
   `LOAD_LOCALS` — unambiguous, and the reason no heuristic is needed. For each:
   its `__guid__` and `__notifyevents__` (recovered with a tiny abstract
   interpreter over the body's bytecode, so `['A'] + Base.__notifyevents__`
   yields its const part rather than nothing), and every method
   (`LOAD_CONST <code>; MAKE_FUNCTION n; STORE_NAME name`), which also gives
   each method's `firstlineno`, `argcount`, `varnames`, `co_flags` and its
   **number of defaults** (the `MAKE_FUNCTION` operand).
2. **Index base classes.** At each `BUILD_CLASS`, walk back past
   `MAKE_FUNCTION` and the class code object to the `BUILD_TUPLE n` that holds
   the bases, and record the n dotted names. Resolved by name, preferring a
   class in the same file.
3. **Catch declarations mutated in the class body** — `__notifyevents__ =
   Base.__notifyevents__` followed by `.add(...)`/`.extend([...])`/`+=`. Only 4
   classes do this, and skipping them would have produced **wrong** rows: it is
   how `InfoPanelLocationInfo` subscribes to nine events, including three that
   otherwise look like unregistered names.
4. **Index `sm.RegisterForNotifyEvent(ob, 'OnX')` sites** with a const name, and
   record whether `ob` is plain `self`. 54 of the 56 sites in the build register
   `self`; the two exceptions register a module object
   (`sys.modules[__name__]`, in `monolithhoneycomb*/__init__.py`). That is what
   makes rule 6c below sound.
5. **Index every local emission** — `sm.ScatterEvent('OnX', …)`,
   `SendEvent`, `ChainEvent`, `NotifySessionChange` and the
   `…WithoutTheStars` forms — with a const first argument. 829 distinct names.
   This is not used to include or exclude a row; it is reported per row as
   `local_emitters=N`, because an event the client also fires itself is one
   where a push is not the only source.
6. **Reconcile.** For every method whose name is a declared event name, the row
   is kept if the declaration can be tied to that class:
   (a) the class declares it itself → `notifyevents` (and `michelle` when the
   class is `Michelle`, i.e. the Destiny/ballpark path);
   (b) an ancestor **or a descendant** declares it → `notifyevents`, with the
   related class in `notes` (the descendant direction matters: a mixin can hold
   the handler while the subclass holds the declaration);
   (c) a `RegisterForNotifyEvent` site **in the same file** names it → `other`;
   (d) the class's declaration is computed and unresolvable → `scatter_only`.
   Otherwise the match is a **coincidence** and is dropped: 341 of them, e.g.
   all 128 `Update` methods in the tree, which collide with
   `devtools/svc_destroyableitems`'s `__notifyevents__ = [… 'Update' …]`.
   Dropping them is the difference between a table and a word cloud.
7. **Analyse the payload use** of each surviving handler (§2), and emit one row
   per handler plus one row per declared event that has **no** handler anywhere.
8. Sort by `(event, handler_file)`.

Sanity: 12 rows were checked line-by-line against `uncompyle6` output
(`walletSvc.OnAccountChange`, `invCache.OnItemChange`, `michelle.DoDestinyUpdate`,
`michelle.DoDestinyUpdates`, `michelle.OnDroneStateChange`,
`marketsvc.OnOwnOrdersChanged`, `marketbase.OnOwnOrdersChanged`,
`industrySvc.OnIndustryJob`, `industrySvc.OnAccountChange`,
`lpStoreSvc.OnAccountChange`, `turret.OnGodmaItemChange`,
`agents.OnAgentMissionChange`) plus four picked at random
(`infoPanelSkyhookTheft.OnLinkedWithShipSiloItemUpdated`,
`contentProviderAgents.OnNPCStandingChange`, `sessions.DoSimClockRebase`,
`environmentSvc.DoBallsAdded`). All sixteen matched.

---

## 2. The columns

| column | meaning |
|---|---|
| `event` | the broadcastID / notification name. |
| `handler_file` | the handler's own source file and line, taken from the **code object's embedded filename** with the `…/release/V24.01/` prefix stripped, and its `firstlineno`. So `packages/…` appears for packaged code, exactly as the client sees it. |
| `service` | the class that owns the method, with its `__guid__` in brackets when it has one. |
| `declared_in` | `notifyevents`, `michelle`, `other` (a same-file `RegisterForNotifyEvent`), `scatter_only` (no visible declaration), or `notifyevents (no handler)` for a declared event with no method. |
| `arity` | positional parameters excluding `self`. `3` is exact; `2..3` means one default, so both arities are legal; `+*args` / `+**kwargs` as present. **A `*args` handler cannot be got wrong on arity, and 492 of the 3,148 handler rows have one.** |
| `params` | the parameter names, in order. |
| `payload_shape` | one entry per parameter, `\|`-separated (§2.1). |
| `risk` | `;`-separated tags (§2.2), or `none`. |
| `notes` | `kind=svc\|ui`, `decl=N` (how many classes declare this event), `local_emitters=N`, the declaration route when inherited, `N default(s)`, `[stack-desync: shapes partial]` where the analysis degraded, and then the evidence: attribute names read off a parameter, constants it is compared against, dict keys, and the functions it is handed to. |

### 2.1 The `payload_shape` vocabulary

Read off the disassembly by a mini abstract interpreter that tracks which stack
slots hold which parameter. Several uses are joined with `+`, most constraining
first; the control flow is approximated by clearing the stack at every jump
target, so a desynchronised stack degrades to `unknown`, never to a confident
wrong answer.

| entry | what the bytecode showed | what it constrains |
|---|---|---|
| `unpack_N` | `UNPACK_SEQUENCE N` | exactly N elements, or `ValueError` |
| `subscript` / `as-key` | `BINARY_SUBSCR` with the parameter as base / as index | a sequence or mapping / a hashable scalar |
| `attr` | `LOAD_ATTR` on the parameter | an object with those fields (named in `notes`) |
| `iterate` | `GET_ITER` | iterable |
| `elem.attr`, `elem.unpack_N`, `elem[]` | the same, applied to the loop variable of `for x in param` | a sequence **of rows/objects**; the element fields are in `notes` |
| `contains` | `in` / `not in` with the parameter on the right | a container |
| `len` | `len(param)` | sized |
| `native` | handed to a `blue.*` / `destiny.*` / `trinity.*` / ballpark call | the one place list-vs-tuple is observable |
| `rowset:X` | handed to a rowset constructor | a rowset of that kind |
| `arith` | arithmetic on it | a number |
| `truth` | branched on, or compared (the constant, if any, is in `notes`) | a value test |
| `format` | only interpolated into a string | nothing |
| `pass` | handed to another Python function (named in `notes`) | nothing *here* |
| `store` | stashed on `self` and not inspected | nothing *here* |
| `discard` | **never loaded at all** — the parameter does not appear as a `LOAD_FAST` | nothing; send anything |
| `unknown` | it is loaded, but the interpreter lost the stack | unknown, and said so |

`discard` and `unknown` are deliberately different claims: `discard` is a
positive finding (the handler ignores the argument), `unknown` is an admission.

### 2.2 The `risk` tags

The handler-side half of `docs/reference/push-path.md` §5's four hazards, plus what the
dispatch discipline adds:

| tag | meaning |
|---|---|
| `native-payload` | **a parameter is handed to native code.** Seven rows (§6). This is the only place list-vs-tuple, int-vs-long and tuple-vs-`KeyVal` are observable in a way no Python caller could see. |
| `native-touch` | the handler calls into `blue`/`destiny`/`trinity`, but not with the payload. |
| `dbrow` | the payload is passed to `blue.DBRow`, so it **must** be a real `DBRow` (§6). |
| `lock` | the handler takes a lock (`uthread.Lock`, a semaphore). |
| `reentrant-call` | the handler makes a **remote** call — a push that makes the client call us back. Detected only for the direct forms (`sm.RemoteSvc`, `sm.ProxySvc`, `MachoBindObject`/`MachoResolveObject`); an indirect one through another service's method is not detectable and §8 says so. |
| `strict-arity` | the payload is `UNPACK_SEQUENCE`d, so its length is load-bearing. |
| `raises-on-shape` | the handler subscripts, iterates, `len()`s or reads attributes off the payload — a scalar or `None` there raises. |
| `raise-stmt` | the handler contains an explicit `raise`. |
| `do-sync-blocktrap` | a `Do…` event: dispatched by `sm.SendEventWithoutTheStars`, which runs handlers **synchronously with `stackless.getcurrent().block_trap = 1`**, so a handler that blocks raises (`docs/reference/push-path.md` §2.3). |
| `session-change-path` | `DoSessionChanging` / `ProcessSessionChange` / `OnSessionChanged`. The real server drives these from a session-change packet (command 16/18), not from a broadcast; `NotifySessionChangeWithoutTheStars` **raises** for any other event name. Reachable by a push, but it is not how the client expects to get them. |

---

## 3. The numbers

| | |
|---|---|
| `.pyc` files parsed | 12,774 (all of them; no parse failures) |
| class bodies examined | 18,029 |
| classes declaring `__notifyevents__` | 1,146 — 1,071 plain const lists, 70 built by concatenation, 5 computed (4 of which the §1.3 pass resolved) |
| distinct declared event names | **1,311** |
| …with no client-side local emitter, i.e. a push is the only source | **562** |
| handler rows | **3,148**, over **1,282** distinct events and 1,102 classes (224 of them services) |
| declared events with no handler anywhere | 29 rows — but 13 are artifacts of one client bug (§7) |
| rows in `findings/push-events.tsv` | **3,177** + header |
| coincidental name matches dropped | 341 (94 distinct names) |
| parameters analysed | 4,328 |
| …`unknown` | **5** (0.1%) — `OnKillRightUsed.toID`, `OnSpecialFX.repeat`, `OnSpecialFX.startTime`, `OnUITestAnimationSelected.animFunc`, `OnWeaponGroupDestroyed.itemID` |
| …`discard` (never loaded) | 1,518 (35%) |
| handlers where the stack desynchronised | 107 (3.4%), flagged per row |

Risk tallies over the 3,177 rows: `raises-on-shape` 695, `session-change-path`
287, `do-sync-blocktrap` 134, `native-touch` 100, `strict-arity` 16,
`reentrant-call` 11, `raise-stmt` 8, `lock` 8, **`native-payload` 7**,
**`dbrow` 1**, and 2,282 with nothing.

The ten events with the most handlers: `OnSessionChanged` 227,
`OnSessionReset` 63, `OnSkillsChanged` 44, `ProcessSessionChange` 41,
`DoBallRemove` 33, `DoBallsRemove` 31, **`OnItemChange` 30**,
`OnCharacterSessionChanged` 29, `OnStateChange` 28,
`ProcessActiveShipChanged` 27. Handler count is a cost: every one of them runs
in its own `uthread.worker` for an `On…` event, and all of them run
synchronously for a `Do…` one.

---

## 4. Five things the mechanism makes true

All **[V]**, all read this session, and each one changes what a push should
contain.

**4.1 A push only reaches an object that is registered *and* alive.**
`ObjectShouldReceiveMessages(obj)` is `not isinstance(obj, Service) or
obj.state == SERVICE_RUNNING` (`serviceManager.py:26`), so a handler on a
service that has not started is skipped — logged as `Skipping … (service not
running)` and nothing else. For UI classes the equivalent is that they call
`sm.RegisterNotify(self)` in `ApplyAttributes` and unregister on close: **a
window that is not open is not a handler.** The `kind=svc|ui` tag in `notes`
is there for exactly this reading.

**4.2 One frame can carry many events.** `BroadcastStuff.NotifyUp` has a
`__MultiEvent` branch:

```python
elif packet.destination.broadcastID == '__MultiEvent':
    for broadcast in packet.payload[1]:
        broadcastID, args = broadcast
        if broadcastID.startswith('Process'):   sm.ChainEventWithoutTheStars(broadcastID, args)
        elif broadcastID.startswith('Do'):      sm.SendEventWithoutTheStars(broadcastID, args)
        else:                                   sm.ScatterEventWithoutTheStars(broadcastID, args)
```

So `broadcastID = '__MultiEvent'` with a payload of `(name, args)` pairs
dispatches each by its own prefix rule. Each element is `UNPACK_SEQUENCE 2`d,
so a 1- or 3-element entry is a `ValueError` inside
`ExceptionWrapper.NotifyUp`'s bare `except` — one traceback, nothing worse.
Worth having for the day an item move produces four events at once.

**4.3 Do not send `__PacketBus`.** The other special branch *re-broadcasts*
through `self.ScattercastWithoutTheStars(…)`, i.e. it makes the client
**emit** notifications. It is the one broadcastID that turns a push into client
traffic, which is `docs/reference/push-path.md` §5's "a push that makes the client call us
back" in its purest form.

**4.4 An exception in a handler is logged, not fatal — but per handler.**
`ScatterEvent` runs each handler through `MollycoddledUthread`, which is
`try: func(*args, **keywords) except: LogError; LogException; sys.exc_clear()`.
`SendEvent` and `ChainEvent` wrap each call in the same shape. So a wrong
payload costs one traceback in `client-*.log` per registered handler, and the
*other* handlers still run — which means a push with a wrong shape can leave
the client in a **partially updated** state. That is worse than a clean failure
and is the real argument for getting shapes right rather than probing.

**4.5 `sm.notify` and `sm.notifyObs` are `defaultdict(list)`.** An unknown
broadcastID yields an empty iteration — `ScatterEvent` does not even log it
(`SendEvent` logs `Orphan'd event`). This is what makes §7 safe.

---

## 5. The priority ordering

Grouped by what each push unblocks, cheapest-first inside each group. "Cheap"
here means: no `DBRow`, no item store, no Destiny, no new service surface, and
every handler's shape pinned by §2.

### 5.0 Group 0 — the plumbing, with nothing at stake

One event, no handler, no payload: §7. This is `docs/reference/push-path.md` increment 1 and
it is worth doing first precisely because it cannot be partially wrong.

### 5.1 Group 1 — `OnAccountChange`-class state (the first real push)

Everything here is scalars. `OnAccountChange` is the one the brief names, and
the table confirms the choice: **5 handlers, 3 scalar parameters, risk `none`
on every row, no `DBRow`, no native, no lock, no remote call.**

| event | handlers | arity | params | what the shapes pin |
|---|---|---|---|---|
| `OnAccountChange` | 5 (4 svc, 1 window) | 3 | `accountKey, ownerID, balance` | `accountKey` is compared `== 'cash'` (`walletSvc`, `lpStoreSvc`, `securityOfficeWindow`) and `in (b'cash', b'cash2', b'cash3', b'cash4', …)`; `balance` is tested `is not None` and then arithmetic; `IndustryService` passes `accountKey` to `get_accounting_key_for_name` — so the **string** name, never the integer key |
| `OnShareChange` | 4 (2 in the wallet, 2 in corp services) | 3 | `shareholderID, corporationID, change` | `shareholderID` compared against `session.charid`; `change` is `discard`ed by three of the four and only `pass`ed on by the fourth |
| `OnBillReceived` | 1 (`walletSvc`) | `0+*args` | — | cannot be got wrong; a free "the wallet noticed" push |
| `OnLPChange` / `OnCorpLPChange` | 1 each | 4 / 2 | `_changeType, issuerCorpID, lpBefore, lpAfter` | all scalars, `pass`ed on |
| `OnCorpWalletBillsChanged` | 3 panels | 0 | — | zero-arity; a pure "refetch" signal |

Note what `OnAccountChange` already buys in *testing* value: `walletSvc.
_OnCharAccountChange` reads `self.wealth`, which issues a remote
`GetCashBalance` if `_wealth` is unset, so the first push also rehearses "the
client calls us back because of something we pushed" — for free, and under the
`On…` discipline where blocking is legal (§2.2).

### 5.2 Group 2 — before a mutator's effect is visible at all

This is the group the brief asks about, and the answer has a shape worth
stating plainly: **the inventory window is not a handler.** The push goes to
`invCache`, and `invCache` fans it out *inside the client*:

```python
# invCache.OnItemChange, tail, verbatim
            finally:
                sm.services['godma'].OnItemChange(item, change, location)
                sm.services['inv'].OnItemChange(item, change, location)
                sm.ScatterEvent('OnItemChanged', item, change, location)
```

and, per affected container, `invCacheContainer.OnItemChange` →
`_ProcessItemChange` → `sm.ScatterEvent('OnItemChangeProcessed', item, change)`.
That is why `Godma.OnItemChange`, `InventorySvc.OnItemChange`,
`DogmaLocation.OnItemChange` and `invCacheContainer.OnItemChange` are **not**
rows in the TSV: they are direct calls, not subscriptions. Push `OnItemChange`
once and all of them run.

In dependency order:

| event | handlers | arity | params | cost |
|---|---|---|---|---|
| `OnItemChange` | **30** | 3 | `item, change, location` | **needs a real `DBRow`** (§6) and an item store. The gate for the whole group. |
| `OnItemsChanged` | 3 | 3 | `items, change, location` | the batch form; `invCache` re-scatters `OnItemChange` per item and then `OnMultipleItemChange(items, change)`. Same `DBRow` cost, paid once per frame. |
| `OnMultipleItemChange` | 9 | 2 | `items, change` | 4 of the 9 do `iterate+elem.attr`; the rest `discard` both |
| `OnDogmaAttributeChanged` | 7 | 4 | `shipID, itemID, attributeID, value` | **four scalars, no `DBRow`, no native.** Moves `fittingController`, `shipUI`, `hangarView`, `stationSvc`. Every handler guards on identity or membership first, so it is *safe* to send before any of that state exists — it simply does nothing. |
| `OnCapacityChange` | 3 | 1 | `moduleID` / `itemID` | one scalar; `invWindow` refreshes the capacity gauge if it is the open container |
| `OnItemNameChange` | 7 | 6 of them `0+*args` | — | arity-proof |
| `OnGodmaItemChange` | 2 | 2 | `item, change` | `item.groupID` / `item.typeID` only — an object with fields, **not** necessarily a `DBRow` |
| `OnFittingStripped`, `OnFittingsUpdated`, `OnFittingDeleted` | 1–2 each | small | — | fitting-window housekeeping, scalars |
| `ProcessActiveShipChanged` | 27 | 2 | `shipID, oldShipID` | two scalars, but `Process…` is **chained synchronously** and 27 handlers deep |

The honest ordering inside the group: `OnDogmaAttributeChanged` first (it is
the only one that is both visible in the fitting window and free of `DBRow`),
then `OnCapacityChange`, then `OnItemChange` once the item store and the row
descriptor exist.

### 5.3 Group 3 — one scalar push per subsystem, each unblocking its window

All of these are pure scalars with `risk` `none`, i.e. implementable the day
the corresponding server state exists:

| event | handlers | arity | params |
|---|---|---|---|
| `OnIndustryJob` | 2 (both svc) | 6 | `jobID, ownerID, blueprintID, installerID, status, successfulRuns` |
| `OnOwnOrdersChanged` | 6 | 3 | `orders, reason, isCorp` — and `marketsvc` guards `if orders is not None`, so **`None` is a legal payload**: it invalidates the cached `GetCharOrders`/`GetOrders` and the window refetches. The cheapest market push there is. Elements, when sent, need `.typeID` and `.duration`. |
| `OnAgentMissionChange` | 7 | 2 | `actionID, agentID` (two scalars; one handler names them `missionState, agentID`) |
| `OnCorporationChanged` | 7 | 2 | `corpID, change` — five of the seven `discard` at least one argument; `change` is read as a mapping by two |
| `OnContactChange`, `OnContactLoggedOn/Off` | 12 / 2 each | 2 / 1 | scalars; most handlers `discard` both |
| `OnSkillsChanged` | 44 | mostly `0+*args` | the widest arity-proof event in the build |

### 5.4 Group 4 — Destiny, i.e. step E and nothing sooner

`DoDestinyUpdate`, `DoDestinyUpdates`, `DoBallsAdded`, `DoBallRemove`,
`DoBallsRemove`, `OnFleetStateChange`, `OnDroneStateChange`,
`OnDroneActivityChange`, `DoSimClockRebase`. Every one of them is
`do-sync-blocktrap` *and* most are `native-payload` or `native-touch`; see §6.
`docs/archive/destiny-and-the-ballpark.md` owns this group.

### 5.5 What not to push

`OnSessionChanged` (227 handlers), `ProcessSessionChange` (41) and
`DoSessionChanging` (19) are reachable as broadcasts but belong to the
session-change packet path; `NotifySessionChangeWithoutTheStars` raises for any
other name, which is a hint about how narrow that door is. `OnSessionReset`
(63 handlers) *clears* caches — `walletSvc.ProcessSessionReset` drops
`_wealth` — so it proves a push by making the client ask again, which
`docs/reference/push-path.md` §3.1 already ranks last.

---

## 6. The payloads we cannot fake cheaply

**Seven rows hand a parameter straight to native code** (`native-payload`), and
they are the whole list:

| event | handler | shape |
|---|---|---|
| `DoDestinyUpdate` | `eve/client/script/remote/michelle.py:625` | `state` → `self.__bp.FlushState`, plus `subscript [0]` and `iterate`; `waitForBubble` → the same native call |
| `OnDroneStateChange` | `michelle.py:386` | **all seven** parameters → `self.__bp.OnDroneStateChange(...)` |
| `OnDroneActivityChange` | `michelle.py:391` | all three → `self.__bp.OnDroneActivityChange(...)` |
| `OnFleetStateChange` | `michelle.py:381` | the single parameter → the ballpark |
| `OnItemChange` | `eve/client/script/environment/invCache.py:250` | `item` → `blue.DBRow(item)` |
| `OnRemoveCalendarEvent` | `eve/client/script/ui/services/eveCalendarsvc.py:496` | one parameter to a `blue.*` call |
| `OnStateChange` | `eve/client/script/parklife/bracketMgr.py:987` | one parameter to a `blue.*` call |

These are the rows where a Python-invisible mistake is visible: a native method
type-checks like C, which is exactly how
`michelle.py(453) LoadFormations(formations)` produced
`TypeError: must be tuple, not list` (`docs/archive/destiny-and-the-ballpark.md` §1).

**`OnItemChange`'s `item` must be a real `blue.DBRow`, and the descriptor is
pinned too.** Two independent places construct from it —
`invCache.OnItemChange` (`oldItem = blue.DBRow(item)`) and
`invCacheContainer._ProcessItemChange` (the same line) — and both then do:

```python
for k, v in change.iteritems():
    if k in (const.ixStackSize, const.ixSingleton):
        k = const.ixQuantity
    oldItem[k] = v
```

`oldItem[k]` with `k` an **integer** means `change` is keyed by column index
and the row's columns must be in that order. The indices are const **[V]**:
`carbon/common/lib/const.py` gives `ixItemID 0, ixTypeID 1, ixOwnerID 2,
ixLocationID 3, ixFlag 4, ixQuantity 5`, and
`packages/inventorycommon/const.py` gives `ixGroupID 6, ixCategoryID 7,
ixCustomInfo 8, ixStackSize 9, ixSingleton 10`.
The handlers then read `item.itemID`, `item.typeID`, `item.ownerID`,
`item.locationID`, `item.flagID`, `item.quantity`, `item.stacksize`,
`item.groupID`, `item.categoryID` by name, so the eleven columns need those
names in those positions. `docs/reference/dbrow.md` has the wire layout (tag `0x2A`, a
`blue.DBRowDescriptor` built from `(name, dbtype)` tuples); this is the
descriptor it needs.

**No handler in the build applies a rowset constructor to a pushed argument**
— `rowset:` never fires. What does happen is the softer version of it: 112
parameters are `iterate`d, and 83 carry an `elem.*` entry (16 of them
`elem.attr`), i.e. a sequence whose *elements* are read by attribute
(`orders` → `.typeID`, `.duration`; `items` → `.itemID`, `.typeID`). Those
elements need to be objects with fields, which `KeyVal` satisfies; only the
`blue.DBRow` sites above need the packed-row machinery. And a caveat worth
stating: **a handler that only reads attributes cannot tell us whether the real
server sent a `DBRow`, a `Row` or a `KeyVal`** — where it matters, it is
because something *else* in the handler pins it, and §8 says where we are blind.

---

## 7. The inert events — the safe probe

`sm.notify` and `sm.notifyObs` are `defaultdict(list)`, so a broadcastID that
nothing has registered for is dispatched into an empty list and returns. That
makes an unregistered name the one push that **cannot** be partially wrong, and
`docs/reference/push-path.md` §6 correction 2 is right that the plumbing should be proved by
sending *something*.

There are two grades of "no handler", and conflating them would be a mistake:

**Grade A — nothing is registered under the name. Silent, guaranteed.** The
name appears in no `__notifyevents__` (including the computed ones, §1.3) and in
no `RegisterForNotifyEvent`. Best candidates, with the evidence that each really
has no handler:

| name | evidence |
|---|---|
| `OnEveOfflinePushProbe` | `grep -ra "OnEveOfflinePushProbe" out/pyc/` returns **nothing**: the string does not exist in the build. Nothing can declare it, nothing can collide with it, and finding it in a capture identifies it instantly. This is `docs/reference/push-path.md` §4's `--push-probe` name and it remains the best one. |
| `OnItemLocked` | the string occurs **once in the whole tree**, as `sm.ScatterEvent('OnItemLocked', itemID)` at `invCache.py:333`. No class declares it; no class has a method of that name. The client already fires it into the void every time it locks an item. |
| `OnItemUnlocked` | identically: one occurrence, `sm.ScatterEvent('OnItemUnlocked', itemID)` at `invCache.py:380`. |
| `OnAddBallpark` | one occurrence, `sm.ScatterEvent('OnAddBallpark')` in `michelle.AddBallpark` (line 458 of the real source). Zero-arity, and already fired on every undock with nobody listening. |
| `OnTargetsArranged` | one occurrence, in `parklife/targetMgr.py`, scattered, undeclared. |
| `OnWindowStartBlinking` / `OnWindowStopBlinking` | one occurrence each, in `carbonui/control/window.py`. |

That is not a short list by accident: **67 event-name-shaped strings are
scattered by the client today with no listener at all**, and 202 undeclared
names have no method of that name anywhere in the build. The scan is in the
lane's scratchpad (`inert.json`); the six above were each re-checked by hand
with `grep -ral`.

For a probe, prefer `OnEveOfflinePushProbe` (unambiguous in a capture, and
`On…` so `ScatterEventWithoutTheStars` does not `LogError` about the prefix —
it does that for anything not starting with `On`, which is a reason never to
name a diagnostic event `Probe…`).

**Grade B — a registrant exists but has no method of that name. Not silent.**
These are the 29 `(none)` rows in the TSV. `ScatterEvent` does
`getattr(srv, eventid)` *inside* its `try`, so the push costs one
`AttributeError` traceback per live registrant. Harmless, but it is not a clean
probe, and sending one would put a misleading traceback in `client-*.log`.
Examples: `OnCollapsed` (declared by `MyFleetPanel`, which has no such method),
`OnHackingNewTurn`, `OnSkyhookCanBeHacked`.

Two client bugs fall out of Grade B, both **[V]**, and both are the reason the
29 is not 16:

- **`shipTreeShipIcon.ShipTreeShipIcon.__notifyevents__ = 'OnShipTreeShipFocused'`**
  — a bare string, not a list. `sm.RegisterNotify` does
  `for notify in ob.__notifyevents__`, so the icon registers itself under
  **21 single-character event names** (`'O'`, `'n'`, `'S'`, …). Its real
  `OnShipTreeShipFocused` method is never reachable, and the
  `sm.ScatterEvent('OnShipTreeShipFocused', …)` in the same file finds nobody.
  Thirteen of the 29 handler-less rows are these characters.
- **Two missing commas.** `killRights.KillRightsEntry` declares
  `'OnClientContactChangeOnBlockContacts'` and `overviewWindow.OverviewWindow`
  declares `'OnOverviewOverrideChangedOnDoBallAddEgoChange'` — Python implicit
  string concatenation, baked into the bytecode, so neither class is subscribed
  to either of the four events it meant to name.

Neither bug matters to us except as evidence that the declaration side must be
read mechanically rather than by eye.

---

## 8. Limits, and where this is blind

Stated so that a wrong row can be found rather than trusted:

1. **5 parameters are `unknown`** and say so. 107 handlers (3.4%) hit a stack
   desynchronisation and carry `[stack-desync: shapes partial]`; their shapes
   are a lower bound — more uses may exist than are listed. The four random
   spot-checks included two desynced handlers and both were still correct.
2. **Provenance is lost through a function call.** `enumerate(updates)` is why
   `michelle.DoDestinyUpdates`'s parameter reads `len+pass` when the source
   actually does `for idx, args in enumerate(updates)` and then
   `len(args) == 2` → `state, waitForBubble = args`. So for
   `DoDestinyUpdates` the real shape is **a sequence of 2- or 3-tuples**, and
   the TSV understates it. Python-2 list comprehensions *are* inlined, so
   `elem.*` entries from comprehensions are real; generator expressions and
   nested `def`s are separate code objects and are not followed.
3. **`reentrant-call` is a floor, not a ceiling.** Only direct remote forms are
   detected. `stationSvc.OnDogmaAttributeChanged` calls `LoadItem(shipID)`,
   which can reach the server through another service — invisible to this
   analysis. Treat any handler that calls into `dogma`, `invCache`, `godma` or
   `cfg` as potentially re-entrant.
4. **`local_emitters` is a floor too**: only `ScatterEvent('OnX', …)` with a
   *constant* first argument is counted, so events scattered through a variable
   or a `const.*` name are missed. It is reported, never used to filter.
5. **341 coincidental name matches were dropped** (§1.6). The judgement is
   mechanical and stated; `dropped.json` in the lane's scratchpad has all of
   them, and the three largest groups were checked by hand —
   `spacecomponents`' 37 `OnSlimItemUpdated` methods, for instance, belong to
   that package's **own** message bus (`MSG_ON_SLIM_ITEM_UPDATED =
   'OnSlimItemUpdated'` in `packages/spacecomponents/client/messages.py`), not
   to `sm.notify`.
6. **Runtime registration is not static.** `other` rows subscribe at runtime
   (`MultiFitWnd` and `CreateContractWnd` register for `OnItemChange` while
   open), and §4.1's running/open gate applies to every row. The table says who
   *can* receive a push, never who *will*.
7. **Nothing here has been run against a client.** Every row is bytecode. The
   first push that goes out will test §5.1 and nothing else.
