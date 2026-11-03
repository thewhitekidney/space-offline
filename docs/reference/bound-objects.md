# Bound objects (client build V24.01, 3619879)

The gap left by `findings/remote-services.txt`. That inventory was built by
matching `sm.RemoteSvc('name').Method()` call sites, which only sees **service
calls**. The client's second call style — *bound objects* — never appears as
such a call site, and that is where inventory, dogma and ship/space control
live.

Conventions used below, same as `docs/archive/wire-format.md`: **confirmed** = read out
of a decompiled `.pyc` in `out/pyc/` of this build, with the module named;
**gap** = lives in native code or on the server and could not be established
from the client tree. Nothing here is guessed. Every module path is relative to
`out/pyc/`.

---

## 1. The binding mechanism

### 1.1 The mechanism is `Moniker` + `MachoBindObject`, not a `BindObject` call

A binary-safe string scan of all 12,794 `.pyc` files finds `machoBindObject`
**0** times, and `BindObject` in only **6** modules — in every case as part of
`MachoBindObject` (`carbon/common/script/sys/service.pyc`,
`carbon/common/script/net/ServiceCallGPCS.pyc`,
`carbon/common/script/net/moniker.pyc`,
`carbon/common/script/net/ResolveGPCS.pyc`,
`carbon/client/script/remote/connectionService.pyc`,
`machonet_tracing/__init__.pyc`). `GetBoundObject` occurs once, in
`evegraphics/validation/__init__.pyc`, and is unrelated to machoNet. So the
mechanism in this build is **`util.Moniker` + `MachoBindObject`**, implemented
in:

- `carbon/common/script/net/moniker.pyc` — `Moniker`, `MonikerCallWrap`, `UpdateMoniker` (client side of binding)
- `carbon/common/script/net/ServiceCallGPCS.pyc` — `CoreServiceCall` (service-call layer; special-cases `MachoBindObject`)
- `carbon/common/script/net/ObjectCallGPCS.pyc` — `ObjectCall`, `MachoObjectConnection`, `MachoObjectCallWrapper` (object-call layer)
- `carbon/common/script/sys/service.pyc` — `CoreService.MachoGetObjectBoundToSession`, `MachoResolve`, `__machomethods__`
- `carbon/common/script/sys/basesession.pyc` — `ObjectConnection`, `GetObjectUUID`, `Session.ConnectToObject`
- `eve/common/script/net/eveMoniker.pyc` — the EVE-specific moniker factory functions

`carbon/common/script/sys/serviceProxy.pyc` turns out **not** to be part of this
path: `ServiceProxy` / `ServiceProxyCall` is a *server-side* node-fanout helper
(`__exportedcalls__[...]['resolve']`, `NODE_ALL`, `uthread.parallel`) that ends
in an ordinary `ConnectToRemoteService(...).<method>()`. It is not how the
client reaches a bound object. Confirmed by reading the module.

### 1.2 A moniker is a lazy handle

`Moniker(serviceName, bindParams, nodeID=None)` (`moniker.pyc`). It is
`__passbyvalue__ = 1` and persists only
`['__serviceName','__nodeID','__bindParams','__sessionCheck']`
(`Moniker.__persistvar__`, `__getstate__`/`__setstate__`), so a moniker itself
can be marshalled by value and re-created on the other side.

Attribute access rules (`Moniker.__getattr__`, confirmed):

- an attribute whose **first character is uppercase** returns a
  `MonikerCallWrap`, i.e. it becomes a remote call;
- anything else forces `Bind()` and is read off the bound object (its
  `__publicattributes__`);
- `RegisterObjectChangeHandler` / `UnRegisterObjectChangeHandler`
  (`__remobsuppmeth__`) are forwarded to the bound object, not remoted.

The same uppercase rule holds on `MachoObjectConnection.__getattr__`
(`ObjectCallGPCS.pyc`). **Consequence for the server: a bound-object method
name always starts with an uppercase letter**, and lowercase attribute reads are
served from the `__publicattributes__` payload shipped with the reference, never
from the wire.

### 1.3 Step 1 — resolve (which node owns this object)

`Moniker.__ResolveImpl` (`moniker.pyc`), client path:

1. `sm.services['machoNet'].GuessNodeIDFromAddress(serviceName, bindParams)` —
   the base implementation in `carbon/common/script/net/machoNet.pyc` is
   literally `return None`, so on a stock client this always falls through.
2. `sm.GetService('machoNet').LookupAddressCache(serviceName, bindParams)`.
3. Otherwise an **ordinary service call**
   `MachoResolveObject(bindParams)` on `serviceName`, whose return value is a
   `nodeID`; `None` is an error (`'Resolution failure: ResolveObject returned
   None'`).
4. The result is cached with
   `sm.services['machoNet'].SetNodeOfAddress(serviceName, bindParams, nodeID)`.

Note that the proxy's resolve layer treats bind specially: in
`ResolveGPCS.CallUp` (`carbon/common/script/net/ResolveGPCS.pyc`), a
service-addressed packet whose `payload[1] == 'MachoBindObject'` skips the
service's own `MachoResolve(session)` entirely (`nodeID = None`) and is let
through; any *other* method gets `MachoResolve`, and a non-matching node id
raises `WrongMachoNode(nodeID)`. A `MachoResolve` returning a **string** is an
error message, not a node id.

So the server must export `MachoResolveObject` on every bindable service. It is
one of `CoreService.__machomethods__ = ('MachoResolve', 'MachoBindObject',
'MachoResolveObject', 'MachoGetObjectBoundToSession')` (`service.pyc`), and
those four are exempt from the `__exportedcalls__` check and get
`role = ROLE_ANY` (`ServiceCallWrapper.PreCall_IsExportedCall` /
`PreCall_RoleCheck`, `service.pyc`).

**Gap:** `MachoResolveObject`'s *body* is not in the client tree (only
`MachoResolve` / `_MachoResolveAdditional` are, and those implement the
`__machoresolve__ = 'station' | 'solarsystem' | 'solarsystem2' | 'location' |
'locationPreferred' | 'character'` session-gating, mapping to
`machoNet.GetNodeFromAddress(SERVICE_STATION|SERVICE_BEYONCE, id)`). For a
single-node private server, returning our own node id is sufficient.

### 1.4 Step 2 — bind

`Moniker.Bind(sess, call)` (`moniker.pyc`), remote case:

```python
remotesvc = sess.ConnectToRemoteService(self.__serviceName, self.__nodeID)
self.boundObject, retval = remotesvc.MachoBindObject(self.__bindParams, call)
self.boundObject.persistantObjectID = (self.__serviceName, self.__bindParams)
```

- `call` is `None`, or the tuple `(methodName, args, keywords)` of the call that
  triggered the bind — i.e. **bind and first call are fused into one round
  trip**. Keywords `'machoTimeout'` and `'noCallThrottling'` are stripped from
  `call[2]` before sending (`localKeywords` in `Bind`).
- The reply is a **2-tuple `(boundObject, retval)`**; `retval` is `None` when
  `call` was `None`. Confirmed on both sides: `moniker.Bind` unpacks it, and
  `CoreServiceCall.CallUp` builds `payload = (boundObject, retval)` /
  `(boundObject, None)` (`ServiceCallGPCS.pyc`).
- Server side of the same call (`ServiceCallGPCS.CallUp`, the
  `destinationMethod == 'MachoBindObject'` branch) does, in order:
  `bindParams = args[0]`; `nodeID = service.MachoResolveObject(bindParams)`;
  raise `WrongMachoNode(nodeID)` if that is not this node;
  `boundObject = service.MachoGetObjectBoundToSession(bindParams, session)`;
  then if `call` is not `None`, invoke `method` on the bound object.
- `CoreService.MachoGetObjectBoundToSession(bindParams, callerSession)`
  (`service.pyc`): under `LockedService(bindParams)`, look up a per-service
  cache, else `obj = self.MachoBindObject(bindParams)` and tag the raw object
  with `objectConnections = {}`, `sessionConnections = WeakValueDictionary()`,
  `machoInstanceID = wallclock`, `boundBy = bindParams`; then return
  `callerSession.ConnectToObject(obj, self.__servicename__, bindParams)`.
