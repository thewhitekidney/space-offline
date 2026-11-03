<!-- Provenance: produced by the read-only `item-spawn` research lane on
     2026-10-19/20 from this client's bytecode (V24.01, 3619879) and from the
     client's own static data. No Rust was written, no server code changed,
     nothing was run against a client. [V] = read from bytecode or from the
     client's own data files; [I] = inferred and unsettled. Every line number
     below came from `tools/call_lines.py` / `tools/pyc27.py` (i.e. from
     `co_lnotab`), never from decompiler output -- see
     `docs/guides/debugging-the-client.md`, "Tooling traps". The companion artifact
     is `findings/spawn-types.tsv`. -->

# Spawning items: the spec `/spawn` is implemented from

The owner's report was *"assets seem to work? i want more ships or items to
test"*. This document is what the item store and the GM `/spawn <type>
[quantity]` command have to satisfy. It answers seven questions; §8 is the
order to build them in.

The one-paragraph summary, for a reader who stops here:

> The hangar is **one** server call -- `invbroker::inventory.List(flag=4)` on
> the `(containerHangar, None)` inventory -- and the client splits it into
> Ship Hangar and Item Hangar itself, on `categoryID` read off *our* row.
> `groupID` and `categoryID` are therefore load-bearing server data that the
> client never cross-checks against its own FSD tables (§4), so a `/spawn`
> needs real ids and `findings/spawn-types.tsv` has twenty-five of them.
> **The hangar listing latches for the session** (§2): without a push, a
> spawned item is invisible until the next session change. And our item
> descriptor is **one column short** in a way that already breaks the
> "undock in which ship?" picker (§3).

Contents: 1 the enumeration path, 2 push or re-ask, 3 the `OnItemChange`
contract and the eleventh column, 4 do our ids have to agree with FSD,
5 the type table, 6 stacking and singleton, 7 flags, 8 implementation order.

---

## 1. The enumeration path

### 1.1 What the station inventory window actually calls

The UI chain is `invCont.StationItems` / `invCont.StationShips`
(`eve/client/script/ui/shared/inventory/invContainers.py`) wrapping a
controller in `eve/client/script/environment/invControllers.py`. The
controller is where the calls come from **[V]**:

```text
BaseInvContainer.GetItems         invControllers.py:124
    -> filter(self.IsItemHere, self._GetItems())
BaseInvContainer._GetItems        invControllers.py:135
    -> self._GetInvCacheContainer().List(flag=self.locationFlag)
StationItems._GetInvCacheContainer   invControllers.py:1337
StationShips._GetInvCacheContainer   invControllers.py:1460
    -> sm.GetService('invCache').GetInventory(const.containerHangar)
```

`invCache.GetInventory` (`invCache.py:553`) routes `containerHangar` (10004)
to `self.stationInventoryMgr` and mints an `invCacheContainer` keyed
`(10004, None)`. `invCacheContainer.List` (`invCache.py:1201`) is then the
only thing that talks to us.

In wire terms, one hangar draw is:

| # | call | what it is for | does a new item have to be in it? |
|---|---|---|---|
| 1 | `invbroker.GetItemDescriptor` | the row descriptor, cached as `invCache.__itemhdr` (`invCache.py:482`) | n/a -- but see §3, it must be **11 columns** |
| 2 | `invbroker.GetInventoryFromId` / `GetInventory` | mints the bound inventory object | no |
| 3 | `invbroker::inventory.GetSelfInvItem` | describes the **container** (the station), not its contents | no |
| 4 | **`invbroker::inventory.List(flag=4)`** | **the hangar listing** | **yes -- this is the one** |
| 5 | `charMgr.ListIncludingContainers` | assets window, cross-station search (`invCache.py:955`) | yes |
| 6 | `charMgr.ListStations` | assets window's station index (`invCache.py:904`) | the station must be a row, or §1.3 |
| 7 | `charMgr.ListStationItems(stationID)` | assets window, one station's contents (`invCache.py:914`) | yes -- **and we do not answer it today** |
| 8 | `invbroker.GetContainerContents(id, ...)` | what is *inside* a ship or container | only if `locationID == that container` |

**One listing feeds both hangars.** `StationItems.IsItemHere`
(`invControllers.py:1331`) and `StationShips.IsItemHere`
(`invControllers.py:1445`) differ in exactly one clause **[V]**:

```python
# StationItems.IsItemHere          invControllers.py:1331
item.locationID == session.stationid and item.ownerID == session.charid \
    and item.flagID == const.flagHangar and item.categoryID != const.categoryShip
# StationShips.IsItemHere          invControllers.py:1445
...                                and item.categoryID == const.categoryShip
```

so there is no second server call for the Ship Hangar, and `categoryID` is
the whole partition. `invCache._GetShipsInHangarInventory`
(`invCache.py:514`) and `_GetItemsInHangarInventory` (`invCache.py:521`) are
the same split again, over `idCheckers.IsShip(item.categoryID)`, and
`hangarView.GetShipItemFromHangar` (`hangarView.py:716`) -- the function that
decides whether a ship model is drawn -- is a third copy of it.

### 1.2 Irrelevant to a new hangar item, and worth not chasing

- `invbroker::inventory.GetSelfInvItem`, `GetAvailableTurretSlots`,
  `QALockItem` / `QAUnlockItem` -- about the container or about GM menus.
- `invbroker::inventory.GetCapacity` -- **there is no such call.**
  `invCacheContainer.GetCapacity` (`invCache.py:1311`) is computed entirely
  client-side from `evetypes.GetCapacity(typeID)` plus
  `GetItemVolume(each)` summed over `self.List(flag)`. The one thing it needs
  from us is the *container's* row: `invCache.py:1312` short-circuits
  `if self.item.groupID in (const.groupStation,)` to a capacity of
  9e15. `Registry::station_self_item` already sends `groupID == 15`, so the
  item hangar reads as effectively infinite. **Keep it that way** -- lose it
  and the hangar starts rejecting drops.
- `invbroker.GetContainerContents` -- for a loose hangar item,
  `locationID == stationID`, so `contents_of()` correctly filters it out of
  every container query. It matters the first time `/spawn` puts something
  *in* a ship.
- `dogmaIM` / `godma` -- see §4.3; a hangar item with `flagHangar` is
  deliberately ignored by godma.

### 1.3 `charMgr.ListStationItems` is an open hole **[V]**

`assetsWindow.GetSubContent` (`assetsWindow.py:939`) does

```python
items = sm.GetService('invCache').GetInventory(const.containerGlobal) \
            .ListStationItems(data.location.locationID)
