# The market browse tree: why it is empty, and what the server can do

Written by the `market-tree` research lane after the owner tested the market
fix for tracker issue #2 and reported: *"still nothing there, it shows up when
i search though"*. The order list no longer throws, search finds types, the
browse tree is blank.

**Every client line number in this file is decoded from `co_lnotab` with
`tools/call_lines.py` / `tools/pyc27.py`, never read off uncompyle6's output**
(`docs/guides/debugging-the-client.md` §4, "Tooling traps"). Where a statement is
described without a line number, it was read from the decompiler and no number
is claimed. See §7 for a worked example of why that rule exists, found in this
round.

Claims are marked **[V]** read from the code, **[I]** inferred.

---

## 1. The answer

**The previous lane was right, and the symptom the owner reported is the one
the client is designed to produce.** Not a second bug.

**[V]** `MarketBase.SetupFilters` (`marketbase.py`, `firstlineno` 3073) opens
with

```text
3074  settings.user.ui.Set('showonlyavailable',
                           settings.user.ui.Get('showonlyavailable', True))
```

and the disassembly of line 3074 is `LOAD_CONST 'showonlyavailable'`,
`... LOAD_ATTR Get`, `LOAD_CONST 'showonlyavailable'`, **`LOAD_GLOBAL True`**,
`CALL_FUNCTION`, `CALL_FUNCTION`, `POP_TOP` — a read-with-default-`True`
immediately written back, i.e. "default on, and persist the default". It is
reached from `Startup` at `marketbase.py:649` **[V]**, so it runs once per
market-window construction.

**[V]** `MarketBase.FilterItemsForBrowse` (`firstlineno` 2667) is then

```text
2681  filterNone = settings.user.ui.Get('showonlyavailable', 0)
2682  ret = []
2683  for typeID in marketGroupInfo.types:
2684      ask = self.asksForMyRange.get(typeID, None)
2685      if (not filterNone or ask) and self.ShouldShowType(typeID):
2686          ret.append(typeID)
2687      elif typeID == typePlex and is_global_plex_market_experiment_enabled():
2688          ret.append(typeID)
2689  return ret
```

so with `filterNone` truthy a type survives only if `asksForMyRange` has an
entry for it.

**[V]** And the group itself disappears, which is why the tree is *empty*
rather than full of empty folders. `GetGroupListForBrowse` (`firstlineno` 965),
in its top-level arm:

```text
1022  grouplist = sm.GetService('marketutils').GetMarketGroups()[marketGroupID]
1023  for marketGroupInfo in grouplist:
1024      if not len(marketGroupInfo.types):
1025          continue
...
1030      typeIDs = self.FilterItemsForBrowse(marketGroupInfo)
1031      if len(typeIDs) == 0:
1032          continue
```

Every group filters to zero, every group is skipped, the scroll gets an empty
`contentList`.

**[V]** `asksForMyRange` is ours and it is empty. `PopulateAsksForMyRange`
(`firstlineno` 878) stores whatever `_GetAsksForRange` returns (896, 901), and
`_GetAsksForRange` (`firstlineno` 903) is:

```text
904  quote = sm.StartService('marketQuote')
905  marketRange = self.GetRange()
906  if marketRange == const.rangeStation:
907      asksForMyRange = quote.GetStationAsks()
908  elif marketRange == const.rangeSolarSystem or idCheckers.IsWormholeRegion(session.regionid):
909      asksForMyRange = quote.GetSystemAsks()
910  else:
911      asksForMyRange = quote.GetRegionBest()
912  return asksForMyRange
```

and all three are answered `{}` by
`server/crates/eve-server/src/service/market.rs` today.

**[V]** Which of the three is live is settled: `MarketUtils.GetMarketRange`
(`quote.py`, `firstlineno` 81) is

```text
84   r = settings.user.ui.Get('marketRangeFilterStation', rangeRegion)   # docked
86   r = settings.user.ui.Get('marketRangeFilterSpace',  rangeRegion)    # in space
```

— default `rangeRegion` in both arms, and `marketRangeFilterStation` /
`marketRangeFilterSpace` are **absent** from the owner's own settings file
(§3), so the browse tree is fed by **`marketProxy.GetRegionBest`**.

### Direct confirmation, off the owner's disk