- `Session.ConnectToObject` (`basesession.pyc`) allocates a per-session
  monotonic `c2ooid` and wraps the object in an `ObjectConnection`. So what
  crosses the wire is the `ObjectConnection`, not the raw service object.
- `Moniker.MonikeredCall` retries once through `Unresolve()` on
  `UnMachoDestination` / `UserError('UnMachoDestination')`, and re-binds on an
  `UpdateMoniker(serviceName, bindParams, nodeID)` exception — the server's way
  of saying "that object moved" (`moniker.pyc`, `UpdateMoniker`).

### 1.5 What a bound-object id looks like on the wire

The reference is produced by the **marshal save callback**, not by a dedicated
packet field. `ObjectCall.ToPickle` calls `blue.marshal.Save(value,
self.GetObjectID)`; the native codec calls `GetObjectID` back for every
old-style instance it cannot serialise by value
(`ObjectCallGPCS.GetObjectID`, confirmed):

```python
objectID = basesession.GetObjectUUID(object)       # registers it in the session
refID    = blue.os.GetWallclockTime()              # first time only
pa = {k: getattr(object, k) for k in object.__publicattributes__ if hasattr(...)}
return blue.marshal.Save((objectID, pa, refID), self.GetObjectID) if pa \
  else blue.marshal.Save((objectID, refID), self.GetObjectID)
```

- **`objectID` is a string**, not an integer. `basesession.GetObjectUUID`:
  `'N=%s:%s' % (machoNet.GetNodeID(), counter)` on a server and
  `'C=0:%s' % counter` on a client, where `counter` is a process-global
  increment. It is cached on the object as `__machoObjectUUID__`.
- **`refID`** is `blue.os.GetWallclockTime()` at the moment the object was first
  added to the session, and is the reference-count token used for release.
- The reference therefore marshals as a **nested marshal stream containing
  `(objectID, refID)` or `(objectID, publicAttributes, refID)`**, emitted by the
  save callback. The client decodes it in `ObjectCall.ParseObjectID` with
  `blue.marshal.Load(objectID, self.ParseObjectID)`, accepting both the 2-tuple
  and 3-tuple forms, and builds a `MachoObjectConnection` for it.
- **Gap:** which numeric marshal tag the save/load callback mechanism uses is
  native (`blue.dll`), exactly like the rest of the opcode table — see
  `docs/archive/wire-format.md` §"Marshal blob". The *contents* of the callback payload
  are confirmed; the tag byte is not.

Bookkeeping that travels in `oob` (confirmed, `ObjectCallGPCS.pyc`):

| key | direction | value | set by |
|---|---|---|---|
| `OID+` | with any packet that introduced new references | `{objectID: refID}` | `ObjectCall.CallUp`/`CallDown`/`NotifyUp`/`NotifyDown`/`ObjectCallWithoutTheStars` from `self.added_objects` |
| `OID-` | client → server, release | `{objectID: maxRefID}` | `ObjectCall.__ReleaseObjectHelper` |

Release is a **Notification** (not a CallReq) whose method name is the literal
`'ClientHasReleasedTheseObjects'`, batched every 30 s by
`__ReleaseObjectHelper`, and handled in `ObjectCall.NotifyUp` by
`session.UnregisterMachoObject(objectID, refID)`. In that one case the
payload's first element is the `(objectID, refID)` *list*, not a single id.

### 1.6 How a subsequent method call is addressed

`MachoObjectConnection.__init__` (`ObjectCallGPCS.pyc`) parses the objectID and
fixes the destination **once**:

- `'N=<nodeID>:<oid>'` → `destination = MachoAddress(nodeID=long(nodeID))`, `blocking = 1`
- `'C=<clientID>:<oid>'` → `destination = MachoAddress(clientID=long(clientID))`, `blocking = 0`

Note what is **absent**: no `service` and no `callID` are attached. Per
`docs/archive/wire-format.md`, `MachoAddress.__getstate__` for NODE is
`(1, nodeID, service, callID)`, so a bound-object CallReq carries
`(1, nodeID, None, None)` — the *service* field that a plain service call uses
is empty, and the object is identified only by the `objectID` string inside the
payload.

`ObjectCall.ObjectCallWithoutTheStars` (`ObjectCallGPCS.pyc`) builds the body:

```python
payload = (1, self.ToPickle((objectID, method, args, kw2)))
packet  = CallReq(destination=destination, payload=payload)      # blocking == 1
# or
packet  = Notification(destination=destination, payload=payload) # blocking == 0
```

and the service-call layer builds, for comparison
(`CoreServiceCall.RemoteServiceCallWithoutTheStars` + `ObjectCall.CallDown`):

```python
payload = (1, method, args, kw2)                 # service-call layer
packet.payload = (0, self.ToPickle(packet.payload))   # object-call layer wraps it
```

**So the single discriminator on the wire is the first element of the CallReq /
Notification payload 2-tuple:**

| `payload[0]` | meaning | `payload[1]` (a nested marshal stream) decodes to |
|---|---|---|
| `0` | service call | `(1, method, args, kwargs)` — dispatched by `destination.service` |
| `1` | bound-object call | `(objectID, method, args, kwargs)` — dispatched by `objectID` |

Confirmed from both the send side above and the receive side
(`ObjectCall.CallUp`: `if packet.payload[0]: tup = FromPickle(payload[1]); objectID, method, args = tup[0], tup[1], tup[2]` else forward up to
`CoreServiceCall.CallUp`, which reads `packet.payload[1]` as the method name).
The 4th element (`kwargs`) is optional: `len(tup) == 3` is handled explicitly on
both layers.

Other confirmed details of the call envelope:

- `kw2['machoVersion'] = 1` is injected into **every** call's keywords unless an
  object-cache version is in play; the server strips it again
  (`ObjectCall.CallUp`: `if len(tup) == 4 and 'machoVersion' in tup[3]`, and if
  it was the only keyword the tuple is shortened back to 3 elements).
- `machoTimeout` is removed from the keywords and moved to `packet.oob['machoTimeout']`.
- Client calls go through `macho.ThrottledCall(key, ...)` keyed on
  `(objectID, method, str(args), str(kwargs))` unless the caller passes
  `noCallThrottling` (`MachoObjectCallWrapper.__call__`). `noCallThrottling` is
  consumed locally and never sent.
- Non-blocking calls return a `gpcs.MachoBoobyTrap` whose `__getattr__` raises —
  i.e. the client expects **no response at all** for `blocking == 0`.
- A `MachoObjectConnection` checks `gpcs.machoNet.GetClientSessionID() != self.clisid`
  and raises `'This object was acquired by a different session'`, and
  `PerformSessionCheck()` enforces the moniker's `SetSessionCheck` dict
  client-side before every call (e.g. `{'stationid': ...}`). Both are local
  guards; they cost the server nothing but mean a session change invalidates
  handles without any wire traffic.

### 1.7 Server-side dispatch rules the private server must mirror

From `ObjectCall.CallUp` (`ObjectCallGPCS.pyc`) and
`ServiceCallWrapper` (`service.pyc`), all confirmed:

1. `objectID not in session.machoObjectsByID` → `session.DumpSession('OBJECTNOTMEMBER', ...)` and raise `UnMachoDestination`.
2. `method.startswith('_')` or `method in object.__restrictedcalls__` → `RuntimeError('This call can only be the result of a hack attack', method)`.
   `basesession.ObjectConnection.__restrictedcalls__ = {'PseudoMethodCall', 'LogPseudoMethodCall', 'GetObjectConnectionLogClass', 'RedirectObject', 'Objectcast'}`.
3. For a non-`ObjectConnection` pass-by-reference object with a caller lacking `ROLE_SERVICE`: the method must be in `object.__exportedcalls__`, else `'%s call is not exported from this pass-by-reference object.'`.
4. `__exportedcalls__[method]` may be a list `[role, *preargNames]` or a dict `{'role':…, 'preargs':[…]}`. The role is checked against `session.role`; the `preargs` are **prepended to `args` from the session** (`args2[i] = getattr(session, preargs[i])`). A server reimplementation must reproduce this or client calls will arrive short by the implicit leading arguments.
5. Return value goes back as `packet.Response(payload=self.ToPickle(retval))` — a *bare* marshalled value at the object-call layer, with `oob['OID+']` if the return value itself contained new references.