self.PrimeLocationNames(items)
for each in items: ...
```

so `None` is a `TypeError`. `inv5MinCacheContainer.ListStationItems`
(`invCache.py:914`) stores the answer as `(now, <answer>)` behind a five
minute stamp, so our `None` **is stored successfully** -- this is the
`contractProxy` poisoned-cache variant from
`docs/guides/debugging-the-client.md` §4, except that the five-minute TTL bounds it.
**The re-ask is paced by the player expanding a station node in the assets
window, and at most once per five minutes per station**, so it is a
traceback, not a live lock. It belongs in the same change as the other two
`charMgr` listings, which `service/character.rs`'s `ListStations` arm already
says. The honest answer is the same `owned_item_rows()` filtered to
`locationID == stationID` (and note its sibling `ListStations` is
*deliberately* empty today, so the window will not reach this call until that
changes).

---

## 2. Does a spawn need a push?

**Yes. Reopening the window does not re-ask.** This is the answer that
decides whether `/spawn` is useful before `OnItemChange` lands, and it is
"only just".

Three caches sit between us and the window, and they fail differently:

| cache | where | what fills it | what empties it |
|---|---|---|---|
| `invCacheContainer.cachedItems` + `.listed` | `invCache.py:990` | `List(flag)` at `invCache.py:1201`, which adds `flag` to `self.listed` and only calls us `if flag not in self.listed` | **nothing, for the session** -- only `invCache.InvalidateCache` (`:634`), `InvalidateLocationCache` (`:660`), `InvalidateCorpCache`, `RemoveInventoryContainer`, or the `OnInvalidateCacheForLocations` notify (`:680`) |
| `inv5MinCacheContainer.cachedItemsAll` + `.listedAll` | `invCache.py:872` | `ListIncludingContainers` (`:955`) | a tasklet, `_DeleteCachedItemsAll` (`:974`), after `const.MIN * 5 / 10000` ms -- `const.SEC` is 10 000 000 (100 ns units, `carbon/common/lib/const.py`) and `MIN = SEC * 60`, so that is five minutes. `cachedStations` and `cachedStationItems` carry their own five-minute stamps |
| `godma.StateManager.invitems` | `godma.py:1238` | `UpdateItem` (`godma.py:1858`) | `Release` (`godma.py:1303`) and `ProcessSessionChange` (`godma.py:1381`) |

The window's own lifecycle touches none of them: `_InvContBase`
(`container.py:349`) subscribes to exactly `OnPostCfgDataChanged` and
`OnInvTempFilterChanged`, and closing it drops UI objects, not cache entries.
`GetItems()` on reopen goes to `_GetItems()` -> `List(flag=4)` -> `flag in
self.listed` -> `cachedItems.values()`. **No `CallReq` is emitted. [V]**

`invCache.InvalidateCache` runs from `ProcessSessionChange`
(`invCache.py:162`) only when `_ShouldInvalidateCache` is true, i.e. when
`stationid`, `structureid` or `solarsystemid` is in the change
(`invCache.py:202`), and from `OnSessionReset` (`invCache.py:208`). So
without a push the only ways to see a spawned item are **dock/undock, a
session reset, or a client restart.**

### 2.1 The cheap push, and why it is not quite enough

`OnInvalidateCacheForLocations(locationIDs)` **is a real notify event on
`invCache`** -- `findings/push-events.tsv` has it, declared in
`invCache.__notifyevents__`, handler at `invCache.py:680`, arity 1, the
argument merely iterated. It takes **a list of integers and nothing else**:
no `blue.DBRow`, no descriptor, no `change` dict. Pushing
`OnInvalidateCacheForLocations([10004])` deletes the
`(containerHangar, None)` entry from `invCache.inventories`, so the *next*
`GetItems()` re-asks us and sees the new item.

That makes it the cheapest possible "the hangar changed" signal and a
genuinely useful first push **[V]** -- but it does not *redraw*: nothing in
`invCont`/`invWindow` subscribes to it, so the player still has to reopen or
switch the view. Treat it as the stepping stone, not the destination: it
proves the push plumbing with a payload we can already emit, and it makes
`/spawn` usable ("spawn, then reopen the window") a long time before the
`DBRow` work lands.

The real push is `OnItemChange`, and `InventorySvc.OnItemChange`
(`inventorysvc.py:87`) is what makes the open window redraw: it walks its
registered inventory containers and calls `ob.AddItem(item)`
(`inventorysvc.py:142`) for any `ob` where the item `isHere` now and was not
before.

---

## 3. `OnItemChange(item, change, location)` -- the exact contract

### 3.1 The claim in `push-events.md` §6 is right, and under-sold

`docs/reference/push-events.md` §6 says `item` must be a real `blue.DBRow` and that
eleven positional indices are read. Both hold, and the indices are **[V]**
from the two const modules:

| index | column | const | declared at |
|---|---|---|---|
| 0 | `itemID` | `ixItemID` | `carbon/common/lib/const.py:440` |
| 1 | `typeID` | `ixTypeID` | `carbon/common/lib/const.py:441` |
| 2 | `ownerID` | `ixOwnerID` | `carbon/common/lib/const.py:442` |
| 3 | `locationID` | `ixLocationID` | `carbon/common/lib/const.py:443` |
| 4 | `flagID` | `ixFlag` | `carbon/common/lib/const.py:444` |
| 5 | `quantity` | `ixQuantity` | `carbon/common/lib/const.py:445` |
| 6 | `groupID` | `ixGroupID` | `inventorycommon/const.py:3484` |
| 7 | `categoryID` | `ixCategoryID` | `inventorycommon/const.py:3485` |
| 8 | **`customInfo`** | `ixCustomInfo` | `inventorycommon/const.py:3486` |
| 9 | `stacksize` | `ixStackSize` | `inventorycommon/const.py:3487` |
| 10 | `singleton` | `ixSingleton` | `inventorycommon/const.py:3488` |

**`item_columns()` is missing index 8, `customInfo`**, which shifts
`stacksize` to 9 and `singleton` to 10-out-of-range. Four independent reads
pin the eleven-slot layout, and only one of them is a push:

1. **`undockingSvc.ShipPicker`** -- the "undock in which ship?" dialog --
   iterates `hangarInv.List(const.flagHangar)` and subscripts the rows
   **positionally**: `item[const.ixCategoryID]` and `item[const.ixSingleton]`
   on one line (`undockingSvc.py:195`), then `item[const.ixTypeID]` and
   `item[const.ixItemID]` (`undockingSvc.py:197`). `ixSingleton` is 10.
   With today's ten-column descriptor the capsule passes the `categoryShip`
   test and the `and` then evaluates `item[10]`. **[I]** that this raises
   `IndexError` (the native `DBRow.__getitem__` was not read); **[V]** that
   index 10 is subscripted. `station/base.SelectShipDlg` does the same pair
   at `base.py:626` and `base.py:627`.
2. **`cfg.IsChargeCompatible`** (`eveCfg.py:407`) is
   `if not item[const.ixSingleton]: return 0` (`:408`) then
   `return item[const.ixGroupID] in self.__chargecompatiblegroups__`
   (`:411`) -- index 10 and index 6, positionally, off a module's row. It is
   what decides whether a fitted weapon offers an ammo slot.
3. **`InventorySvc.OnItemChange`** builds a log dict *before* any remapping:
   `fancyChange[item.__columns__[k]] = (v, '->', item[k])`
   (`inventorysvc.py:93`). Both subscripts take the **raw** `change` key, so
   a `change` carrying `ixStackSize` (9) or `ixSingleton` (10) indexes the
   descriptor's own column list at that index. This runs unconditionally --
   the dict is built whether or not logging is on.
4. **`godma.sublocrd`** (`godma.py:180`) derives a second descriptor from
   ours by iterating `list(self.itemrd)` and **skipping every column with
   `dbtype == 0 and size == 0`** (`godma.py:183`), then copying
   `self.itemrd.virtual` onto the result (`godma.py:191`).
   `godma.subloc_internalrd` (`godma.py:198`) skips the same columns plus the
   two it names by string: `'quantity'` and **`'customInfo'`**
   (`godma.py:204`). A descriptor that had no `customInfo` column and no
   `DBTYPE_EMPTY` columns would make both filters dead code.

### 3.2 CCP's descriptor is nine physical columns plus two virtual ones

Five sites in `godma.py` construct `blue.DBRow(rd, [...])` against
`self.itemrd` or `self.sublocrd` with a hard-coded **nine**-element list:
`OnGodmaPrimeItem` (`godma.py:506`), `UpdateItem` (`:1900`, `:1910`),
`GetAttribute` (`:2245`), `OnDroneStateChange2` (`:2656`),
`ApplyAttributeChange` (`:3061`). The order is
`itemID, typeID, ownerID, locationID, flag, quantity, groupID, categoryID,
<ninth>`, and the ninth value is `None` at three of the sites. Combine that
with §3.1.4 and the real descriptor is **[I], but strongly**:

```text
0..7  physical   itemID, typeID, ownerID, locationID, flagID, quantity, groupID, categoryID
8     physical   customInfo
9     virtual    stacksize       DBTYPE_EMPTY (0), size 0
10    virtual    singleton       DBTYPE_EMPTY (0), size 0
```

and the two legacy helpers that compute them survive in the client:
`eveCfg.StackSize` (`eveCfg.py:1387`) is `item[ixQuantity]`, or 1 if it is
negative (`:1394`); `eveCfg.Singleton` (`eveCfg.py:1397`) is
`-item[ixQuantity]` when quantity is negative (`:1404`), else 1 if the
`locationID` is in the solar-system id range (`:1406`), else 0. That is the
pre-2008 encoding in which a negative `quantity` *was* the singleton flag.

**We cannot emit a virtual column today.** `docs/reference/dbrow.md` §8 gap 4 is
exactly this ("EMPTY/virtual columns -- simplest safe rule: never declare
one"), the descriptor constructor's format string is `"si|O:DBRowDescriptor"`
(`docs/reference/dbrow.md` §2) whose optional third element is unexplained, and `__reduce__`
re-emits only the column tuple, so how `.virtual` is populated is **unknown**.

**So declare eleven *physical* columns.** `stacksize` and `singleton` become
real `I4` columns at 9 and 10. Then positional reads at 6, 7, 9 and 10 are
right, attribute reads (`item.stacksize`, `item.singleton`) are right, and
`len(item) == len(res.header)` -- which `invCacheContainer.List` tests at
`invCache.py:1238` before logging `'Invalid Item'` and dropping the row at
`:1239` -- holds, because the header it compares against is *our* answer to
`GetItemDescriptor`.

The cost is the one `service.rs` already records: a client-side
`blue.DBRow(itemrd, [nine values])` does not match an eleven-column
descriptor. That cost **already exists** at ten columns, so moving to eleven
adds no new class of failure; it closes two real bugs and leaves one open.
Whether the native constructor pads or raises is **unknown**, and none of the
five sites is on the docked path: they need a `OnGodmaPrimeItem`
notification, or a sublocation, i.e. a charge loaded into a fitted module.

`customInfo`'s type is **[I]**. CCP's is string-ish --
`neocomInventoryBadging.has_custom_info_attribute` (`:315`) does
`item.customInfo and hasattr(item.customInfo, 'get') and
item.customInfo.get(name, ...)` and `is_item_from_my_ship` (`:349`) iterates
it inside a bare `except Exception`. **Declaring it `I4` and sending `0` is
safe**: `0` is falsy so the first guard short-circuits, and iterating `0`
raises inside the handler's own `try`. That avoids `docs/reference/dbrow.md`'s class-5
object-slot path entirely. Revisit if anything ever needs a real value.

### 3.3 The `change` dict

`change` is **keyed by column index**, and its keys are consumed three ways:
`oldItem[k] = v` in `invCache.OnItemChange` (`invCache.py:266`-`269`) and in
`invCacheContainer._ProcessItemChange` (`:1436`-`1439`), and
`item.__columns__[k]` / `item[k]` in `InventorySvc.OnItemChange`
(`inventorysvc.py:93`). Both `invCache` loops rewrite `ixStackSize` and
`ixSingleton` to `ixQuantity` *before* the subscript (`:268`, `:1438`), so
those two indices are never written there -- but `inventorysvc.py:93` sees
them raw.

The semantics: **`change[k]` is the value the column had *before* the
change.** `oldItem = blue.DBRow(item)` then `oldItem[k] = v` reconstructs the
previous row, and `item` is the new one.

| key | what the client does with it |
|---|---|
| `ixItemID` 0 | a stack merge. `neocomInventoryBadging.is_seen_item_id_change` guards `if ixItemID not in change: return False` and then reads `change[ixItemID]` (`:304`, `:311`) as the *old* item id. Omit it. |
| `ixTypeID` 1 | nothing tests it. **[I]** never sent. |
| `ixOwnerID` 2 | `invCache.OnItemChange` logs `REMOVING ... ownership changed` and, with `ixLocationID` also present, calls `containerGlobal.TrashItem` (`invCache.py:312`-`326`); `_ProcessItemChange` uses it for the corp-hangar owner branch (`:1469`); `invWindow.OnInvChangeAny` reads `change.get(ixOwnerID)`. |
| `ixLocationID` 3 | the move key. Decides `keyWas` (`invCache.py:324`); gates `godma.StateManager.OnItemChange` (`godma.py:1447`); makes `invWindow.OnInvChangeAny` refresh the tree. `change[ixLocationID] == session.shipid` is how "came from my ship" is detected (`neocomInventoryBadging.py:345`). |
| `ixFlag` 4 | forces the second container fan-out (`if keyIs != keyWas or const.ixFlag in change`, `invCache.py:306`); makes `inventorysvc` scatter `OnModuleUnfitted` / `OnHighSlotFittingChanged` (`inventorysvc.py:154`-`158`); drops `godma.chargeTypeByModule`. |
| `ixQuantity` 5 | `invCache.OnItemChange` **raises `RuntimeError`** if `change[ixQuantity] < 0` or `item.quantity < 0` (`invCache.py:252`-`254`), after an unconditional `log.LogTraceback`. |
| `ixGroupID` 6, `ixCategoryID` 7, `ixCustomInfo` 8 | no consumer anywhere in the build tests these as `change` keys (`ixCustomInfo` appears in **no** file but `const.py`). **[I]** never sent. |
| `ixStackSize` 9 | remapped to `ixQuantity` before the write; separately, `item.stacksize == 0` means *gone* -- see §6. |
| `ixSingleton` 10 | remapped to `ixQuantity`; `inventorysvc` first rewrites a value of `0` to `1` (`inventorysvc.py:98`); `invWindow.OnInvChangeAny` refreshes the whole tree when it is present (`invWindow.py:1537`). |

**For a spawn, send `change = {ixLocationID: 0}`.** The reasoning:
`InventorySvc.OnItemChange` computes
`wasHere = old.stacksize != 0 and ob.invController.IsItemHere(old)` and
`isHere = item.stacksize != 0 and ob.invController.IsItemHere(item)`
(`inventorysvc.py:126`-`130`) and only calls `ob.AddItem(item)` when
`not wasHere and isHere` (`:142`). A spawn has no previous state, so the
push has to *fabricate* a previous location that fails `IsItemHere`, and
`locationID` is the field `IsItemHere` tests first. `0` does that while
staying an ordinary integer; godma itself uses `{ixLocationID: None, ixFlag:
None}` for the same purpose (`godma.py:492`), but `None` would exercise the
**[?]** setter side of `docs/reference/dbrow.md`'s NULL bitmap (gap 3) through
`oldItem[3] = None`, and there is no reason to find out the hard way.

`location` is the third parameter and it is **not** the item's location.
`invCacheContainer._ProcessItemChange` sets its own `location = self.key[0]`
(`invCache.py:1435`ff) and ignores the argument; `invCache.OnItemChange`
passes it straight through to `godma`, `inv` and the `OnItemChanged` scatter
(`invCache.py:296`-`299`); `godma.StateManager.OnItemChange` takes it and
never reads it. Every handler in `findings/push-events.tsv` marks it
`pass` or `discard`. **`None` is the right value [V]**, and it is what
godma's own emission sends.

---

## 4. Must our `groupID`/`categoryID` agree with the client's static data?

**This is the most valuable answer in the brief, so here it is first:
the client reads `groupID` and `categoryID` off our row, does not
re-derive them from `typeID`, and never cross-checks. A contradictory row
does not raise and is not ignored -- it is *believed*, for everything that
reads the row, while everything that goes through `typeID` keeps telling the
truth. The failure mode is divergence, and it is silent.**

### 4.1 The client has its own type table, and it is not ours

`evetypes` (`out/pyc/evetypes/__init__.pyc`) resolves `GetName`, `GetGroupID`,
`GetCategoryID`, `GetVolume`, `GetCapacity` and the rest from
`res:/staticdata/types.fsdbinary`, `groups.fsdbinary` and
`categories.fsdbinary` via `fsdBuiltData.common.base.BuiltDataLoader`
(`evetypes/data.py`) **[V]**. `cfg.invtypes` **does not exist in this build**
-- the string appears in zero files under `out/pyc/`. So the question is not
"does the client have its own data" (it does) but "does it ever compare".

### 4.2 It does not. Both derivations live in one function

`godma.StateManager.UpdateItem` (`godma.py:1858`) is the clearest case,
because it uses *both* sources four lines apart **[V]**:

```python
if isSubLocation:
    itemGroupID    = evetypes.GetGroupID(itemTypeID)        # FSD
    itemCategoryID = evetypes.GetCategoryID(itemTypeID)     # FSD
