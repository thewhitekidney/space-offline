# The client's call surface

`findings/client-calls.tsv` is a census of every machoNet service call **site**
in the V24.01 client bytecode — one row per place in the client where a call is
issued, not one row per method — with the things that actually decide an
answer's shape recorded beside it: what the client does with the return value,
and what guard it is wrapped in.

It exists because the loop up to now has been reactive and serial: launch,
break, read the capture, answer one more call. That works, and `--alerts` and
`--triage` made it fast, but it only ever surfaces the next gap. An earlier
enumeration pass (`findings/remote-services.txt`, `findings/client-services.json`)
recorded **names** only, and names turn out to be the least useful thing about a
call: `charMgr.GetHomeStationRow` and `charMgr.GetLabels` look identical in a
name list, and one of them is a one-shot read behind a `@Memoize` while the
other is three poisoned caches in the address book. Read
`docs/guides/debugging-the-client.md` §4 first — the ten kinds of empty are the reason the
`consumer` and `guard` columns are the point of this file and the other six are
scaffolding.

---

## 1. The numbers

| | |
|---|---|
| call sites | **1816** |
| distinct `service.method` pairs | **1240** |
| distinct services named | **181** |
| client source files containing a call | **448** |
| of those sites, in `eve/devtools/` (GM tools, not the player path) | 187 |

Served, measured against the dispatcher rather than assumed (§4):

| | call sites | distinct pairs |
|---|---|---|
| `yes` — service routable **and** the method has an arm | 457 (25%) | 221 (18%) |
| `partial` — service routable, method has no arm (we answer `None` + `UNANSWERED`) | 753 | 514 |
| `no` — the service is in no module's `SERVICES` (we answer `None` + `no such service`) | 601 | 501 |

Those were 383 / 772 / 656 sites when this file was first written. The move is
two different things, and they are worth separating because only one of them is
progress: round 1 of `docs/archive/plan-docked-subsystems.md` added real arms, and **19
`(service, method)` pairs were already answered and being reported as
`partial`** by a bug in the scanner — rustfmt wraps a long `"a" | "b" => …`
across lines, and the matcher was anchored per line on a leading quote, so
every wrapped arm was invisible. Measured by running both scanner versions
against the same tree: 251 pairs before the fix, 270 after.

That is the `charMgr.GetHomeStationRow` mistake pointing the other way. The
hand pass over-claimed; this under-claimed, which is the direction nobody
thinks to check, and it was found by a lane whose own five-way arm came back
`partial` while working correctly.
| `unknown` — the service name is not a constant in the bytecode | 5 | 4 |

`partial` and `no` behave identically on the wire today — `Dispatcher::unhandled`
answers `None` either way — so the split is about *where the work goes*, not
about client-visible behaviour. 107 of the 181 services have no arm at all.

**Consumer** (what the client does with the value, immediately):

| consumer | sites | | consumer | sites |
|---|---|---|---|---|
| `discard` | 618 | | `subscript` | 37 |
| `pass` | 377 | | `arith` | 20 |
| `iterate` | 199 | | `unpack_3` | 17 |
| `truth` | 133 | | `rowset` | 15 |
| `store` | 114 | | `len` | 15 |
| `attr` | 110 | | `pack` | 12 |
| `compare` | 86 | | `unpack_4` | 8 |
| `unpack_2` | 44 | | `unpack_5..8` | 5 |
| | | | `native` | 1 |
| | | | `unknown` | 5 |

**Guard** (counted per component, because a row can carry several joined with `+`):

| guard | sites |
|---|---|
| `none` (no pattern we automated matched) | 1434 |
| `try_except:<Exc>` | 141 |
| `if_x_is_none` | 91 |
| `if_not_x` | 68 |
| `try_finally` | 55 |
| `memoize` | 51 |
| `singleflight` | 25 |
| `finally_return` | 1 |

Of the pairs with `yes`, 42 further pairs that the dispatcher *does* answer
have **no** census row at all. That is not a bug in the dispatcher; it is the
census's main blind spot, and §5 names it.

---

## 2. How it was produced

Everything runs in the analysis container (`tools/analysis.sh`: repo at `/work`,
no network, no host installs). The generator is a Python 3 script that uses
`tools/pyc27.py`, our own read-only Python 2.7 `.pyc` reader and disassembler,
to walk **every** code object in `out/pyc/` and run a symbolic-stack machine
over its bytecode.