Not inference. The setting is already written, with the value `True`, in the
owner's own profile:

```
~/.local/share/Steam/steamapps/compatdata/8500/pfx/drive_c/users/steamuser/
  AppData/Local/CCP/EVE/c_ccp_eve_tq-offline_tranquility/settings/core_user_1.dat
```

That file is `blue.marshal`, and in EVE marshal a dict entry is written
**value first, then key** (`docs/archive/marshal-tags.md`, tag `0x16`), with `0x1F` =
`True` and `0x20` = `False`. The byte immediately before each key string is
therefore the stored value, and it reads:

| key in `core_user_1.dat` | byte before the key | value | `SetupFilters` default |
|---|---|---|---|
| `showonlyavailable` | `1f` | **True** | `True` (3074) |
| `showonlyskillsfor` | `20` | False | `False` (3075) |
| `autorefresh` | `1f` | True | `True` |
| `market_filter_zerosec` | `1f` | True | `True` |

Four for four against `SetupFilters`' own defaults. **[V]** The filter is on,
it was turned on by the client itself, and it has already been persisted.

### The workaround, in one sentence

**In the Market window, click the small slider ("tune") icon immediately to
the right of the search box in the left-hand panel and untick the first
checkbox, "Show only available" — the tree fills immediately and the choice
persists.**

It persists because `OnCheckboxChange` writes through
`settings.user.ui.Set(...)` and then calls `LoadMarketListOrSearch()` in the
same frame, so there is no relaunch and no window reopen needed.

---

## 2. Where that control is, exactly

**[V]** The icon: `MarketBase.Startup` builds it at `marketbase.py:505` as

```text
UtilMenu(parent=filterParent, align=uiconst.CENTER, GetUtilMenu=self.SettingMenu,
         texturePath=eveicon.tune, width=24, height=24, iconSize=16,
         uniqueUiName=pConst.UNIQUE_NAME_MARKET_BROWSE_SETTINGS)
```

`filterParent` is a `ContainerAutoSize` aligned `TORIGHT, idx=0` inside
`searchAndSettingParent`, the 24-px-tall strip at the top of the market
window's left panel that also holds the search field and a collapse-all
button — so the icon is the leftmost of the two small buttons to the right of
the search box.

**[V]** The menu: `MarketBase.SettingMenu`, whose **first** entry is at
`marketbase.py:700`:

```text
menuParent.AddCheckBox(text=localization.GetByLabel('UI/Market/ShowOnlyAvailable'),
                       checked=bool(settings.user.ui.Get('showonlyavailable', 1)),
                       callback=(self.OnCheckboxChange, 'showonlyavailable'))
```

The localisation **key** is `UI/Market/ShowOnlyAvailable` **[V]**; the English
string is "Show only available" **[I]** — no localisation table is extracted
into `out/`, so that is the live client's wording from memory, not evidence.
The same key is used for the filter icon's hint in `FiltersInUse1`
(`marketbase.py:2861`) **[V]**, so if the owner hovers the icon the hint will
name the filter that is on.