else:
    itemTypeID, itemGroupID, itemCategoryID, itemFlag = \
        (item.typeID, item.groupID, item.categoryID, item.flagID)   # godma.py:1869 -- OUR ROW
```

For an ordinary item the row wins, and what it wins is the gate that decides
whether the item exists for dogma at all: `itemCategoryID not in (<eleven
categories>) and itemGroupID != const.groupCharacter: return`
(`godma.py:1877`-`1885`). A ship we labelled `categoryModule` would be
*accepted* by that filter and then fail `IsShipFittingFlag` checks; a ship we
labelled `categoryCelestial` would be dropped with a
`'Godma location received unexpected and unwanted item'` traceback in the
client's own log and no attributes, forever.

Every other consumer is a row read too:

| what breaks | reads | at |
|---|---|---|
| Ship Hangar vs Item Hangar split | `item.categoryID` | `invControllers.py:1331`, `:1445` |
| whether a ship model is drawn | `item.categoryID` | `hangarView.py:725` |
| `ReplaceExistingShipModelModular` | `item.categoryID` | `hangarView.py:1032` |
| "undock in which ship?" | `item[ixCategoryID]` | `undockingSvc.py:195`, `base.py:626` |
| fittable-module filter | `i.categoryID` | `invCache.py:1300` |
| does this weapon take ammo | `item[ixGroupID]` | `eveCfg.py:411` |
| is this a cargo container (open-on-click) | `item.groupID` | `eveCfg.py:429`, `invCache.py:1307` |
| container capacity shortcuts | `self.item.groupID` | `invCache.py:1312`, `:1360`, `:1372`, `:1396` |
| inventory tree node kind | `item.groupID`, `item.categoryID` | `inventorysvc.py:414`, `:415` |
| close-the-loot-window heuristic | `item.groupID` | `inventorysvc.py:80` |
| tree refresh after a move | `item.groupID`, `item.categoryID` | `invWindow.py:1550` (`OnInvChangeAny`, firstlineno 1523) |
| Neocom badge "is a capsule change" | `item.groupID` | `neocomInventoryBadging.py:298` |
| godma location bookkeeping | `item.groupID == groupCapsule` | `godma.py:1519` |

And what stays truthful from FSD, no matter what we send: the **name**
(`undockingSvc.py:188` is `evetypes.GetName(item[const.ixTypeID])`), the
icon, the volume (`inventorycommon/util.py:45` `GetItemVolume` ->
`evetypes.GetVolume(item.typeID)`), the cargo capacity
(`invCache.py` `GetCapacity` -> `evetypes.GetCapacity(typeID)`), the dogma
attributes, and the 3D model.

So a lie produces a hangar entry named "Rifter", drawn with a Rifter icon,
with a Rifter's volume -- filed under the wrong hangar tab, with the wrong
context menu, and either fittable or not depending on which half of the code
you ask. **Nothing raises.** That is the worst available outcome and it is
why `/spawn` needs a real type table rather than an invented one.

### 4.3 The corollary: godma ignores hangar items on purpose

Worth stating so nobody "fixes" it. `godma.StateManager.OnItemChange`
(`godma.py:1447`) early-returns for an item whose flag is not a ship
fitting flag and not `flagPilot`, when `ixLocationID` is in the change. A
spawned hangar item (`flagHangar`) therefore never enters `godma.invitems`
from our push, and does not need to: the hangar list, the model and the icon
all come from `invCache`. Dogma matters when the item is *fitted*.

---

## 5. The type table -- `findings/spawn-types.tsv`

### 5.1 There is no readable static-data table; saying so plainly

- `bin64/staticdata/` contains **one** file, `mapObjects.db`. There is no
  types, groups or categories table in sqlite anywhere in the install.
- The real tables are `res:/staticdata/types.fsdbinary` (9.17 MB),
  `groups.fsdbinary` (69 KB) and `categories.fsdbinary` (2.3 KB), present in
  the resource cache (`CCP/EVE/ResFiles/<ab>/<hash>`, located through
  `tq/resfileindex.txt`).
- They are read by **native per-table loaders** -- `typesLoader.pyd`,
  `groupsLoader.pyd`, `categoriesLoader.pyd` in `bin64/` -- so the schema is
  compiled into a DLL, not described in Python. `uncompyle6` cannot even
  decompile `fsdBuiltData/common/base.pyc`.
- **Type names are not in the binaries at all.** `categories.fsdbinary`
  contains exactly one printable ASCII run in 2,392 bytes. Names are
  message ids resolved through
  `res:/localizationfsd/localization_fsd_en-us.pickle`, a 40 MB Python
  pickle of `{messageID: (text, ?, ?)}` which loads cleanly under a
  `find_class`-refusing `pickle.Unpickler`.

### 5.2 How the TSV was produced, and why its numbers are [V]

Rather than guess, the lane calibrated the record layout against ground
truth the *client's own Python* provides, and then checked it against ground
truth that was not used for calibration.

Record detection: a type record is an offset `o` where
`u32(o) == u32(o+128)` (the key is stored twice). Fields, little-endian:

| offset | field | evidence |
|---|---|---|
| `+8` | `basePrice` (f64) | Merlin 400 000, Apocalypse 153 900 000, Iron Charge S 200 |
| `+16` | `capacity` (f64) | Merlin 150, Apocalypse 845 |
| `+32` | `mass` (f64) | Capsule 32 000, Merlin 997 000 |
| `+48` | `radius` (f64) | Tritanium 1, Capsule 2, Merlin 39 |
| `+56` | **`volume`** (f64) | Tritanium 0.01, small charge 0.0025, every module 5, frigate 16 500 - 29 500, battleship 450 000 - 470 000 -- each strictly larger than its group's packaged override (§6.2) |
| `+72` | `factionID` | Merlin 500001 (Caldari), Apocalypse 500003 (Amarr) |
| `+84` | **`groupID`** | calibrated, below |
| `+68` | `descriptionID` | resolves to the in-game description text |
| `+132` | **`typeNameID`** | resolves to exactly the expected name, 6/6 |

A group record is an offset `o` where `u32(o) == u32(o+8)`, with
`categoryID` at `+4` and `groupNameID` at `+12`.

**Calibration** used fourteen `(typeID, groupID)` pairs assembled from
`inventorycommon/const.py` (e.g. `typeCapsule 670` at `:1477` with
`groupCapsule 29` at `:512`). `+84` was the only offset that explained all
fourteen with exactly one hit each.

**Independent confirmation** -- constants *not* used to calibrate, every one
of which came out right:

| decoded | agrees with |
|---|---|
| 21857 -> group 46 | `type1MNCivilianAfterburner` `:1414`, `groupPropulsionModule 46` `:374` |
| 21853 -> group 62 | `typeCivilianArmorRepairer` `:1509`, `groupArmorRepairUnit 62` `:383` |
| 215, 216 -> group 85 | `typeIronChargeS`/`typeTungstenChargeS` `:1651`,`:1901`, `groupHybridAmmo 85` `:681` |
| 483 -> group 54 | `typeMiningLaser` `:1695`, `groupMiningLaser 54` `:732` |
| 16233 -> group 419 | `typeProphecy` `:3587`, `groupBattlecruiser 419` `:492` |
| 588, 596, 601, 606 -> group 237 | the four racial corvettes `:3570`-`:3580`, `groupRookieship 237` `:916` |
| group 15 -> cat 3, "Station" | `groupStation 15` `:971`, `categoryStation 3` `:359` |
| group 18 -> cat 4, "Mineral" | `groupMineral 18` `:729`, `categoryMaterial 4` `:345` |
| group 85 -> cat 8, "Hybrid Charge" | `groupHybridAmmo 85` `:681`, `categoryCharge 8` `:337` |
| group 54 -> cat 7, "Mining Laser" | `groupMiningLaser 54` `:732`, `categoryModule 7` `:346` |

1,593 groups and 50,756 type candidates decode (the type scan is a
superset -- the `u32(o)==u32(o+128)` signature has false positives, so only
rows whose `groupID` resolves and whose `nameID` is in the localization table
were kept, and only the 25 curated rows were hand-checked).

The `source` column in the TSV is `fsd` for the decode plus
`const:<lines>` naming the `inventorycommon/const.py` lines that
independently agree, type line first, group line second.

### 5.3 What is in it, and what is not

Twenty-five rows: four frigates (one per race), two destroyers, three
cruisers, two combat battlecruisers, three battleships, three haulers, one
corvette, five modules, one ammo, one mineral. Nothing is `unknown`.

Two honest caveats:

- **`volume` is informational.** The server never sends a volume; the client
  computes it from `typeID` (`inventorycommon/util.py:45`). The column is the
  FSD *assembled* volume, which is what `GetItemVolume` uses for a singleton
  item; for a packaged one the client substitutes the group override (§6.2).
  Getting it wrong in the TSV costs nothing; getting `groupID` wrong costs
  §4.
- **the `+56` volume and the other f64 field names are [I]**, calibrated by
  consistency rather than read from a schema. `groupID`, `categoryID` and
  the names are **[V]** by the agreement table above.

---

## 6. Stacking and singleton

### 6.1 What each field means to the client

- **`stacksize == 0` means the item is gone.** Everywhere.
  `invCacheContainer._ProcessItemChange` deletes it from `cachedItems`
  (`invCache.py:1473`, `:1494`, `:1519`); `invCache.OnItemChange` logs
  `REMOVING: ... new stacksize is 0` (`:320`-`321`);
  `InventorySvc.OnItemChange` computes `wasHere`/`isHere` as
  `old.stacksize != 0 and ...` (`inventorysvc.py:126`-`130`);
  `assetsWindow` skips `each.stacksize == 0` (`assetsWindow.py:950`).
  **`/spawn` must never emit `stacksize == 0`.**
- **`quantity < 0` raises.** `invCache.OnItemChange` lines 252-254 raise
  `RuntimeError` when `change[ixQuantity] < 0` or `item.quantity < 0`, and
  `eveCfg.StackSize`/`Singleton` reinterpret a negative quantity as the
  legacy singleton encoding (§3.2). Keep `quantity >= 1`.
- **`singleton` truthy means "assembled".** It is the gate on
  `godma.UpdateItem`'s skip3: `if (isSubLocation or not item.singleton) and
  (itemCategoryID != const.categoryCharge or itemGroupID ==
  const.groupMine): return` (`godma.py:1939`). For a ship the right-hand
  side is true, so **a packaged ship gets no dogma attributes** -- which is
  correct EVE behaviour, not a bug. It is also what `ShipPicker` filters on
  (`undockingSvc.py:195`), so a packaged ship cannot be boarded.
- **charges are the exception written into that same line.** For
  `categoryCharge` the right-hand side is false, so a `singleton == 0` stack
  of ammo *does* get attributes. Ammo is the one thing designed to arrive as
  a non-singleton stack.
- `GetItemVolume` (`inventorycommon/util.py:45`) branches on `item.singleton`
  and then multiplies by `item.stacksize`.

### 6.2 The combinations, and which `/spawn` should refuse

| category | `singleton` | `stacksize` | verdict |
|---|---|---|---|
| ship (6) | 1 | 1 | **the normal case.** Appears in the Ship Hangar, boardable, has attributes |
| ship (6) | 0 | n | legal (a packaged ship), appears in the Ship Hangar, **not** boardable, no dogma. Allow, but say so |
| ship (6) | 1 | n > 1 | **refuse.** `GetItemVolume` returns n x the *assembled* volume, and one `itemID` cannot be n assembled hulls. Nothing raises; the capacity gauge and the stack label simply lie |
| module (7) | 0 | n | **the normal case** for an item-hangar module |
| module (7) | 1 | 1 | legal (an assembled/fitted module); needed if `/spawn` ever fits something |
| module (7) | 1 | n > 1 | **refuse**, same reason as a ship stack |
| charge (8) | 0 | n | **the normal case.** The exception in skip3 is written for exactly this |
| charge (8) | 1 | 1 | legal but pointless; `cfg.IsChargeCompatible` is about the *weapon*, not the round, so nothing improves |
| material (4) | 0 | n | **the normal case** |
| anything | any | 0 | **refuse** -- §6.1, the client reads it as "removed" |
| anything | any | negative `quantity` | **refuse** -- raises in `invCache.OnItemChange` |

`findings/spawn-types.tsv`'s `stackable` column is exactly this rule:
`singleton` for the eighteen ships, `stackable` for the seven others.

A note on the *packaged* volume path, because it is the only place the two
flags change a number the player sees: `GetPackagedVolume`
(`inventorycommon/util.py:33`) consults
`packagedVolumeOverridesPerType` then `packagedVolumeOverridesPerGroup`
(`inventorycommon/const.py:3051`-`3317` and `:2998`-`3047`) before falling
back to `evetypes.GetVolume`. The ship-group overrides are **[V]**:
frigate 2 500, destroyer 5 000, cruiser 10 000, battlecruiser 15 000,
battleship 50 000, hauler/industrial 20 000, capsule 500, shuttle 500,
corvette 2 500, mining barge 3 750.

---

## 7. Flags

**Confirming the capsule row's doc comment and extending it: there is no
separate Ship Hangar flag.** Both station hangars are `flagHangar == 4`
(`inventorycommon/const.py:115`), and the split is `categoryID`:

- `invCtrl.StationItems.locationFlag = const.flagHangar` and
  `invCtrl.StationShips.locationFlag = const.flagHangar` -- the same constant
  on both classes (`invControllers.py:1318` and `:1432` class bodies), so
  `_GetItems` issues the same `List(flag=4)`.
- the only difference is `categoryID != categoryShip` vs `== categoryShip`
  (`invControllers.py:1331`, `:1445`).

So **a module sitting loose in the station item hangar gets `flagID == 4`,
the same flag as a ship.** `/spawn` has one flag for everything it drops in
the hangar.

`flagShipHangar == 90` (`const.py:162`) is a *ship's* maintenance bay, not a
station's -- it appears in `panelFitting.GetShipContents` alongside
`flagCargo` when deciding which dockable to query, and in
`invCacheContainer.GetCapacity`'s `attributeShipMaintenanceBayCapacity`
branch (`invCache.py:1350`). It is not a station flag and must not be
used for one.

The flags `/spawn` can legitimately produce, all **[V]** from
`inventorycommon/const.py`:

| flag | value | line | use |
|---|---|---|---|
| `flagNone` | 0 | 111 | the character row's own flag; also what a station's self-item uses |
| `flagHangar` | 4 | 115 | **everything `/spawn` drops in a station** -- ships and items alike |
| `flagCargo` | 5 | 116 | inside a ship's cargo hold. `godma.UpdateItem` **skips** these (`godma.py:1890`, with `flagDroneBay` and the fighter tubes) |
| `flagCapsule` | 56 | 149 | the character inside the pod |
| `flagPilot` | 57 | 150 | applied client-side by `_OnCharacterEmbarkation`; never ours to send |
| `flagHiSlot0..7` | 27-34 | 138-145 | fitted high slots |
| `flagMedSlot0..7` | 19-26 | 130-137 | fitted mid slots |
| `flagLoSlot1..7` | 12-18 | 123-129 | fitted low slots |
| `flagRigSlot0..7` | 92-99 | 164-171 | rigs |
| `flagDroneBay` | 87 | 159 | drone bay; also skipped by godma |
| `flagShipHangar` | 90 | 162 | a ship's maintenance bay -- **not** the station's |

Two consequences for `/spawn`:

1. `flagID` is declared `I2` in `item_columns()`, and the largest value in the
   whole `flag*` family is `flagHangarAll` 1000 (a UI pseudo-flag; the largest
   real storage flag is `flagExpeditionHold` 188), so `I2` is fine **[V]**.
2. Anything with a fitting flag has to satisfy `godma.UpdateItem`'s skip2 --
   `IsShipFittingFlag(itemFlag) and not isSubLocation and itemCategoryID not
   in (categoryModule, categoryStructureModule, categoryCharge,
   categorySubSystem)` logs `Godma:UpdateItem.skip2` and returns -- so
   fitting a *ship* into a slot is silently dropped. `/spawn <ship>` into the
   hangar never touches this; `/fit` would.

---

## 8. Implementation order

Each step is independently shippable and each one closes named calls.

1. **Widen the item descriptor to eleven physical columns.**
   `itemID I8, typeID I4, ownerID I4, locationID I8, flagID I2, quantity I4,
   groupID I4, categoryID I4, customInfo I4, stacksize I4, singleton I4`.
   Nothing else changes; the three existing row builders gain one `0`.
   *Closes:* `undockingSvc.ShipPicker` and `station.base.SelectShipDlg`,
   which today evaluate `item[10]` on a ten-column row (§3.1), and makes
   `cfg.IsChargeCompatible` readable. *Needs:* nothing new in
   `eve-marshal`. *Do this first because it is the only step that fixes a
   bug we already have.*

2. **An item store, plus `/spawn <type> [quantity]` writing into it.**
   `Registry::owned_item_rows()` becomes the store's projection instead of a
   single hard-coded capsule; `station_assets_rowset`'s `itemCount` already
   derives from it. Validate against §6.2 and reject rather than render
   something broken. Load the type table from
   `findings/spawn-types.tsv`.
   *Closes:* `invbroker::inventory.List` and
   `charMgr.ListIncludingContainers` for real data. `/spawn` is now useful
   with one caveat: the player must dock/undock or restart to see it (§2).

3. **`charMgr.ListStationItems(stationID)`** -- `owned_item_rows()` filtered
   to `locationID == stationID` -- together with making
   `station_assets_rowset` non-empty, which its own comment says belongs in
   the same change. *Closes:* the assets window's station drill-down, and the
   `TypeError` at `assetsWindow.py:939` (§1.3).

4. **Push `OnInvalidateCacheForLocations([10004])` after a spawn.**
   A list of one integer -- no `DBRow`, no descriptor, no `change` dict -- so
   it is implementable the day the push path works. Drops the
   `(containerHangar, None)` container so the next window build re-asks us.
   *Closes:* "spawn, then reopen the window", i.e. the caveat in step 2.
   It does **not** redraw an already-open window.

5. **Push `OnItemChange(item, change, None)`** with `item` a real
   `blue.DBRow` over the eleven-column descriptor and
   `change = {const.ixLocationID: 0}`.
   *Closes:* live insertion into an open window, via
   `InventorySvc.OnItemChange` -> `ob.AddItem(item)`
   (`inventorysvc.py:142`), plus the Neocom "unseen item" badge, plus
   `invCache`'s own `cachedItems` so the assets window agrees. This is the
   step that makes `/spawn` feel like a server.

6. **Only then: fitted items.** `flagHiSlot*`/`flagMedSlot*`/`flagLoSlot*`
   rows, `invbroker.GetContainerContents` returning them (the filter in
   `service/inventory.rs` is already written), and `dogmaIM.ItemGetInfo` for
   each. This is where godma, sublocations and the nine-value
   `blue.DBRow(sublocrd, ...)` constructions start to matter, and where
   `docs/reference/dbrow.md` gap 4 (virtual columns) should be revisited.

### What is still unknown

- Whether a `blue.DBRowDescriptor` can declare a `DBTYPE_EMPTY` virtual
  column over the wire, and how `.virtual` is populated. `docs/reference/dbrow.md` §8 gap 4.
  Until it is answered, the eleven-physical-column layout is a deliberate
  deviation from CCP's nine-plus-two, and the five nine-value
  `blue.DBRow(itemrd, ...)` sites in `godma.py` are the exposure.
- Whether the native `blue.DBRow(descriptor, values)` constructor pads or
  raises when `values` is shorter than the descriptor. Decides how loud the
  item above is.
- Whether `oldItem[k] = None` is accepted by the native setter
  (`docs/reference/dbrow.md` §8 gap 3, the NULL bitmap setter side). Avoided, not resolved,
  by using `{ixLocationID: 0}`.
- `customInfo`'s real DBTYPE. `I4`/`0` is safe for every consumer found
  (§3.2), but it is not what CCP sends.