It works on the **disassembly**, not on decompiled source, for the reason
`docs/guides/debugging-the-client.md` gives: `POP_TOP` vs `UNPACK_SEQUENCE` vs `STORE_ATTR`
is unambiguous in bytecode and guessy in uncompyle6 output, and uncompyle6's
line numbers are its own output's lines, not the client's. The `call_site`
column is built from the code object's `co_filename` and `lnotab`, so the file
and line are the ones a traceback prints.

**Regenerating it:** `tools/census-calls.sh`, which runs the three stages in
the analysis container and rewrites this file's TSV; `tools/census-calls.sh
--diff` shows what changed and leaves the tree alone. The stages are
`tools/scan_calls.py` (the bytecode census), `tools/scan_calls_served.py` (the
dispatcher's served set, parsed out of `service.rs` and `service/*.rs`) and
`tools/scan_calls_emit.py` (the join and the TSV).

A census you cannot regenerate is a snapshot, and most of the value here is in
the **diff** after a batch of service work: which rows moved from `no` to
`yes`, and — the one to watch — whether a new `native` consumer appeared.
There is exactly one today, and it is the call that caused the undock black
screen.

With the script in place the commands are:

```sh
cd $REPO
tools/analysis.sh python3 -I tools/scan_calls.py          # writes findings/client-calls.tsv
```

and the three stages it runs are:

```text
1. scan.py    -> sites.json     1816 call sites from out/pyc/**.pyc
2. served.py  -> served.json    79 services / 201 (service, method) arms from
                                server/crates/eve-server/src/service.rs and
                                server/crates/eve-server/src/service/*.rs
3. emit.py    -> findings/client-calls.tsv  (join + deterministic sort)
```

Sorted by `(service, method, file, line)` with a case-insensitive primary key,
so a regenerated file diffs cleanly.

---

## 3. The columns

### `service`, `method`

The machoNet service name and method. For a moniker or bound call this is **the
service the moniker names** — `invbroker`, not `invbroker::inventory` — because
that is what the client's own code says and what a capture shows in the bind.
Our dispatcher sometimes spells the bound sub-object `svc::sub`
(`invbroker::inventory`, `skillMgr2::skillHandler`, `scanMgr::systemScanMgr`),
and the `served` lookup accepts either spelling.

`unknown` means the service name is not a constant anywhere the machine could
reach it — five rows, all of them generic machoNet plumbing
(`moniker.MachoBindObject`, `cfg.ConnectToService`) where the name is a
parameter.

### `call_site`

`path/file.py:LINE`, with the `C:\BuildAgent\...\release\V24.01\` prefix
stripped and backslashes flipped. **A `packages/` prefix does not appear in
`out/pyc/`**: `packages/vgs/client/vgsClient.py:44` is
`out/pyc/vgs/client/vgsClient.pyc`. That is the build tree's layout, not ours,
and it is what the client's tracebacks print, which is why it is kept.

### `kind`

- `remotesvc` — the handle came from `sm.RemoteSvc('x')`, `sm.ProxySvc('x')` or
  `session.ConnectToRemoteService('x')`, directly or through a local, an
  attribute or a one-line accessor in the same file. 1147 sites.
- `moniker` — the handle came from `Moniker('x', bindParams)`, an
  `eveMoniker.GetXxx()` helper, or a service's `GetMoniker()`. 447 sites.
- `bound` — a method on an object a previous call returned: the `invbroker`
  inventory objects, `skillMgr2.GetMySkillHandler()`'s handler,
  `scanMgr.GetSystemScanMgr()`, `keeper.GetLevelEditor()`. 222 sites.

The `eveMoniker` helper table (30 entries: `GetBallPark` → `beyonce`,
`CharGetDogmaLocation` → `dogmaIM`, …) is **derived**, not hardcoded: the
machine runs over `eveMoniker.pyc` first and records which function returns
`Moniker(<const>, …)`. 28 cross-module accessors and 21 attribute names are
seeded from `findings/bound-objects.json`'s `promoted_names`, which is the one
piece of the earlier pass that this one builds on rather than replaces.

### `consumer` — the column that decides the answer's shape

Classified at the bytecode op that consumes the value. The machine tracks the
value from the `CALL_FUNCTION` that produced it until something pops it, so
`f(remote(), x)` is classified as `pass -> f()` even though the value is not on
top of the stack when the call happens.

| value | what it means | why it matters |
|---|---|---|
| `discard` | `POP_TOP`; the result is never stored | the client learns the outcome from a **push** |
| `unpack_N` | `UNPACK_SEQUENCE N` | raises *before* any store; `None` is fatal even when the next line tests for `None` |
| `subscript` | `BINARY_SUBSCR` / slice | needs a real sequence/mapping |
| `attr` | `LOAD_ATTR`; the attribute name is in `notes` | needs an object, not a container |
| `rowset` | `LOAD_ATTR` of a name only a rowset answers (`Index`, `Filter`, `GetLine`, `header`, `columns`, …) | `None`/`{}`/`[]`/`(header, data)` all raise |
| `iterate` | `GET_ITER`, or handed to `list`/`dict`/`sorted`/… | `None` → `TypeError` |
| `len` | handed to `len()` | same |
| `truth` | only tested for truthiness | `None` is honest here |
| `compare` | `COMPARE_OP`; the operator is in `notes` | **Python 2 orders `None` below every number**, so a `None` here is a wrong answer with no symptom — the `crimewatch.GetMySecurityStatus` trap |
| `arith` | arithmetic on it | `None` → `TypeError` |
| `native` | the callee is an object created by `blue`/`destiny`/`trinity` | strict about list-vs-tuple in a way no Python caller can observe |
| `store` | cached, and the cached value is never used in the same function | the real consumer is elsewhere |
| `pack` | built into a tuple/list/dict | |
| `pass` | handed to another function, **named in `notes`** | |
| `unknown` | the machine could not trace it — 5 rows | not guessed |

`compare`, `arith`, `pack` and `unknown` are extensions to the vocabulary the
lane was given. They were added because leaving them as `unknown` would have
thrown away the one distinction that caught `GetMySecurityStatus`.

**Two consumers are resolved one hop, and say so.** 350 sites had `-> return`
as their only immediate consumer — the value leaves the function, so the
immediate classification is useless. For those the machine makes a second full
pass over the tree, probes every call to a function of that name, and if all
callers agree on one class, adopts it and writes
`via return from <func>(); all N callers: <class>` into `notes`. 109 of 350
resolved that way; the other 241 keep `pass` and carry the caller distribution
in `notes` (e.g. `callers of GetMySecurityStatus(): compare x16, pass x8`).
Function names are matched tree-wide, so a collision is possible; that is why
unanimity is required and the caller count is printed.

### `guard`

What the call is wrapped in, joined with `+` when several apply. **`none` means
"none of the patterns we automated matched"**, not "no guard" — see §5.

- `if_x_is_none` — the call is in the body of `if x is None:`, or after an
  `if x is not None: return x` early exit. 91 sites.
- `if_not_x` — the call is in the body of `if not x:`. 68 sites. **Here `{}`,
  `[]`, `0`, `0.0`, `''` and an empty `CRowset` are all live locks.**
- `try_except:<Exc>` — the exception swallowed, read off the handler's
  `COMPARE_OP 10`; `*` is a bare `except:`.
- `try_finally` / `finally_return` — the latter is a `return` inside `finally`,
  which **discards the in-flight exception** and makes the failure invisible in
  every server-side instrument.
- `memoize` — the enclosing function carries `@Memoize(n)` / `@MemoizeWithLock(n)`
  (`caching/memoize.py`; `n` is **minutes**).
- `singleflight` — the call is inside `with <something>Lock(...):`.
- `caller:<guard>` — the guard is one frame up, at the call to the enclosing
  function, not at the wire call. 65 rows. This is how the one `finally_return`
  in the tree is recorded, and it is the right place for it: the remote call in
  `contracts.py:825` is unguarded, and the damage happens in its caller.
- `debounce_Nms` was **not** automated and appears nowhere.

Two notes in the `notes` column change how a guard reads:

- `guard on <expr>` — the expression the condition tested.
- `result stored back to the guarded name` — the result is assigned to the thing
  the guard tested. **This is the live-lock signature.** Without it, an
  `if not x:` is usually a precondition, not a cache.

65 rows have the store-back note; 55 of those store into a **persistent**
attribute (`self.x`) and 10 into a local or a defaulted parameter. Only the 55
can loop: a local `if not dna: dna = GetPaperDollData(charID)` in an `__init__`
runs once per construction, which is why `paperDollServer.GetPaperDollData`
appears with store-back and is *not* on the priority list.

### `served`

Determined by parsing `server/crates/eve-server/src/service.rs` and
`server/crates/eve-server/src/service/*.rs` — not by memory, because a previous
pass claimed `charMgr.GetHomeStationRow` was answered when it was not:

1. every `SERVICES: &[&str]` list, for the routable service names (79, counting
   the `svc::sub` spellings);
2. every `"Method" => …` arm inside a `"svc" => match req.method {` block,
   including multi-pattern arms and the two delegating arms
   (`machoNet => reg.macho_net`, `charUnboundMgr => reg.char_unbound_mgr`);
3. `Registry::OBJECT_RETURNING`, the bind accessors answered by the bind path
   rather than by a method arm — without it `skillMgr2.GetMySkillHandler`,
   `invbroker.GetInventoryFromId`, `invbroker.GetInventory` and
   `scanMgr.GetSystemScanMgr` all read `partial` while actually working.

`GetHomeStationRow` reads `yes` in this file because `character.rs:225` now has
an arm for it. That is a change since the mistake, not a repeat of it.

### `notes`

One line, no tabs. Always carries the enclosing function and the positional
argument count (`in GetStationClones(), 0 args`), plus the handle's provenance
when it was indirect (`handle via GetInventoryFromId()`), the guard expression,
the store target, the `pass` target, and the one-hop caller resolution.

---

## 4. The rows that look like trouble

### 4a. `if_not_x` / `if_x_is_none` with a persistent store-back — 55 rows

These are the live locks. The mechanism is in `docs/guides/debugging-the-client.md` §4 and
has two variants that need telling apart, because the first leaves a traceback
and the second leaves nothing:

- **the LP variant** — the answer throws inside the guarded statement, so the
  assignment never runs, so the cache stays unset, so the next read re-issues.
  Spot it by a `consumer` of `iterate`, `subscript`, `attr`, `arith` or
  `unpack_N` on a store-back row.
- **the `NumOutstandingContracts` variant** — the answer stores *successfully*
  and is the one value that fails the guard. No throw at the assignment; the
  throw, if any, is a line later. Spot it by a `consumer` of `store`, `truth`
  or `pass` on a store-back row with `if_x_is_none`, because `None` is exactly
  what `is None` rejects.

The ten to fix first, in order, are in §4e. The full 55 are the rows carrying
both `result stored back to the guarded name` and `cached as @`:

```sh
awk -F'\t' '$8 ~ /stored back to the guarded name/ && $8 ~ /cached as @/' \
    findings/client-calls.tsv
```

**The counts are lower bounds.** The detector takes the *nearest* conditional
jump that skips the call, so a compound condition hides half of itself.
`jumpCloneSvc.GetShipCloneState` at `clonejumpsvc.py:105` reads `guard=none`,
and the real code is

```python
if not self.shipJumpClones or shipID != self.shipJustClonesShipID:
    ...
    self.shipJumpClones = lm.GetShipCloneState()
```

— the nearest jump belongs to the `!=`, so the `if not` half is invisible. It is
the same live lock as its sibling two methods down. When a row says `none` and
the method smells like a cache, read the caller.

### 4b. Every `unpack_N` — 74 rows

An `UNPACK_SEQUENCE` raises before any store and before any `is None` test, so
these are the rows where `None` is fatal and the arity is not a free choice.
25 of the 44 `unpack_2` rows and 2 of the 8 `unpack_4` rows are services with
no arm at all.

The widest ones are the least forgiving, because nothing but the exact arity
will do:

| consumer | service.method | served | call site |
|---|---|---|---|
| `unpack_8` | `accessGroupBookmarkMgr.BookmarkStaticLocation` | partial | `bookmarks/bookmarkSvc.py:569` |
| `unpack_7` | `beyonce.GetRelativity` | partial | `remote/michelle.py:705` |
| `unpack_6` | `planetOrbitalRegistryBroker.GetSettingsInfo` | no | `inflight/orbitalConfiguration.py:67` |
| `unpack_5` | `posMgr.GetTowerSentrySettings` | no | `inflight/moonmining.py:696` |
| `unpack_5` | `kiringMgr.PerformKiringServerSideAuthenticationFromCode` | no | `kiring/client/apigateway.py:320` |
| `unpack_4` | `ship.GetShipFittingInfo` ×2 | partial | `ui/crate/controller.py:76,252` |
| `unpack_4` | `warStatisticMgr.GetBaseInfo` | no | `corporation/war/warReport.py:294` |
| `unpack_4` | `petitioner.GetCategoryHierarchicalInfo` | no | `neocom/petitionWindow.py:394` |

The four `unpack_4` rows that read `yes` are already correct and worth keeping
that way: `crimewatch.GetClientStates` ×3 and
`seasonalLoginCampaignManager.get_active_campaign` both answer 4-tuples today.

```sh
awk -F'\t' '$5 ~ /^unpack_/' findings/client-calls.tsv
```

### 4c. Every `native` consumer — 1 row

```
beyonce  GetFormations  eve/client/script/remote/michelle.py:452  remotesvc  native  none  yes
```

`self.__bp` is `blue.classes.CreateInstance('destiny.Ballpark')` and
`LoadFormations` is native, so `[]` raises `TypeError: must be tuple, not list`
and the player gets a black screen after undocking. We answer
`PyValue::Tuple(vec![])` (`space.rs:172`), which is right.

The value of this row is that it is the **only** one. The machine finds native
consumers by learning which attributes hold objects created by
`blue`/`destiny`/`trinity`/`bloodyPlugin` and propagating that through attribute
chains — it found this one from first principles, including the two-hop
`self.__bp.LoadFormations`. So if a regenerated census ever shows a second
`native` row, that is a new black-screen risk and should be read the day it
appears.

```sh
awk -F'\t' '$5 == "native"' findings/client-calls.tsv
```

### 4d. The `discard` clusters — the map of what the push path owes

618 sites discard the return value. `POP_TOP` means the client is not waiting
for data: it issued a command and expects a **notification** to tell it what
happened. Answering those calls correctly buys nothing on its own.

Excluding `eve/devtools/` call sites and the `slash` service (the GM-tool
transport, which has its own story and accounts for 71 of the 618) leaves
**486 sites across 100 services and 426 distinct methods** — of which 21 are
answered, 273 are `partial` and 192 are `no`.

The clusters, largest first:

| service | discard sites | the push the client is waiting for |
|---|---|---|
| `beyonce` | 27 | `DoDestinyUpdate` — `Cmd*` is the whole movement API |
| `invbroker` | 27 | `OnItemChange` — `Add` ×9, `MultiAdd` ×4, `SetLabel` ×3 |
| `mailMgr` | 22 | mail notifications |
| `corpRegistry` | 21 | corp/role change notifications |
| `allianceRegistry` | 20 | alliance notifications |
| `fleetObjectHandler` | 19 | `OnFleetStateChange` and friends |
| `charMgr` | 17 | contact/note notifications |
| `warRegistry` | 16 | war notifications |
| `dogmaIM` | 14 | `OnGodmaItemChange` / `OnDogmaAttributeChanged` |
| `posMgr` | 13 | structure state |
| `userSvc` | 12 | — |
| `mailingListsMgr` | 11 | — |
| `petitioner` | 11 | — |
| `ProjectDiscovery` | 10 | — |

`beyonce` and `invbroker` are first for a reason: `beyonce.Cmd*` is how the
player moves and `invbroker.Add`/`MultiAdd` is how the player moves *items*, and
in both cases a `None` answer plus no push is a UI that silently does nothing.
Cross-reference `docs/reference/push-path.md`.

```sh
awk -F'\t' '$5 == "discard" && $3 !~ /devtools/ && $1 != "slash"' \
    findings/client-calls.tsv | cut -f1 | sort | uniq -c | sort -rn
```

### 4e. The ten to fix next

1. **`jumpCloneSvc.GetShipCloneState`** — `clonejumpsvc.py:105`, **served
   `yes`**, and its row says `guard=none`, which is this file's own worst
   false negative (§4a: the compound `if not self.shipJumpClones or shipID != …`
   hides the `if not` half from the detector). We answer an empty `CRowset`, and
   `carbon/common/script/sys/crowset.py` makes `CRowset` a **subclass of
   `list`** — so an empty one is falsy, `self.shipJumpClones` never latches, and
   every `GetShipClones()` re-issues. Two reachable callers, and the second is
   the problem: `infosvc.GetCurrentShipCloneCount` is a **dogma attribute
   display hook** doing `len(sm.GetService('clonejump').GetShipClones())`, so it
   runs whenever the info window paints that attribute. That is the LP
   live-lock cadence — the one the player reported as "fps issues" — on a call
   we already answer, with no traceback and no `UNANSWERED` line because the
   call is answered. Check it with the repeat-count grep in
   `docs/guides/debugging-the-client.md` §4 before changing anything.
   Its sibling `GetStationCloneState` (`:111`, which the detector *does* catch
   as `if_not_x` + store-back) is the identical shape but **not** reachable:
   `GetStationClones` appears nowhere else in the tree, so only the
   `__exportedcalls__` path — the server calling the client — can reach it. Fix
   both together; only the first one costs us anything today.
2. **`shipCosmeticsMgr.GetLicencedSkins`** — `cosmeticsSvc.py:157`,
   `if_x_is_none`, store-back to `self._licensedSkins`, consumer `iterate`,
   `partial`. `shipCosmeticsMgr` is routable but this method has no arm, so
   `None` is stored into the cache that tests `is None`, poisoning it for the
   session *and* making every caller's iteration raise. Both failure modes at
   once, in the fitting/Personalization window — which already takes tens of
   seconds for an unrelated gateway reason, so it will be blamed on that.
3. **`map.GetStationCount` ×3** — `mapView/filters/mapFilterStationCount.py:17`,
   `mapView/mapViewColorHandler.py:648`, `maps/starModeHandler.py:452`.
   `if_x_is_none` + store-back + `store`, all `partial`. All three guard and
   fill **the same** module-level cache, `starmap.stationCountCache`, so one
   missing method poisons one cache reached from three points on the map-redraw
   path. The `NumOutstandingContracts` variant: our `None` stores successfully
   and is the one value `is None` rejects, so no throw, no traceback, just the
   same call forever.
4. **`corpRegistry.GetTitles`** — `corporation/bco_titles.py:73`,
   `if_x_is_none` + store-back + `attr`, `partial`. `None.<attr>` throws inside
   the guarded statement, so the assignment never runs — the LP mechanism
   exactly, on a service we already answer eleven other methods of.
5. **`charMgr.GetLabels` ×3** — `addressBook/addressbookService.py:1213`,
   `:1217`, `:1221`. `if_x_is_none` + store-back, `partial`. Three caches, one
   method, one of our best-served services.
6. **`ship.GetShipFittingInfo` ×2** — `ui/crate/controller.py:76` and `:252`,
   `unpack_4`, `partial`. `ship` is routable, so our `None` reaches an
   `UNPACK_SEQUENCE 4` and raises immediately. A 4-tuple of `None` costs two
   lines and is the §4 third-row answer.
7. **`contractProxy.NumOutstandingContracts`** — `contracts.py:825`,
   `caller:if_x_is_none+finally_return`, **served `yes`**. The only
   `finally_return` in the tree, independently rediscovered by this census and
   matching `docs/guides/debugging-the-client.md`'s worked example line for line. Worth a
   test asserting our answer has `.myCharTotal`, because nothing else will ever
   tell us: a `return` inside `finally` eats the exception and `--triage` shows
   nothing.
8. **`petitioner.GetMyPetitionsEx`** (`neocom/petition.py:265`) and
   **`petitioner.GetCategories`** (`:666`) — `if_not_x`/`if_x_is_none` +
   store-back + `iterate`, both `no`. `petitioner` has 35 sites and no arms at
   all; the petition window is a self-contained subsystem where `[]`/`{}` for
   the readers would close the whole thing out cheaply.
9. **The widest unpacks with no arm** —
   `planetOrbitalRegistryBroker.GetSettingsInfo` (`unpack_6`),
   `posMgr.GetTowerSentrySettings` (`unpack_5`),
   `warStatisticMgr.GetBaseInfo` (`unpack_4`, and also `if_not_x`). Each is a
   guaranteed `TypeError` the moment the window opens, and each needs exactly
   one shape: a tuple of the stated arity. Cheapest traceback-to-fix ratio in
   the file.
10. **`beyonce.Cmd*` and `invbroker.Add`/`MultiAdd`/`SetLabel`** — 27 and 27
    `discard` sites respectively (`Add` ×9, `MultiAdd` ×4, `SetLabel` ×3 among
    them), all `partial`. Not a shape problem: these rows are the
    statement that movement and item-moving cannot be finished by answering
    calls at all. They are the top of the push path's worklist, and the census
    exists partly to say how long that list is (486 sites, 426 methods).

---

## 5. What this census does not see

Honesty about coverage is worth more than coverage, so:

- **5 rows have `consumer = unknown`** and say so
  (`accessGroupBookmarkMgr.GetFolderInfo`, `corpmgr.GetPublicInfo`,
  `dogmaIM.MergeModuleGroups`, `keeper.GetLevelEditor`,
  `mailingListsMgr.Create`). The value is stored and the machine lost it. Not
  guessed.
- **5 rows have `service = unknown`**: the service name is a parameter, in
  generic machoNet plumbing.
- **1434 rows have `guard = none`**, which means only that no automated pattern
  matched. `singleflight` via `uthread` locks that are not `with` statements,
  `debounce_Nms` of any kind, and compound conditions (§4a) are all invisible.
  `if_not_x` = 68 and `if_x_is_none` = 91 are **lower bounds**.
- **241 rows keep `consumer = pass -> return`.** The value leaves the function
  and the callers did not agree on one class. The distribution is in `notes`.
- **42 (service, method) pairs the dispatcher answers have no row here.** Three
  distinct causes, all real:
  1. *The method name is a string constant handed to generic machinery.*
     `eveCfg.LoadEveOwners` does
     `sysCfg.Recordset(EveOwners, 'ownerID', 'GetOwnersEx', 'GetMultiOwnersEx')`
     and the prime layer `getattr`s it. No literal call site exists, so a
     bytecode scan cannot see one. This covers most of `config.*`,
     `charMgr.GetContactList` and the `cfg` prime path — and it is exactly what
     `findings/remote-services.txt`'s name-based grep *does* find, which makes
     the two files complementary rather than redundant.
  2. *The handle arrives as a parameter.*
     `loginCharacterselection/charselData.py` takes `charRemoteSvc` in its
     constructor and calls
     `self.charRemoteSvc.GetCharacterSelectionData()` — so the single most
     important call on the login path has no row. Following that needs
     inter-procedural dataflow this machine does not do.
  3. *Bound sub-objects whose accessor is not in the `BIND_SHAPE` whitelist.*
     `invbroker::inventory.GetSelfInvItem`, `scanMgr::systemScanMgr.GetFullState`
     and `skillMgr2::skillHandler.GetSkillQueueAndFreePoints` are reached
     through handles the machine did not connect.
- **The `kind = bound` chain is a whitelist.** A service method is treated as
  returning a bound object only when its name matches
  `^(Get|CharGet|Macho)\w*(Handler|Mgr|Manager|Access|Location|Object|Svc|Service|Broker|Editor|Ballpark|Park|Inventory|LM|Registry|Moniker)$`.
  That is what makes the 222 `bound` rows auditable, and it is also why some are
  missing. Container and string methods (`append`, `get`, `extend`, `keys`, …)
  are excluded from being read as remote calls, which removed two false
  positives (`corpmgr.extend`, `abyssal_content_manager.get`).
- **Each row is one call site, not one execution.** A `discard` in a per-frame
  handler and a `discard` in a button callback are one row each. Cadence still
  comes from `evecap --calls` and from the repeat-count grep in
  `docs/guides/debugging-the-client.md` §4.
- **187 rows are `eve/devtools/`.** They are real calls and the GM tools matter
  to this project, but they are not the player path; filter on `$3 !~ /devtools/`
  when reading the file for login-path work.

---

## 6. Using it

```sh
# everything about one service
awk -F'\t' '$1 == "charMgr"' findings/client-calls.tsv

# unanswered methods, worst consumers first
awk -F'\t' '$7 != "yes" && $5 ~ /^(unpack_|native|rowset|iterate|subscript|attr)/' \
    findings/client-calls.tsv | sort -t$'\t' -k5,5

# the live locks
awk -F'\t' '$8 ~ /stored back to the guarded name/ && $8 ~ /cached as @/' \
    findings/client-calls.tsv

# what a service costs: how many sites, how many already answered
cut -f1,7 findings/client-calls.tsv | sort | uniq -c | sort -rn | head -40
```

Two habits that make it keep paying:

- **Regenerate it after a batch of service work** and diff. The `served` column
  moving from `partial` to `yes` is the lane's progress bar, and a new `native`
  row or a new `finally_return` row is a warning that no other instrument gives.
- **Read the row before writing the arm.** `consumer` plus `guard` is the §4
  table applied to that one call: `iterate` + `if_x_is_none` + store-back means
  `[]`, never `None`; `compare` means a number; `rowset` means a real rowset;
  `discard` means go and look at `docs/reference/push-path.md` instead.