Below it in the same menu, for context: a **Range** radio group
(Station / Solar System / Region, `SettingMenu`'s next block), then three more
checkboxes — `showonlyskillsfor`, `showhavecpuandpower`, `shownewskills`, all
defaulting to `False` at `marketbase.py:3075-3077` **[V]**, so none of them is
contributing — then a reset button wired to `ResetFilterSettings`.

**Do not press the reset button.** **[V]** `ResetFilterSettings` sets
`showonlyavailable` back to **`True`** at `marketbase.py:2154`
(`LOAD_CONST 'showonlyavailable'; LOAD_GLOBAL True`). It clears the three skill
filters to `False` and re-arms this one.

### The key, and where it lives

| | |
|---|---|
| setting key | `showonlyavailable` **[V]** |
| section / group | `settings.user.ui` → section `user`, group `ui` **[V]** (`USER_GROUPS` at `settingsSvc.py:30`, created at `:137`) |
| file template | `settings:/core_%s_%s.%s` % (section, `session.userid`, ext) — `settingsSvc.py:153` **[V]** |
| our file | `.../AppData/Local/CCP/EVE/c_ccp_eve_tq-offline_tranquility/settings/core_user_1.dat` **[V]**, observed; the directory key follows `EVE_CLIENT=.../drive_c/CCP/EVE/tq-offline` from `tools/up.sh:88` |
| format | `blue.marshal.Save(self.datastore)` → `blue.AtomicFileWrite`, `settings.py:241` **[V]**. Binary EVE marshal, same format our wire encoder emits |
| datastore shape | `{group: {key: (wallclock, value)}}` — `SettingSection.Set` **[V]** |
| flush | an `AutoTimer(62 s)` created in `LoadFromFile` at `settings.py:125` **[V]**, plus `ProcessShutdown` |

**Persistence across a relaunch: yes [V]**, with one caveat worth saying out
loud — the write is on a 62-second timer and on clean shutdown, so a client
killed within a minute of the click can lose it. The evidence that it does
persist is the table in §1: those four keys are in the file *because* the
client wrote them in an earlier session.

### Could we ship a default?

Two places. Neither is worth doing, and the first is the only one that is
unambiguously in bounds.

1. **Write the key into the player's own `core_user_1.dat`.** That file is
   generated by the player's client on the player's machine; it contains no
   CCP content, so writing `('ui', 'showonlyavailable') -> (t, False)` into it
   is not shipping anything of the client's. **[I]** It is in bounds under the
   owner's rule. Mechanically it needs our marshal *encoder* pointed at a
   local file, the client not running, and a merge rather than a rewrite
   (the file holds ~60 other keys, including the quickbar). Against one click,
   that is a poor trade.

2. **Patch the registered default in the installed client.** **[V]**
   `SettingSection.Get` falls back to
   `GetAttrs(defaultsetting, self._name, groupName, n)` at
   `carbonui/util/settings.py:150`, and `carbonui/util/defaultsetting.py:33`
   is `user = SafeBundle(ui=SafeBundle(language=0))` — so adding
   `showonlyavailable=False` to that bundle registers a client-wide default.
   **[I]** It would also win over the `True` literal at `marketbase.py:3074`,
   because `SettingSection.Get` returns the registered default and
   `Group.Get` only substitutes its own `defValue` when the result `is None`
   — so `SetupFilters` would read `False` and persist `False`.
   This is **modifying a file inside the client install**: in bounds only as a
   patch to the player's own copy, in the same category as
   `tools/client-patch-manifest.py` and `tools/client-prepare.py`, and it
   would have to be described that way and never redistributed. It is also a
   `.pyc` edit inside the script archive, which is a heavier mechanism than we
   have today — `tools/client-overrides/` is a *drop a script into
   `C:\eve-logs` and run it from the client console* mechanism
   (`docs/reference/destiny-reference-blob.md` §2), not an import override.

**Recommendation: ship neither.** One click, documented, and the real fix is
§4.

---

## 3. Why search works when browse does not

Two paths, one shared setting, and they diverge on **what the setting gates**.

| | browse | search |
|---|---|---|
| entry point | `LoadMarketListOrSearch` → `LoadMarketList` → `GetGroupListForBrowse` | `OnSearchFieldChanged` → `DoSearch` (`marketbase.py:849`) → `Search` |
| where the type list comes from | `marketutils.GetMarketGroups()` (`marketbase.py:1022`), FSD static data | `marketutils.GetMarketTypeIDFinder().FindTypeIDsWithPartialMatch(...)`, built from `GetMarketTypes()` — also FSD static data |
| what reads `showonlyavailable` | `FilterItemsForBrowse` **2681** | `GetSearchEntry` **2000**; `GetSearchResult` **1860** |
| what the setting decides | **membership**: `ret.append(typeID)` is skipped (2685–2686), then the whole group is skipped (1031–1032) | **one boolean on the node**: `data.inRange = False` (2003) vs `True` (2005) |

**The exact point of divergence is `marketbase.py:2002`.** **[V]**
`GetSearchEntry` (`firstlineno` 1990):

```text
2000  onlyShowAvailable = settings.user.ui.Get('showonlyavailable', 0)
2002  if onlyShowAvailable and not self.IsTypeIdInCurrentRange(typeID):
2003      data.inRange = False
      else:
2005      data.inRange = True
2006  entry = GetFromClass(GenericMarketItem, data)
2007  return entry
```

The entry is built and returned either way — there is no `continue` and no
`append` guard. `IsTypeIdInCurrentRange` (`firstlineno` 2009) is
`if self.searchAsksForMyRange: return typeID in self.searchAsksForMyRange`
(2010–2011) `return False` (2012) **[V]**, so with our empty dict it short
-circuits at 2010 and every search hit gets `inRange = False`.

**[V]** `inRange` is then read exactly once, in `GenericMarketItem.Load`
(`entries.py:235`): `if not node.get('inRange', True):` — and the body sets
`SetOpacity(0.5)` and a hint of `UI/Market/Marketbase/NotAvailableInRange`.
So the owner's search results are present and **half-faded, with a "not
available in range" tooltip**, which is precisely the rendering the client
intends for "this exists but nobody is selling it here".

`showonlyavailable` at `GetSearchResult` **1860** is only the third element of
the `self.sr.lastSearchResult` cache key, so flipping the checkbox
invalidates the cached result list; it never filters it.

One more thing the client says itself: **[V]** `Search` disables the filter
icon at `marketbase.py:1294` and sets its hint to
`UI/Market/FiltersDontApply` at **1295**, re-enabling it in
`LoadMarketListOrSearch` at **866–867**. So the client's own UI tells the
player that filters do not apply to search — which is true of the three skill
filters and *almost* true of this one.

---

## 4. What the server can do about it

**It can fix it properly, and the call is `marketProxy.GetRegionBest`.** The
honest answer is not "nothing"; it is "nothing *short of having real orders*,
and the one thing that would fake it is a lie the player can see".

### The only bulk availability question the client asks

There is exactly one, in three range-dependent forms, and nothing else on the
browse path touches the wire:

| candidate | is it asked? | does it mark types available? |
|---|---|---|
| `marketProxy.GetRegionBest` | **yes**, on every browse load, when Range = Region (the default) — `marketbase.py:911` | **yes.** Its return value *is* `asksForMyRange` |
| `marketProxy.GetStationAsks` | yes, when Range = Station — `:907` | yes, same slot |
| `marketProxy.GetSystemAsks` | yes, when Range = Solar System or in a wormhole region — `:909` | yes, same slot |
| `marketProxy.GetPlexBest` | only for PLEX, inside `FilterItemsForGroupPage` (`:2717-2719`) | one type only |
| a region-wide `GetOrders` | **no.** `GetOrders(typeID)` is per-type and lazy, issued when a type is *selected*. The filter never consults it | no |
| a price-history call | `GetHistoryForManyTypeIDs`, `GetPlexHistory`, `Get{Old,New}PriceHistory`. All feed the ticker and the chart | **no.** Nothing in `FilterItemsForBrowse` or `ShouldShowType` reads history |
| **a market-group call** | **no — there is none.** **[V]** `marketutils.GetMarketGroups` (`quote.py:205`, `:215`) builds the tree from `marketgroups.data.MarketGroups`, whose `__resBuiltFile__` is `res:/staticdata/marketGroups.fsdbinary`. The group tree and each group's `types` are local FSD static data | no |

So: **the browse tree's shape is entirely client-side static data, and the
server's only lever on it is the availability dict.** That is a clean result
and it is the main thing this document exists to record.

### The shape that dict must have — and a trap in it

**[V]** `PopulateAsksForMyRange` does *not* treat the three forms alike:

```text
898  marketRange = self.GetRange()
899  if marketRange in (const.rangeStation, const.rangeSolarSystem) or idCheckers.IsWormholeRegion(session.regionid):
900      asksForMyRange = ConvertTuplesToBestByOrders(asksForMyRange)
901  self.asksForMyRange = asksForMyRange
```

and `ConvertTuplesToBestByOrders` (`marketutil/__init__.py:8`) is
`{typeID: BestByOrder(*order) for typeID, order in bestOrdersByType.iteritems()}`
over `BestByOrder = namedtuple('BestByOrder', ['price','volRemaining','typeID','stationID'])`
(`marketutil/__init__.py:6`) **[V]**.

| call | value type required | why |
|---|---|---|
| `GetStationAsks`, `GetSystemAsks` | a **4-sequence** `(price, volRemaining, typeID, stationID)`, in that order | `BestByOrder(*order)`; a 3- or 5-sequence is `TypeError`, and a non-iterable is `TypeError` |
| `GetRegionBest` | **not converted** — whatever we send is stored as-is. Read as `getattr(ask, 'price', 0)` / `getattr(ask, 'volRemaining', 0)` in `FilterItemsForGroupPage` (`:2713` onwards), which also reads `ask.stationID` with no default, for the jumps; otherwise only for truthiness | the `else` arm at 911 skips line 900 |

Both keys are `typeID` **[V]** (`asksForMyRange.get(typeID, None)` at 2684).
`GetRegionBest` is the laxer of the two and also the one that is live today —
which means **the station/system forms are a latent `TypeError` waiting for
the first player who switches the Range radio to Station**, if we ever put
anything in them that is not a 4-sequence. They are empty dicts today, so the
conversion iterates zero times and the trap is dormant.

### Is answering it sane for us?

**Not as a cosmetic fill, and this is the recommendation.** Returning
`{typeID: True}` for every published market type *would* fill the tree with
the filter on — `FilterItemsForBrowse` only tests truthiness — and
`FilterItemsForGroupPage` would even render the price column honestly, because
`data.price` would be `0`, falsy, and it formats that as
`UI/Market/Marketbase/NoneAvailable` **[V]**. But:

- it makes "Show only available" mean nothing, which is a worse lie than an
  empty tree, and it is the kind of lie that costs an evening later when
  somebody is debugging why a type with no orders is listed as available;
- clicking any of those types still shows an empty order book from
  `GetOrders`, so the player sees the contradiction one click in;
- the moment we have real orders we would have to take it back out, and the
  station/system forms would then need the 4-tuple shape anyway.

**The right fix is to make the dict true**: implement order placement, keep an
order store, and have `GetRegionBest` answer the real best ask per type for
the session's region. Then the filter does what it says, the tree fills with
exactly what is for sale, and the `showonlyavailable` default stops being a
papercut. That is §5's implementation order.

**Until then, the documented "no" is:** *no value the server can return will
populate that tree without asserting that items are for sale which are not.
Untick the box.*

One related finding worth carrying into that work: **[V]**
`marketsvc.OnOwnOrdersChanged` (`firstlineno` 129) invalidates the
objectCaching entries for `GetCharOrders` (130), `GetOrders(order.typeID)`
(133), `GetPlexOrders` (134), `GetSystemAsks` (136), `GetStationAsks` (137),
`GetMarketOrderHistory` (138) and `GetPlexBest` (139) — and **`GetRegionBest`
is not in that list.** So once orders exist, a newly placed order will refresh
the station and system availability views but not the region one, which is the
default. **[I]** The browse tree will therefore lag a new order until the
market window is reloaded, and that is the client's behaviour, not ours to
fix.

---

## 5. What a working buy/sell needs next

First, a correction to the framing: **there is no `marketProxy` method without
an arm.** A bytecode sweep of every `self.GetMarketProxy().X` in
`marketsvc.pyc` (matching `LOAD_ATTR GetMarketProxy` → `CALL_FUNCTION` →
`LOAD_ATTR`) yields **23 distinct methods at 23 call sites** **[V]**, and with
`quote.StartupCheck` that is the 24 the previous lane enumerated, all of which
`market.rs` answers. The gap is not missing arms; it is (a) missing *behaviour*
behind the mutator arms, (b) missing *rows* in the readers, and (c) two
services on the Sell-Item path that are not the market's.

Live-lock column read against `docs/guides/debugging-the-client.md` §4 — severity set by
the caller's cadence, not by the shape of the guard.

| # | service.method | args | required return | container family | what the client does | live-lock risk |
|---|---|---|---|---|---|---|
| 1 | `marketProxy.GetRegionBest` | 0 | `{typeID: ask}` — one entry per type with a sell order in the session's region | **dict**; values free-form, read via `getattr(.., 'price', 0)` / `getattr(.., 'volRemaining', 0)` | fills `asksForMyRange`; gates the whole browse tree (§1, §4) | **none.** User-paced: `LoadMarketListOrSearch` (`:862`), `ShowGroupPage_thread` (`:1185`). `DoSearch` (`:849`) re-fetches per keystroke past 2 chars via `PopulateSeachAsksForMyRange` — chatty by design, not a loop |
| 2 | `marketProxy.GetStationAsks` / `GetSystemAsks` | 0 | `{typeID: (price, volRemaining, typeID, stationID)}` | **dict of 4-sequences** — forced by `BestByOrder(*order)`, `marketutil/__init__.py:6` | same slot, after `ConvertTuplesToBestByOrders` (`marketbase.py:900`) | none, same callers. **But**: a value of the wrong arity is `TypeError` the first time a player picks a non-Region range |
| 3 | `marketProxy.PlaceBuyOrder` | **9**: `stationID, typeID, price, quantity, orderRange, minVolume, duration, useCorp, expectedBrokersFee` | return **discarded** (`marketsvc.py:313`) | — | **both** an immediate buy and a buy order: `buyThisTypeWindow.Buy` (`:984`) sets `duration = 0` for an immediate purchase and reads it from the duration combo otherwise, then calls `marketQuote.BuyStuff` → `PlaceBuyOrder`. The only visible effect is the `OnOwnOrdersChanged` push | **none from the return.** The risk is on the push: `orders.RefreshOrders` (`:84-91`) re-reads `GetMyOrders` on every `OnOwnOrdersChanged`, coalesced by a one-shot `AutoTimer` (`:89`). A push on a server timer would be a loop *we* drive |
| 4 | `marketProxy.BuyMultipleItems` | 3: `stationID, itemList, useCorp` | a **list of order objects**, each with `.typeID` and `.orderID` | **list** (iterated twice: `marketsvc.py:317`, `buyMultiFromBase.py:331`) | Multibuy. `[]` is "nothing was created" and is honest today; `None` is a `TypeError` | none — a button press |
| 5 | `marketProxy.PlaceMultiSellOrder` | 4: `itemList, useCorp, duration, expectedBrokersFee` | **truthy** when orders were created | anything truthy; `None`/`[]` keeps the Sell window open | the whole Sell path, single item included: `sellMulti._PerformTrade` (`:500`) then `if wereOrdersCreated:` (`:509`) | none — a button press. No store-back behind that `if`, so it is a precondition, not a latch |
| 6 | `marketProxy.PlacePlexSellOrder` | 4: `item, useCorp, duration, expectedBrokersFee` | as above | as above | same, PLEX arm (`sellMulti.py:503`) | none |
| 7 | `marketProxy.ModifyCharOrder` | **9**: `orderID, newPrice, bid, stationID, solarSystemID, price, range, volRemaining, issueDate` | discarded (`marketsvc.py:333`) | — | price edit from `buyThisTypeWindow.Modify` (`:867`). Visible effect is the push only | none |
| 8 | `marketProxy.ModifyPlexCharOrder` | same 9 | discarded | — | PLEX arm | none |
| 9 | `marketProxy.CancelCharOrder` | 2: `orderID, regionID` | discarded (`marketsvc.py:328`) | — | cancel from the My Orders window. Push only | none |
| 10 | `marketProxy.GetCharOrders` | 0 | rows carrying at least `orderID, typeID, bid, price, volRemaining, stationID, solarSystemID, escrow, charID` **[V]** (read in `orders.py` / `personalMarketOrders.py`), plus `keyID` for `quote.GetWalletDivision` (`quote.py:521`) and `volEntered`/`minVolume`/`regionID`/`range`/`issueDate`/`duration`/`jumps` from the public-book readers **[I]** | **unknown family.** `[]` today; the window iterates and reads attributes, which a `CRowset` over a descriptor satisfies. Whether a plain list of `KeyVal` would do is **unknown** — nothing observed requires `.columns` on *this* rowset | the My Orders window. `GetMyOrders` is a pure pass-through (`marketsvc.py:439-440`), no caching | **none from the guard** — no `is None` cache anywhere on the path. Pacing is `OnOwnOrdersChanged` plus the one-shot timer at `orders.py:89` |
| 11 | `marketProxy.GetCorporationOrders` | 0 | as above, plus the `headerIssuedBy` / `headerWalletDivision` columns `corpMarketOrders.GetBuyHeaders` adds | as above | corp orders tab | none (pass-through, `:442-443`) |
| 12 | `marketProxy.GetMarketOrderHistory` | 0 | expired/completed own orders, same column family | as above | the Market History tab; invalidated by the push at `marketsvc.py:138` | none (pass-through, `:445-446`) |
| 13 | `marketProxy.GetOrders` | 1: `typeID` | 2-tuple of two **`CRowset`**s with **real rows** | already correct and empty; needs rows | the order list the owner was looking at. Rows need every column in `market_order_descriptor()`, and `jumps` is written by the client (`marketsvc.py:821`) | **latches** — `RefreshOrderCache` stores at `marketsvc.py:864` with `version` 0 from the purely local `GetCachedMethodCallVersion`, and returns early thereafter. Invalidated only by `OnOwnOrdersChanged` (`:133`) |
| 14 | `marketProxy.GetOldPriceHistory` / `GetNewPriceHistory` | 1: `typeID` | one `CRowset` each over `price_history_descriptor()`, with rows; **positional** order is the contract (`rec[0]..rec[5]`, `entry[3]*entry[4]`) | `CRowset`, correct today | the price chart. Empty renders a flat chart, which is honest | none; chart build is user-paced |
| 15 | `marketProxy.CharGetTransactions` / `CorpGetTransactions` | — | transaction rows | `CRowset` (each row goes through `utillib.KeyVal(row)`) | the wallet's market tab. Correct and empty | none |
| 16 | **`invbroker.ListStations`** | 0 | iterable of rows with `.stationID` and `.itemCount` **[V]** (`quote.py:417`) — and `assetsWindow.Search_thread` (`:561`) needs a **real rowset** with `.Index` | **rowset** (the `.Index` consumer forces it; `quote.PickItem` alone would accept a list) | **the Sell Item path.** Right-click an item → Sell → station picker. `None` is a `TypeError` in the `for` | **not mine** — `inventory` lane. Census: `partial`, handle via `GetInventory()` |
| 17 | **`invbroker.ListStationItems`** | 1: `stationID` | iterable of rows with `.typeID`, `.locationID`, `.stacksize` **[V]** (`quote.py:436`) | list or rowset; `assetsWindow.GetSubContent` (`:939`) passes it on | the second half of Sell Item: picks the stack to sell | **not mine** — `inventory` lane |
| 18 | `structureSettings.CharacterGetService` | 2 | a market-tax value | unknown | `MarketQuote.GetMarketTaxAtStructure`, `@Memoize(2)`. **Cannot fire today** — `GetBaseTax` short-circuits for `IsStation(stationID)` and there are no structures | dormant. Flagged by the `market` lane; a structures concern, not a market one |

**Not needed, recorded so nobody goes looking:** a market-group call (§4),
`reprocessingSvc.GetQuotes` (the reprocessing window, not the market), and
`map.GetStationInfo` (answered, `world-corp`'s).

**The push side is already censused** — `OnOwnOrdersChanged` is
`(orders, reason, isCorp)`, six declarers, `orders` iterated for `.typeID`
/ `.orderID` / `.duration`, `reason` compared against `'Expiry'`, and
`marketsvc` guards `if orders is not None` so **`None` is a legal payload**
that just invalidates and refetches. See `docs/reference/push-events.md` §"the table"
and `findings/push-events.tsv:1791-1796`. Also scattered on the same handler:
`OnMarketEscrowChanged` (`marketsvc.py:141`) and the neocom blink (`:144`).

### Implementation order

1. **Order placement with a persistent order store, plus the
   `OnOwnOrdersChanged` push.** Everything else is downstream of it: it is
   what makes rows exist. Start with `PlaceBuyOrder` with `duration > 0`
   (a resting order), because its effect is purely "a row now exists" and its
   return is discarded, so nothing can be shaped wrong. Send
   `OnOwnOrdersChanged` **once per mutation, never on a timer** — item 3's
   risk column.
2. **`GetCharOrders` with real rows, then `GetOrders` with real rows.** Own
   orders first: the column set is smaller, the window is simpler, and the
   client will tell us in a traceback which attribute we forgot. Then the
   public book, which is the same rows seen from the other side and already
   has a descriptor and tests in `market.rs`.
3. **`GetRegionBest` derived from that store** — one entry per type that has
   a live ask in the session's region. This is the item that closes this
   document: the browse tree fills, the default filter becomes correct instead
   of merely on, and the §1 workaround stops being needed.

Then, in order and all smaller: `BuyMultipleItems` returning created orders;
`PlaceBuyOrder` with `duration = 0` matching against existing asks (the
immediate buy, same method, item 3); `invbroker.ListStations` /
`ListStationItems` with the `inventory` lane, which unlocks Sell;
`PlaceMultiSellOrder`; `ModifyCharOrder` / `CancelCharOrder`;
`GetMarketOrderHistory` and the transaction rowsets; price history last,
because an empty chart is honest and nothing hangs on it.

---

## 6. What is *not* settled

- **The own-order container family (item 10).** `[]` works today because the
  list is empty. Nothing observed forces a `CRowset` on `GetCharOrders` the
  way `RefreshJumps` forces one on `GetOrders`, and I did not sweep
  `orders.py` / `personalMarketOrders.py` / `corpMarketOrders.py` for a
  `.columns` or `.Index` read. **unknown**, and it decides a shape, so the
  next lane should settle it before writing rows.
- **`GetRegionBest`'s real value type.** The client only needs truthiness and
  two `getattr`s with defaults, so *many* things work; which one CCP's server
  actually sends is **unknown**, and we have never seen a non-empty one.
- **The English label** for `UI/Market/ShowOnlyAvailable` — **[I]**, no
  localisation table is extracted into `out/`.
- **`marketbase.pyc` has been read, `marketSvc.pyc` has not.** There are two
  files one letter apart: `eve/client/script/ui/services/marketsvc.py` (the
  `marketQuote` service, read here) and
  `eve/client/script/ui/shared/marketSvc.py` (`svc.market`, which also
  declares `OnOwnOrdersChanged` and discards all three arguments —
  `findings/push-events.tsv:1796`). I did not read the second.

---

## 7. A tooling trap, caught in this round

The brief warned that uncompyle6 sometimes prints const-table *indices* where
values belong. Here is this round's specimen, because it is a clean one and it
is in a file on the market path.

`tools/analysis.sh uncompyle6 /work/out/pyc/marketutil/__init__.pyc` prints:

```python
OrderAllData = collections.namedtuple('OrderAllData', [
 12, 13, 14, 15, 8, 6, 7, 16, 17, 18, 19, 20, 21, 9, 22, 23, 24, 25, 26, 27, 28, 29])
```

Twenty-two small integers where twenty-two field names belong. **The tell is
that they are not monotonic** — `15, 8, 6, 7, 16` — which is what you would
expect from indices into a const table that already holds `price`,
`volRemaining`, `typeID` and `duration` from the `BestByOrder` literal six
lines above, and not from anything a human wrote. A monotonic run would have
been much easier to mistake for data, which is the general warning.

The disassembly of the same module (`tools/pyc27.py`, one `LOAD_CONST` per
line) gives the real list, lines **14**–**35** **[V]**:

| line | field | line | field | line | field |
|---|---|---|---|---|---|
| 14 | `id` | 22 | `minVolume` | 30 | `solarSystemID` |
| 15 | `issuer` | 23 | `range` | 31 | `accountID` |
| 16 | `isIssuerCharacter` | 24 | `bid` | 32 | `corporationID` |
| 17 | `isIssuerNpcCorp` | 25 | `issueDate` | 33 | `corporationWalletKeyID` |
| 18 | `typeID` | 26 | `duration` | 34 | `escrow` |
| 19 | `price` | 27 | `stationID` | 35 | `state` |
| 20 | `volRemaining` | 28 | `structureID` | | |
| 21 | `volEntered` | 29 | `regionID` | | |

(`'OrderAllData'` itself is at line 12, `BestByOrder` at line 6.)

**[I]** Those 22 fields are also the best available description of what a full
market-order row carries on the real server, which makes it worth having read
correctly: it is the superset from which `market_order_descriptor()`'s 14
columns are drawn, and it names the two the `market` lane deliberately left
out of the public book (`issuer`, `corporationWalletKeyID`) plus three we have
not needed yet (`structureID`, `accountID`, `state`). **`OrderAllData` is not
itself used by anything we answer** — nothing in `marketutil` constructs one —
so treat it as a field dictionary, not as a shape.