**Gap:** the per-service `__exportedcalls__` tables for the bound objects live on
the server and are not in the client tree, so the implicit `preargs` and the
required role per bound method cannot be recovered from this build.

---

## 2. Bound services and their methods

### 2.1 Every moniker service name constructed by the client

From a bytecode scan for `Moniker(<const>, …)` over all 12,774 parseable
modules (constructor sites only), 29 service names:

`abyssal_content_manager`, `agentMgr`, `allianceRegistry`, `beyonce`,
`charMgr`, `corpRegistry`, `corpStationMgr`, `crimewatch`, `dogmaIM`, `entity`,
`fleetObjectHandler`, `i2`, `insuranceSvc`, `invbroker`, `jumpCloneSvc`,
`keeper`, `missionMgr`, `planetBaseBroker`, `planetMgr`,
`planetOrbitalRegistryBroker`, `populationCap`, `posMgr`, `repairSvc`,
`reprocessingSvc`, `ship`, `structure`, `tourneyMgr`, `warRegistry`,
`warStatisticMgr`.

Most are created by the factory functions in
`eve/common/script/net/eveMoniker.pyc`; the rest are constructed inline
(`charMgr` in `eve/client/script/environment/invCache.pyc`, `jumpCloneSvc` in
`eve/client/script/ui/services/clonejumpsvc.pyc`, `insuranceSvc` in
`eve/client/script/ui/station/insurance/base_insurance.pyc`, `repairSvc` in
`.../repairshop/base_repairshop.pyc`, `populationCap` in
`.../login/charSelection/characterSelection.pyc`, `structure` in
`.../menuSvcExtras/invItemFunctions.pyc`, and `i2` / `keeper` / `tourneyMgr` /
`abyssal_content_manager` in `eve/devtools/script/*`).

### 2.2 bindParams per service (confirmed)

`bindParams` is the server's object key; `Moniker` refuses `None`
(`"Thou shall not use None as a Moniker's __bindParams"`). All of the following
are from `eveMoniker.pyc` unless noted.

| service | bindParams | sessionCheck |
|---|---|---|
| `dogmaIM` | `(solarsystemID, groupSolarSystem)` or `(stationID, groupStation)` | `solarsystemid` / `stationid` |
| `ship` | same two forms | same |
| `invbroker` | `(solarsystemID, groupSolarSystem)` or `(stationID, groupStation)` | same |
| `crimewatch` | same two forms (`GetLocationBindParams`) | same |
| `jumpCloneSvc` | same two forms (`clonejumpsvc.pyc`) | — |
| `beyonce` | `solarsystemID` (scalar) | `solarsystemid` |
| `entity` | `session.solarsystemid2` | `solarsystemid` / `solarsystemid2` |
| `posMgr` | `session.solarsystemid` | `solarsystemid` |
| `charMgr` | `(session.charid, const.containerGlobal)` (`invCache.pyc`) | — |
| `reprocessingSvc` | `stationID` | `structureid` / `stationid` |
| `corpStationMgr` | `stationID` | `stationid` |
| `repairSvc` | `(locationID, parentLocationID, locationGroup)` | — |
| `insuranceSvc` | `(dockableLocation, locationID, locationGroup)` (`base_insurance.pyc`) | yes |
| `corpRegistry` | `corpID` | `corpid` |
| `allianceRegistry` | `(allianceID, isMaster)` | `allianceid` |
| `warRegistry` | `(allianceOrCorpID, isMaster)` | `corpid`/`allianceid` |
| `warStatisticMgr` | `warID` | — |
| `fleetObjectHandler` | `fleetID` | — |
| `agentMgr` | `agentID` | — |
| `missionMgr` | `('courier', stationID)` | — |
| `planetMgr` | `planetID` | — |
| `planetBaseBroker` | `planetID` | — |
| `planetOrbitalRegistryBroker` | `solarSystemID` | — |
| `populationCap` | `(charID, groupCharacter)` (`characterSelection.pyc`) | — |
| `tourneyMgr` | `session.solarsystemid` (`form_tournament.pyc`) | — |
| `abyssal_content_manager` | `session.solarsystemid2` (`abyssal_content_tool.pyc`) | — |
| `structure` | **gap** — module did not decompile cleanly | — |
| `i2`, `keeper` | **gap** (devtools) | — |

### 2.3 Method inventory

Produced by a bytecode taint scanner adapted from `tools/scan_remote.py`: it
seeds on `Moniker('<name>', …)`, on the `eveMoniker` factory functions, on the
attributes known from source to hold a moniker (`remoteDogmaLM`,
`remoteShipMgr`, `remoteBallpark`, `inventorymgr`, `stationInventoryMgr`,
`_invCache__invmgr`, `insurance`, `repairMgr`), and on the second-level
`GetInventory` / `GetInventoryFromId` results; then propagates the taint through
`STORE_FAST` / `STORE_ATTR` / `self.x = v` within a module and records every
**uppercase** `LOAD_ATTR` taken on a tainted value. Argument counts come from a
stack-depth simulation that locates the *matching* `CALL_FUNCTION`.

Reading the `args` column: `2` = two positional; `2+1kw` = two positional plus
one keyword; `+*a` / `+**kw` = `*args` / `**kwargs` at the call site;
`?` = the attribute was taken but not immediately called — almost always because
the bound method is passed as a callable to
`sessionMgr.PerformSessionChange(...)` / `PerformSelectiveSessionChange(...)` or
`uthread.*`, so the arity is in the caller's argument list rather than in a
`CALL_FUNCTION`. Several entries list more than one arity because the client
calls them from several sites with optional parameters.

Totals: **387 distinct method names across 22 bound services** — superseded,
see the correction at the end of this document. A reproducible scanner
(`tools/scan_bound.py`) finds **378 service/method pairs across 24 services**,
with the same 29 moniker names, and the difference is in both directions.

#### dogmaIM — 41 (ship/module state, targeting, fitting)

`eve/client/script/dogma/clientDogmaLocation.pyc` (`self.remoteDogmaLM`),
`eve/client/script/environment/godma.pyc` (`GetDogmaLM()`),
`eve/client/script/parklife/targetMgr.pyc`,
`eve/client/script/parklife/playerOwnedTargetMgr.pyc`,
`eve/client/script/ui/services/skillsvc.pyc`,
`eve/client/script/parklife/scanSvc.pyc`,
`eve/client/script/ui/services/menuSvcExtras/invItemFunctions.pyc`,
`eve/client/script/ui/control/entries/implant.pyc`.

| method | args | method | args |
|---|---|---|---|
| `GetAllInfo` | 3 | `ItemGetInfo` | 1 |
| `Activate` | 2, 4 | `Deactivate` | 2 |
| `SetModuleOnline` | 2 | `TakeModuleOffline` | 2 |
| `ShipOnlineModules` | 0 | `ChangeDroneSettings` | 1 |
| `AddTarget` | 1 | `RemoveTarget` | 1 |
| `AddTargetOBO` | 2 | `RemoveTargetOBO` | 2 |
| `RemoveTargets` | 1 | `ClearTargets` | 0 |
| `CancelAddTarget` | 1 | `GetTargets` | 0 |
| `GetTargeters` | 0 | `LaunchProbes` | 2 |
| `LoadAmmo` | 4, ? | `UnloadAmmo` | 3, 4, ? |
| `LinkWeapons` | 3 | `LinkAllWeapons` | 1 |
| `UnlinkModule` | 2 | `UnlinkAllModules` | 1 |
| `MergeModuleGroups` | 3 | `PeelAndLink` | 3 |
| `DestroyWeaponBank` | 2 | `Overload` | 2 |
| `StopOverload` | 2 | `OverloadRack` | 1 |
| `StopOverloadRack` | 1 | `InitiateModuleRepair` | 1 |
| `InitiateModuleRepairMany` | 1 | `StopModuleRepair` | 1 |
| `InjectImplant` | 1, ? | `DestroyImplant` | 1 |
| `InjectSkillIntoBrain` | 1 | `UseBooster` | 2 |
| `QueryAttributeValue` | 2 | `QueryAllAttributesForItem` | 1 |
| `FullyDescribeAttribute` | 3 | | |

Note for M2/M3: the modern equivalents of the old `ShipGetInfo` / `CharGetInfo`
are **`GetAllInfo(primeCharacter, primeShip, primeStructure)`** (three booleans)
and **`ItemGetInfo(itemKey)`**. The strings `ShipGetInfo`, `CharGetInfo`,
`DogmaLM` and `GetDogmaLocation` as a *remote* method do **not** occur anywhere
in this build's `.pyc` tree (`GetDogmaLocation` exists only as the local
`svc.clientDogmaIM` accessor, `eve/client/script/dogma/clientDogmaIM.pyc`).

#### invbroker — 15 (the inventory broker; hands out Inventory objects)

`eve/client/script/environment/invCache.pyc` (`self.inventorymgr`,
`self.stationInventoryMgr`), `.../menuSvcExtras/menuFunctions.pyc`,
`.../menuSvcExtras/invItemFunctions.pyc`,
`eve/client/script/ui/shared/inventory/plexVault.pyc`.

`GetInventory`(2) · `GetInventoryFromId`(2) · `ValidateItemCanBeOpened`(1) ·
`ValidateItemListCanBeOpened`(1) · `IsMyItemAtDockableLocation`(2) ·
`GetContainerContents`(2, 3) · `SplitStack`(4) · `SetLabel`(2) ·
`TrashItems`(2, ?) · `AssembleCargoContainer`(3) · `DeliverToCorpHangar`(6) ·
`DeliverToCorpMember`(6) · `GetDamageForCrystals`(1) ·
`DepositPlexToVault`(4, ?) · `WithdrawPlexFromVault`(4, 5)

`invbroker.GetItemDescriptor()` is a plain **service** call
(`sm.RemoteSvc('invbroker').GetItemDescriptor()` in `invCache.GetItemHeader`),
so it is already in `findings/remote-services.txt`; it supplies the DBRow
header for every item row below.

#### invbroker → Inventory (second-level bound object) — 32

`invbroker.GetInventory(containerID, param1)` and
`invbroker.GetInventoryFromId(itemID, passive)` each return **another bound
object** (`invCache.GetInventory` / `GetInventoryFromId`,
`eve/client/script/environment/invCache.pyc`). It is wrapped locally in
`invCacheContainer`, which (a) implements a few methods itself and (b) forwards
**every other uppercase attribute verbatim** to the bound object via
`invCacheContainer.__getattr__` → `FastInvCallWrapper`, through
`PasswordHandlerObjectWrapper` / `PasswordHandlerCallWrapper` (which retries the
same call after `EnterPassword(which, passw)` on `SCGeneralPasswordRequired` /
`SCConfigPasswordRequired`).

Remote methods the wrapper issues **itself**, read directly out of
`invCache.pyc` (these are the authoritative ones):

| wrapper method | remote call it makes |
|---|---|
| `invCacheContainer._SetItemData` | `GetSelfInvItem()` |
| `invCacheContainer.Add` | `Add(itemID, sourceID, **kw)` — `kw` seen: `qty`, `flag` |
| `invCacheContainer.MultiAdd` | `MultiAdd(list(itemIDs), sourceID, **kw)` |
| `invCacheContainer.StackAll` | `StackAll(*args, **kw)` — seen as `StackAll(flag)`, `StackAll()`, `StackAll(securityCode=…)` (`eve/client/script/environment/invControllers.pyc`) |
| `invCacheContainer.MultiMerge` | `MultiMerge(ops, sourceContainerID)` where `ops` is a list of `(sourceID, destID, qty)` |
| `invCacheContainer.List` | `List(flag=flag)` |
| `invCacheContainer.ListByFlags` | `ListByFlags(flags=uncachedFlags)` |
| `invCache.StripFitting` | `StripFitting()` |
| `invCache.AcceptPossibleRemovalTax` | `GetPriceForItems([itemID, …])` |

Forwarded method names observed at call sites across the tree:
`ALSCConfigGet`(0) · `ALSCConfigSet`(1) · `ALSCLockItems`(?) ·
`ALSCUnlockItems`(?) · `BreakPlasticWrap`(0) · `ClearCharges`(1) ·
`DestroyFitting`(1) · `GetAvailableTurretSlots`(0) · `HasExistingPasswordSet`(1) ·
`ImportExportWithPlanet`(4) · `ListFighterBay`(0) · `Reprocess`(1) ·
`RetrievePassword`(1) · `SetPassword`(3) · `QALockItem`(?) · `QAUnlockItem`(?).

**Caveat, important:** the scanner also attributed `GetCapacity`, `GetItem`,
`ListCargo`, `ListDroneBay`, `ListHardwareModules` and
`InvalidateStationItemsCache` to this object, but reading `invCache.pyc` shows
those are **local** `invCacheContainer` / `inv5MinCacheContainer` methods that
do *not* produce a remote call (`ListCargo`/`ListDroneBay`/`ListHardware`/
`ListHardwareModules` are `self.List(flag)` filters; `GetCapacity` is computed
from dogma + `self.List`; `GetItem` returns the cached `self.item`). Do not
implement them server-side on the strength of this scan.

#### charMgr → global asset container — 5

`invCache.GetInventory(const.containerGlobal)` returns
`Moniker('charMgr', (session.charid, const.containerGlobal))` wrapped in
`inv5MinCacheContainer` (`eve/client/script/environment/invCache.pyc`). Its
remote calls, read from that class:

`ListStations()` · `ListStationItems(stationID)` · `List()` ·
`ListIncludingContainers()` · `GetAssetWorth()` (preceded by an explicit
`Bind()`).

These five were lumped into the Inventory bucket by the scanner because the
client reaches both through `invCache`; the split above comes from reading the
source and is the one to trust.

#### ship — 26 (boarding, undocking, jettison, scoop)

`eve/client/script/dogma/clientDogmaLocation.pyc` (`self.remoteShipMgr`),
`eve/client/script/parklife/gameui.pyc` (`GetShipAccess()`),
`.../menuSvcExtras/menuFunctions.pyc`, `.../ui/station/base.pyc`,
`.../ui/shared/dockedUI/undockingSvc.pyc`,
`eve/client/script/environment/fittingSvc.pyc`.

`Board`(?) · `BoardStoredShip`(?) · `LeaveShip`(1) · `Eject`(?) · `Undock`(?) ·
`AssembleShip`(1, 2, +kw) · `FitShips`(?) · `StoreVessel`(?) · `Jettison`(1) ·
`Drop`(3) · `LaunchDrones`(3) · `LaunchFromContainer`(2) · `Scoop`(1, 2) ·
`ScoopDrone`(1) · `ScoopToSMA`(1) · `ScoopToFleetHangar`(1) ·
`ScoopToMobileDepotHold`(1) · `ScoopToInfrastructureHold`(1) ·
`SelfDestruct`(?) · `AbortSelfDestruct`(?) · `SafeLogoff`(0) ·
`AbortSafeLogoff`(0) · `SetShipPassword`(1) · `GetShipConfiguration`(1) ·
`GetDirtTimestamp`(1) · `ResetDirtTimestamp`(1)

The `?` arities for the critical ones are pinned by reading the call sites:

- `Board(shipID, session.shipid or session.stationid)` — 2 args
  (`.../menuSvcExtras/menuFunctions.pyc::Board`), also
  `Board(shipID, session.shipid)` via
  `PerformSelectiveSessionChange('board', 'board', False, self.remoteShipMgr.Board, …)`
  (`clientDogmaLocation.pyc::_MakeShipActive`).
- `BoardStoredShip(structureID, shipID)` — 2 args (`menuFunctions.pyc::BoardSMAShip`).
- `Undock(shipID, ignoreContraband, onlineModules=onlineModules)` — 2 positional
  + 1 keyword (`eve/client/script/ui/station/base.pyc::_DoUndockAttempt`).

#### beyonce — 38 (space/movement/warp, the ballpark)

`eve/client/script/remote/michelle.pyc` holds the handle:
`Park.remoteBallpark = eveMoniker.GetBallPark(self.solarsystemID)` and
`Michelle.GetRemotePark()` returns `self.__bp.remoteBallpark`.
`svc.michelle` itself is a **local** client service wrapping the native
`destiny.Ballpark` (`blue.classes.CreateInstance('destiny.Ballpark')`), so
`michelle.GetBall`/`GetBalls`/`GetBallsAndSlimItems`/`GetSlimItem`/`GetItem`
read the **local** ballpark and are *not* wire calls. Only `remoteBallpark`
crosses the wire.

`CmdDock` and `CmdTurboDock` had `(?)` here until `docs/lanes/dock-up.md` §1
read them out of `movementFunctions.RealDock`: `CmdDock(stationID, shipID)`
and `CmdTurboDock(stationID)`. Neither has a row in
`findings/client-calls.tsv` and that is a **census blind spot**, not an
absence -- both are passed to `sessionMgr.PerformSessionChange` as unbound
methods, so no `CALL_FUNCTION` follows the `LOAD_ATTR` and the scanner sees
nothing. `docs/lanes/census-blindspots.md` has the pattern.

Movement / commands: `CmdStop`(0) · `CmdFollowBall`(2) · `CmdOrbit`(2) ·
`CmdAlignTo`(+2kw) · `CmdGotoDirection`(3) · `CmdGotoBookmark`(1) ·
`CmdSteerDirection`(*args) · `CmdSetSpeedFraction`(1) ·
`CmdWarpToStuff`(2, +kw) · `CmdWarpToStuffAutopilot`(1) · `CmdDock`(2) ·
`CmdTurboDock`(1) · `CmdStargateJump`(?) · `CmdJumpThroughFleet`(?) ·
`CmdJumpToFleetModuleBeacon`(?) · `CmdJumpToFleetDeployableBeacon`(?) ·
`CmdGroupJumpToFleetModuleBeacon`(?) · `CmdGroupJumpToFleetDeployableBeacon`(?) ·
`CmdBridgeToFleetModuleBeacon`(3) · `CmdBridgeToFleetDeployableBeacon`(?) ·
`CmdFleetRegroup`(0) · `CmdFleetTagTarget`(2) · `CmdAbandonLoot`(1) ·
`CmdSelfDestructStructure`(1) · `CmdAbortSelfDestructStructure`(1) ·
`CmdTransferCorpAssetOwnership`(3).

State / queries: `UpdateStateRequest`(0) — polled by
`Park._UpdateStateRequestThread`, this is the destiny state pump ·
`GetBall`(1) · `GetSurfaceDist`(2) · `GetRelativity`(6) · `GetFleetState`(0) ·
`GetFleetLocationAndInfo`(0) · `GetContainerExpiryDate`(1) ·
`GetCapacityOfSiphon`(1) · `GetOperationSpawnpointPosition`(1) ·
`BookmarkLocation`(5+1kw) · `BookmarkScanResult`(6+1kw) ·
`CallComponentFromClient`(3, 5, +*a/**kw — the generic space-component RPC used
all over `spacecomponents/client/components/*`).

Also confirmed service (not bound) calls on `beyonce`, already covered by the
old scan: `sm.RemoteSvc('beyonce').GetFormations()` and
`AdminGetBubbleID(itemID)` (`michelle.pyc`).

#### entity — 16 (drones and NPC/behaviour control)

`eveMoniker.GetEntityAccess()` / `GetEntityLocation()`.
`CmdEngage`(2) · `CmdAssist`(2) · `CmdGuard`(2) · `CmdReturnBay`(1) ·
`CmdReturnHome`(1) · `CmdAbandonDrone`(1) · `CmdReconnectToDrones`(1) ·
`CmdMineRepeatedly`(2) · `CmdSalvage`(2) · `CmdUnanchor`(2) ·
`OpenShippingUnitStorage`(1) · `GetBehaviorTreeManager`(0) ·
`GetExtraFleetMembers`(1) · `GetDebugSpawnPoolFleetEnemyClusters`(1) ·
`EnableBehaviorDebugging`(1) · `DisableBehaviorDebugging`(1).

`GetBehaviorTreeManager()` returns yet another bound object; its method set was
not determined (see gaps).

#### Remaining bound services

| service | n | methods |
|---|---|---|
| `corpRegistry` | 78 | full list in the scanner output; e.g. `GetCorporation`(0), `GetMember`(1), `GetMembersPaged`(1), `GetMembersByIds`(1), `GetTitles`(0), `UpdateTitle`(10), `UpdateMember`(15), `UpdateDivisionNames`(14), `AddCorporation`(15), `CreateAlliance`(4), `GetShareholders`(1), `MoveCompanyShares`(3), `GetBulletins`(0), `AddBulletin`(2), `GetApplications`(0), `InsertApplication`(2), `KickOutMember`(1), `GetCorporateContacts`(0) … Sources: `eve/client/script/ui/services/corporation/base_corporation.pyc`, `bco_*.pyc`, `eve/client/script/ui/shared/neocom/corporation/*` |
| `fleetObjectHandler` | 35 | `Init`(2+1kw), `GetInitState`(0), `GetFleetID`(0), `GetFleetComposition`(0), `GetWings`(0), `CreateWing`(0), `DeleteWing`(1), `ChangeWingName`(2), `CreateSquad`(1), `DeleteSquad`(1), `ChangeSquadName`(2), `MoveMember`(4), `MassMoveMembers`(4), `FinishMove`(?), `KickMember`(1), `MakeLeader`(1), `LeaveFleet`(0), `DisbandFleet`(0), `AcceptInvite`(?→1: shipTypeID), `RejectInvite`(0,1), `GetJoinRequests`(0), `RejectJoinRequest`(1), `Reconnect`(0), `GetMotd`(0), `SetMotdEx`(1), `SetOptions`(1), `SetFleetMaxSize`(1), `GetFleetMaxSize`(0), `SetAutoJoinSquadID`(1), `SetTakesFleetWarp`(1), `SetAcceptsRegroupValue`(1), `SetAcceptsConduitJumpsValue`(1), `SendBroadcast`(4), `UpdateMemberInfo`(1), `LoadFleetSetup`(1) — `eve/client/script/parklife/fleetSvc.pyc` |
| `planetMgr` | 35 | `GetColony`(1), `GetNewColony`(1), `GetColonyByPinID`(1), `GetPin`(1), `RemovePin`(1), `GetCommandPinsForPlanet`(1), `GetCommandCenterForCharacter`(1), `GetCommandCenterLevel`(1), `GetExtractorsForPlanet`(1), `MoveExtractorHead`(4), `InstallProgram`(3), `CancelInstallProgram`(2), `GetProgramResultInfo`(4), `GetResourceData`(1), `GetPlanetResourceInfo`(0), `GetPlanetInfo`(0), `GetPlanetRadius`(0), `GetFullNetworkForOwner`(2), `UserUpdateNetwork`(1), `UserLaunchCommodities`(2), `UserTransferCommodities`(2), `UserAbandonPlanet`(0), `GetEditModeData`(0), `IsInEditMode`(0), `GetCostOfCurrentEdits`(0), `RevertChanges`(0), `StartTicking`(0), `GetLocalDistributionReport`(1), + 7 `GM*` — `eve/client/script/ui/shared/planet/*`, `eveplanet/*` |
| `posMgr` | 20 | `AnchorStructure`(2), `AnchorOrbital`(1), `UnanchorOrbital`(1), `OnlineOrbital`(1), `AssumeStructureControl`(1), `RelinquishStructureControl`(1), `ChangeStructureProvisionType`(3), `GetStarbasePermissions`(1), `SetStarbasePermissions`(3), `GetTowerNotificationSettings`(1), `SetTowerNotifications`(3), `GetTowerSentrySettings`(1), `SetTowerSentrySettings`(6), `SetTowerPassword`(2,4), `GetMoonForTower`(1), `GetMoonProcessInfoForTower`(1), `RunMoonProcessCycleforTower`(1), `LinkResourceForTower`(?), `GetConsumptionFactor`(1), `GetSiloCapacityByItemID`(1) |
| `jumpCloneSvc` | 15 | `GetCloneState`(0), `GetStationCloneState`(0), `GetShipCloneState`(0), `CloneJump`(?), `InstallCloneInStation`(0), `InstallCloneInStructure`(0), `ValidateInstallJumpClone`(0), `GetPriceForClone`(0), `DestroyInstalledClone`(1), `SetJumpCloneName`(2), `OfferShipCloneInstallation`(1), `AcceptShipCloneInstallation`(0), `CancelShipCloneInstallation`(0), `GetNumClonesInPilotsStructure`(0), `ResetLastCloneJumpTime`(0) — `eve/client/script/ui/services/clonejumpsvc.pyc` |
| `crimewatch` | 8 | `GetClientStates`(0), `GetMySecurityStatus`(0), `GetCharacterSecurityStatus`(1), `GetSecurityStatusTransactions`(0), `SetSafetyLevel`(1), `ApplyTagsForSecStatus`(2), `StartDuelChallenge`(1), `RespondToDuelChallenge`(3) — `eve/client/script/ui/services/crimewatchSvc.pyc` |
| `insuranceSvc` | 5 | `GetContracts`(0,1), `GetInsurancePrice`(1), `GetInsurancePrices`(1), `InsureShip`(3,+1kw), `UnInsureShip`(1) |
| `tourneyMgr` | 4 | `GetMatchSystem`(0), `GetActiveMatchForCamera`(0), `GetTeamMembers`(1), `GetFancyDetails`(1) |
| `abyssal_content_manager` | 3 | `GenerateAbyssEncounterContent`(6), `GenerateContentFromExistingPlan`(1), `GetContentDetailsForShipLocation`(0) |
| `repairSvc` | 3 | `GetRepairQuotes`(1), `RepairItemsInStation`(2), `RepairItemsInStructure`(1) |
| `planetOrbitalRegistryBroker` | 3 | `GetTaxRate`(1), `GetSettingsInfo`(1), `UpdateSettings`(7) |
| `warStatisticMgr` | 3 | `GetBaseInfo`(0), `GetKills`(2), `GetKillsByGroup`(0) |
| `reprocessingSvc` | 2 | `GetQuotes`(2), `Reprocess`(5) |
| `populationCap` | 2 | `GetCharacterLoadSlot`(1), `MoveCharacterToNewSystem`(3) |
| `corpStationMgr` | 2 | `MoveCorpHQHere`(0), `DoStandingCheckForStationService`(1) |
| `structure` | 1 | `JettisonStructureFuel`(?) |

One scanner artefact to ignore: `UnBind`(0) under `corpRegistry` comes from a
local wrapper method in
`eve/client/script/ui/services/corporation/bco_base.pyc`, not from the moniker's
own `Unbind`; treat it as unverified.

Services with constructor sites but **no** observed method calls in this build:
`allianceRegistry`, `warRegistry`, `keeper`, `i2`.

> **Correction, 2026-10-20.** This list used to include `agentMgr`,
> `missionMgr` and `planetBaseBroker`, and called `GetAgent`,
> `GetCourierMissionCreator` and `GetPlanetBaseManager` dead code with "**no**
> references anywhere else in the tree". **That was wrong, and badly so for
> `agentMgr`:** `eveMoniker.GetAgent` has two call sites and **fourteen**
> methods are called on the resulting moniker from six modules, including
> `DoAction` — which turns out to be the only mutator in the entire agent and
> mission subsystem. `missionTrackerMgr` was likewise recorded with zero
> methods and has four.
>
> The scanner missed them for the reason §4.9 of this document predicts:
> `GetAgentMoniker` stores the moniker **in a dict**, so the handle is
> laundered through a container and the method calls are never connected to the
> service. A generalisable grep for the related shape is
> `self.<name> = sm.RemoteSvc('<name>')` inside a `Run`/`Initialize`.
>
> So read "no observed method calls" here as "none the scanner could reach",
> never as "none exist". Found by the `agents-missions` lane; the call surface
> is enumerated in [`re/agents-missions.md`](../re/agents-missions.md) and
> `findings/shapes-agents-missions.tsv`. The same blind spot was measured
> independently by the `census-blindspots` lane at **9 of 29** methods on one
> bound object, which is the scale of what this sentence was hiding.

---

## 3. Return shapes on the critical path to "docked with a fitted ship"

Only the handful that M2/M3 needs, each read off a specific client module.

### 3.1 The item DBRow (`Inventory.List`, `GetSelfInvItem`, every `OnItemChange`)

Header comes from the service call `invbroker.GetItemDescriptor()`
(`invCache.GetItemHeader`); the client wraps rows as
`CRowset(self.broker.GetItemHeader(), [])` in `invCacheContainer.List`. Column
order is fixed by the `ix*` constants — `carbon/common/lib/const.pyc` for 0-5
and `inventorycommon/const.pyc` for 6-10:

| idx | constant | attribute read by the client |
|---|---|---|
| 0 | `ixItemID` = 0 | `itemID` |
| 1 | `ixTypeID` = 1 | `typeID` |
| 2 | `ixOwnerID` = 2 | `ownerID` |
| 3 | `ixLocationID` = 3 | `locationID` |
| 4 | `ixFlag` = 4 | `flagID` |
| 5 | `ixQuantity` = 5 | `quantity` |
| 6 | `ixGroupID` = 6 | `groupID` |
| 7 | `ixCategoryID` = 7 | `categoryID` |
| 8 | `ixCustomInfo` = 8 | `customInfo` |
| 9 | `ixStackSize` = 9 | `stacksize` |
| 10 | `ixSingleton` = 10 | `singleton` |

Confirmed consumers: `invCache.OnItemChange` (reads `itemID`, `typeID`,
`ownerID`, `locationID`, `flagID`, `quantity`, `stacksize`, `singleton`,
`groupID`), `invCacheContainer._ProcessItemChange`,
`eve/client/script/ui/services/inventorysvc.pyc::OnItemChange` (uses
`item.__columns__[k]`, i.e. the header's column names must match these
attribute names exactly), and `invCacheContainer.List`, which **rejects rows
whose length differs from the header** (`if len(item) != len(res.header):
LogError('Invalid Item')`). An `OnItemChange` notification's `change` dict is
keyed by these same integer indices.

`Inventory.List(flag=None)` returns an iterable of these rows (the client
re-wraps them); `flag=None` means "everything". `Inventory.ListByFlags(flags=[…])`
likewise. `Inventory.GetSelfInvItem()` returns **one** such row — the client
reads `.itemID` and `.typeID` off it (`invCacheContainer._SetItemData`).

### 3.2 `dogmaIM.GetAllInfo(primeCharacter, primeShip, primeStructure)`

Three booleans in; the result is consumed by
`eve/client/script/environment/godma.pyc::ProcessAllInfo`, which reads exactly
these attributes:

- `.structureInfo` — falsy or a **dict** of contents for `session.structureid`
- `.shipInfo` — falsy or a dict of contents for `session.shipid`
- `.charInfo` — falsy or an **indexable** whose `[0]` is the contents dict for `session.charid`
- `.shipModifiedCharAttribs` — falsy, or an info object passed straight to `RefreshItemAttributes(session.charid, info=…)`, so it must have the `.attributes` / `.time` / `.wallclockTime` shape of §3.3
- `.systemWideEffectsOnShip` — falsy or an opaque payload, scattered as `OnUpdateSystemWideEffectsInfo`

Each "contents" dict is `{key: row}` and is consumed by
`godma.PrimeLocation` → `UpdateItem(row.invItem, row)`. So every value must
carry:

- `.invItem` — an item DBRow as in §3.1
- plus the `ItemGetInfo` fields of §3.3 (`.itemID`, `.attributes`, `.activeEffects`, `.time`, `.wallclockTime`)

The dict key is the item id, or for a sub-location (charges in a module slot)
the tuple `(locationID, flagID, typeID)` — `godma.UpdateItem` branches on
`IsSubLocation` and then unpacks `itemLocationID, itemFlag, itemTypeID =
itemKey`.

### 3.3 `dogmaIM.ItemGetInfo(itemKey)`

Consumed by `godma.RefreshItemAttributes` and `godma.RefreshItemEffects`. Read
attributes:

- `.itemID`
- `.time`, `.wallclockTime` — `wallclockTime` gates stale attribute changes in `_IsAttributeChangeRelevant`
- `.attributes` — dict `{attributeID: value}`, iterated with `iteritems()`
- `.activeEffects` — dict `{effectID: d}` where `d` is indexable with at least
  10 elements: `d[:7]` is the effect *environment*
  (`ENV_IDX_SELF`=0, `CHAR`=1, `SHIP`=2, `TARGET`=3, `OTHER`=4, `AREA`=5,
  `EFFECT`=6, and `ENV_IDX_LINE`=7 is the slice bound), then
  `d[7]` = `startTime`, `d[8]` = `duration`, `d[9]` = `repeat`
  (all from `godma.pyc`, constants at module level)

### 3.4 `ship.Board(shipID, oldShipIDOrStationID)`

Return value is unpacked as a **4-tuple** in
`clientDogmaLocation.pyc::_MakeShipActive`:

```python
self.instanceCache, self.instanceFlagQuantityCache, self.wbData, heatStates = shipState
```

`self.wbData` is then handed to `SetWeaponBanks(shipID, self.wbData)`. The call
runs inside `sessionMgr.PerformSelectiveSessionChange('board', 'board', False,
self.remoteShipMgr.Board, shipID, session.shipid)`, so the server is also
expected to produce the session change for `shipid` as part of it.

### 3.5 `ship.Undock(shipID, ignoreContraband, onlineModules=…)`

`eve/client/script/ui/station/base.pyc::_DoUndockAttempt`. Invoked through
`sessionMgr.PerformSessionChange('undock', shipsvc.Undock, shipID,
ignoreContraband, onlineModules=onlineModules)`, where `onlineModules` comes
from the **local** `clientDogmaIM.GetDogmaLocation().GetOnlineModules(shipID)`.
Return value is not used; the client reacts to the session change. Named
`UserError`s it handles specially: `'ShipNotInHangar'` (expects
`e.dict['capsuleID']`) and `'ShipContrabandWarningUndock'` (expects a message
dict; on confirmation the client retries with `ignoreContraband=True`).

### 3.6 Neighbours worth knowing

- `invbroker.GetInventory(containerID, param1)` / `GetInventoryFromId(itemID, passive)`
  → a **bound object reference** (§1.5), not data. `containerID` is one of the
  `const.container*` pseudo-ids (`containerHangar`, `containerGlobal`,
  `containerCorpMarket`, `containerCapsuleerDeliveries`, `containerWallet`,
  `containerStationCharacters`, `containerSolarSystem`, `containerRecycler`,
  `containerCharacter`, `containerStructure`), `param1` is an owner id or `None`
  (`invCache.GetInventory`).
- `Inventory.GetCapacity` is **not** remote — the client computes capacity from
  dogma attributes and `List` (`invCacheContainer.GetCapacity`). The server
  only has to get the dogma capacity attributes right.
- `charMgr.ListStations()` / `ListStationItems(stationID)` back the assets
  window; results are cached 5 minutes client-side (`inv5MinCacheContainer`).
- `beyonce.UpdateStateRequest()` (0 args) is the destiny state pump driven by
  `Park._UpdateStateRequestThread`; the state itself arrives through
  `michelle.DoDestinyUpdate(state, waitForBubble, dogmaMessages=[])` /
  `DoDestinyUpdates(updates)` as *notifications*, and is applied to the native
  ballpark. Its binary layout is native and out of scope here.

---

## 3a. Implemented — the first bound object **[V]**

`populationCap`, bound when a character is clicked, is the first of these the
server actually answers. See `docs/reference/login-path.md` §8 for the call sequence and the
three constraints `ParseObjectID` puts on an object id. The pieces:

| piece | where |
|---|---|
| tag `0x19`, read and written | `eve_marshal::PyValue::Callback` |
| the `(objectID, refID)` reference | `eve_proto::object_reference` |
| `MachoBindObject`, fused first call, reply `(boundObject, retval)` | `eve_server::dispatch::handle_bind` |
| `MachoResolveObject` → our node id, for **any** service | `eve_server::service::Registry::dispatch`, before the service match |
| calls **on** a handed-out object | `eve_server::dispatch::handle_bound_call` |
| the whole round trip over a socket | `tests/loopback.rs`, `a_bind_resolves_a_node_then_answers_with_an_object_and_the_fused_call` |

### The second half: calls *on* the object

Handing out a reference is only half of it, and the real client made the
other half obvious. For `warRegistry` it bound with **no fused call** and then
called `GetWars` on the reference:

```text
[ 87] OK   warRegistry   MachoBindObject((1000001, 1), None)  -> (<callback ("N=1:2", ...)>, None)
[ 89] HANG warRegistry*  GetWars [N=1:2](1000001)
```

That second packet is addressed to the **node** and names the object only by
its id string — there is no service field — so it cannot be dispatched
without a map from id back to service. `GpsSession::bound_objects` is that
map, recorded as each object is minted, and `handle_bound_call` does the
routing. Before it existed, such a call showed up in a capture as a call to
service `?`, which is exactly as informative as it sounds; `evecap --calls`
now resolves the id back to its owner and marks the line with a `*`.

Two simplifications, both deliberate and both recorded so they are not
mistaken for the protocol:

- **One object per service.** `handle_bind` dispatches the fused call against
  the service name, ignoring `bindParams`, so a bound method is an ordinary
  arm in `service.rs`. `bindParams` *is* stored beside the service name in
  `bound_objects`, both for the log line and because it is what will
  distinguish two objects of one service when we need that. A real cluster
  binds a distinct object per `bindParams`.
- **No `OID+` in `oob`.** The real server announces new references there. The
  client only ever writes that key — inbound registration happens inside
  `ParseObjectID` — so we omit it until release tracking matters. **[I]**

## 4. Gaps

1. **Marshal tag for a callback-encoded object reference.** The *contents* of
   the reference (`(objectID, refID)` / `(objectID, pa, refID)`, §1.5) are
   confirmed from `ObjectCallGPCS.GetObjectID` / `ParseObjectID`, and the tag
   is **`0x19`**, whose mechanism is settled from `blue.dll`
   (`docs/archive/marshal-tags.md` Gap 9: the Save callback is consulted only in the `0x17`
   branch, and a non-`None` return emits `0x19` followed by the returned
   object). Implemented on that basis — see §3a.
   **The client's reader is now confirmed [V]**, and better than by
   inference: its own log prints the object it built from our reference.

   ```text
   crimewatch::MachoBindObject args=((60000004, 15), ('GetClientStates', (), {})),
     retval=(<RemoteObject:N=1:5>, (((0, None), ...), [], ([], []), 0))

   invbroker::MachoBindObject args=((60000004, 15), ('GetInventoryFromId', (90000001, 1), {})),
     retval=(<RemoteObject:N=1:4>, <RemoteObject:N=1:3>)
   ```

   `<RemoteObject:N=1:5>` is a `MachoObjectConnection` that `ParseObjectID`
   constructed out of a `0x19` we emitted, naming the id we minted. The
   second line shows two of them in one reply — the bound object and the
   inventory its fused call returned. The client then called methods on
   them, which is the other half.

   What remains strictly unknown is only whether CCP's *writer* also picks
   `0x19` for references. This said a capture could not settle it, "because
   references travel server→client and we are the server" -- **and that is
   no longer true of us.** `eve-probe` is a client of ours, and the
   `evejs-probe` lane pointed it at a third-party server: a server wrote the
   `0x19` reference form and our own reader took it, with the bound object's
   methods then called on it (`docs/lanes/evejs-probe.md`).
   
   So the unknown has narrowed from "is this what a server writes" to "is
   this what *CCP's* server writes", which is a much smaller question and one
   the reader makes academic. The general lesson is worth more than the
   promotion: **owning a client turns every server→client claim from
   unobservable into testable**, and there is a drawer of them in this
   document and in `docs/archive/marshal-tags.md`.
2. **`MachoResolveObject` bodies.** Not in the client tree; only the
   `__machoresolve__` session-gating helper (`service.pyc`
   `_MachoResolveAdditional`) is. For one node, return our own node id —
   which is what we now do.
3. **`MachoBindObject` bodies / the bound object classes.** Entirely
   server-side. The client never sees the server's class names, so the
   historical names `InventoryBound` / `DogmaLM` / `DogmaLocation` could **not**
   be confirmed from this build — only the *service* names (`invbroker`,
   `dogmaIM`, `ship`, `beyonce`, …) and the method names above are evidence.
   `InventoryBound` does not occur as a string anywhere in `out/pyc/`, and
   `DogmaLM` occurs only as a fragment of *client-local* identifiers
   (`GetDogmaLM`, `SetDogmaLM`, `clientDogmaLM`, `remoteDogmaLM`,
   `fittingDogmaLM`, `indirectDogmaLM`) — never as a class or service name.
4. **`__exportedcalls__` for bound objects**, hence the per-method required role
   and the implicit session-derived `preargs` that `ObjectCall.CallUp` prepends
   to `args` (§1.7 item 4). Server-side only. This is the single biggest
   remaining unknown for getting arities exactly right: a method the client
   calls with *n* arguments may be declared with *n + len(preargs)* parameters.
5. **Arities marked `?`.** Where a bound method is handed to
   `PerformSessionChange` / `PerformSelectiveSessionChange` / `uthread.*` as a
   callable, there is no `CALL_FUNCTION` to measure. The three that matter
   (`Board`, `BoardStoredShip`, `Undock`) are pinned by hand in §2.3/§3; the
   rest (`Eject`, `SelfDestruct`, `StoreVessel`, `FitShips`, `CloneJump`,
   `CmdDock`, `CmdTurboDock`, `CmdStargateJump`, the cyno `Cmd*` family,
   `TrashItems`, `GetPriceForItems`, `ALSC*Items`, `QA*LockItem`,
   `JettisonStructureFuel`, `FinishMove`, `LinkResourceForTower`) are not.
6. **Second-level bound objects beyond Inventory.** `entity.GetBehaviorTreeManager()`
   returns a bound object whose method set was not enumerated; the generic
   `beyonce.CallComponentFromClient(...)` is a tunnel into
   `spacecomponents/*` server components whose method namespace is carried in
   the arguments rather than in the method name, so it is not enumerable this
   way either.
7. **Services with no observed calls**: `allianceRegistry`, `warRegistry`,
   `keeper`, `i2` have `Moniker` constructor sites but no method call sites the
   scanner could reach. `agentMgr`, `missionMgr` and `planetBaseBroker` were in
   this list and should not have been — see the correction in §2. Still,
   `MachoResolveObject`/`MachoBindObject` should exist for them if the client
   ever constructs the moniker.
8. **`structure` bindParams** — the one module that constructs
   `Moniker('structure', …)`
   (`eve/client/script/ui/services/menuSvcExtras/invItemFunctions.pyc`) did not
   yield that line to `uncompyle6`; only the bytecode-level constant is
   confirmed.
9. **Taint-scanner blind spots.** Methods invoked on a moniker that is stored in
   a list/dict/tuple, passed across module boundaries as a function argument, or
   reached via a `__getattr__` chain the scanner does not model, are missed.
   `gameui.GetShipAccess` (which stores the moniker in a *list*,
   `eve/client/script/parklife/gameui.pyc`) is a worked example of the pattern:
   it had to be seeded by hand. Treat the 387 as a floor, not a ceiling.
10. **Client-hosted bound objects.** `'C=<clientID>:<oid>'` references mean the
    *server* can hold a handle to a client object and call it (non-blocking,
    `Notification` only — `MachoObjectConnection.__init__` sets `blocking = 0`).
    Which client objects are published this way was not enumerated; the
    `__passbyvalue__ = 0` / `__publicattributes__` machinery in
    `basesession.pyc` is the place to start.


---

## Correction: the numbers are now reproducible, and were wrong in both directions

The 387/22 figure above was produced by hand and the script that produced it
was never landed, so it could not be re-derived or checked against another
build. `tools/scan_bound.py` now does it from the client's bytecode:

```sh
tools/analysis.sh python3 tools/scan_bound.py -o findings/bound-objects.json \
    --compare docs/reference/bound-objects.md
```

~105 s over all 12,774 modules, byte-identical across runs. Result: the **same
29 moniker service names** — no additions, no omissions — but **378
service/method pairs across 24 services**.

### Where this document undercounted

- **`tourneyMgr`: 34, not 4.** `eve/devtools/script/tournamentRefereeTools.py`
  was missed entirely. `TournamentRefereeTool.StartMatch` does
  `self.matchMoniker = eveMoniker.Moniker('tourneyMgr', ...)` and then calls
  `AbortCountdown`, `CreateMatch`, `TeleportToArena` and the rest on it.
- **`allianceRegistry` (27), `warRegistry` (19) and `i2` (2) do have observed
  method calls.** They are listed above under "constructor sites but no
  observed method calls", which is wrong. The calls go through a
  `GetMoniker()` indirection the hand pass did not follow — e.g.
  `base_corporation.Corporation.DeleteBulletin` is
  `sm.GetService('alliance').GetMoniker().DeleteBulletin(id)`.
- **`structure`'s bindParams** is listed as a gap ("module did not decompile
  cleanly"). It is `session.structureid`, in
  `invItemFunctions.JettisonStructureFuel`.

### Where this document overcounted — `planetMgr` is 19, not 35

**This one is a real error, not a scanner limitation**, and it is worth being
precise about because the inflated list reads like available server API.

Sixteen of the 35 names are methods of the client's **own** `ClientPlanet` /
`BasePlanet` / `BasePin` / `ColonyData` objects, not of the moniker. Spot-checked
here rather than taken on report:

| method | actually defined in | what it does |
|---|---|---|
| `GetColony` | `eve/common/script/planet/basePlanet.py` | returns `self.colonies[...]` |
| `IsInEditMode` | `.../planet/entities/basePin.py` | returns `self.inEditMode` |
| `RemovePin` | `.../environment/planet/clientPlanet.py` | validates and mutates local colony data |
| `GetPlanetRadius` | `.../planet/baseColony.py` | returns `self.radius` |

A server that implemented those would be implementing calls the client never
makes. The genuine 19 — `UserUpdateNetwork`, `GetPlanetInfo`, the `GM*` family
and so on — are all in the scanner's set.

### Where the scanner sees less, and why

- **`corpRegistry`: 45 vs 78.** A genuine blind spot. The `bco_*.py` objects
  call `self.GetCorpRegistry()`, which resolves through
  `self.boundObject` — a back-pointer to a `Corporation` instance in another
  module. Neither the import graph nor an attribute store reveals its type.
  Not fixable without class and field typing.
- **`charMgr`: 0 vs 5.** Deliberate. An earlier iteration produced 12, but the
  bucket was *contaminated*: 5 genuine charMgr methods, 4 second-level
  `Inventory` methods, and 3 that this document itself flags as client-local.
  The cause is `invCache.GetInventory`, which assigns a
  `Moniker('charMgr', …)` on one branch and a local wrapper on another, so
  flow-insensitive taint exports the wrapper as a moniker. The scanner now
  refuses that, and the 5 real methods go with it. **Keep the documented 5**,
  flagged as hand-read.
- **`invbroker`: 12 vs 15**, **`dogmaIM`: 38 vs 41** — the same wrapper and
  back-reference family. Two of the three missing `dogmaIM` names
  (`QueryAttributeValue`, `QueryAllAttributesForItem`) are called as `self.*`
  on `BaseDogmaLocation`, a shared client/server base, so they may be a doc
  error of the same kind as `planetMgr`.
- **The 32 `invbroker → Inventory` second-level methods are not reproduced at
  all.** They exist only because `invCacheContainer.__getattr__` forwards any
  uppercase attribute to the bound object. Enumerating them by taint means
  tainting a client-local wrapper, which is exactly what produced the false
  positives above. `derived_second_level` in the JSON is **honestly empty**,
  not silently dropped.

### The scanner's own weakest assumption

Two tree-wide uniqueness rules: taint crosses a module boundary on a bare name
only when that name occurs in exactly one module of the 12,774. They are what
make `dogmaIM` findable in `targetMgr.py`, and they are the only rules with no
positive evidence that the two names denote the same object — they rest on "the
name is unique, therefore it is the same thing". Every name they admitted is
listed in `promoted_names.promoted` in the JSON and every name they refused is
in `ambiguities_dropped`, so the lever is auditable rather than implicit.
