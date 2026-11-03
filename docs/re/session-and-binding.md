<!-- Provenance: the `session-and-binding` cross-cutting lane of
     `docs/archive/plan-the-churn.md` Wave 1, 2026-10-20. Read-only: nothing outside
     this file, `findings/session-keys.tsv` and `docs/lanes/session-and-binding.md`
     was written. Every line number below came from `co_lnotab`
     (`tools/pyc27.py` / `tools/call_lines.py`), never from uncompyle6's
     output -- see `docs/guides/debugging-the-client.md` on why that distinction is
     load-bearing. Marks are the house convention: **[V]** read from this
     build's bytecode or from this tree's code, **[I]** inferred,
     **[unknown]** not established. -->

# Session and binding, from the client's side

Client build V24.01 (reference 3662168 since 2026-10-31, issue #19; read on 3619879 and re-checked by `scan_session_keys`: 44 keys, only the `role` read count moved, 368 to 369). Companion files: `findings/session-keys.tsv`
(one row per session key), `docs/reference/session-roles.md` (the role mask, still
correct and not restated here), `docs/reference/bound-objects.md` (the bind mechanism
and the method inventory, likewise), `docs/reference/world-state.md` §4–5 (the lock
this document is meant to inform).

Citations are `<client path>.py:LINE`, the form the client's own tracebacks
use. The tree on disk is `out/pyc/<same path>.pyc`.

---

## 1. The ordering contract — the answer the coordinator needs

**The client imposes no ordering requirement between a `SessionChangeNotification`
and the `CallRsp` of the call that caused it. They are delivered by two
different mechanisms that do not interlock, and neither waits for the other.
[V]**

The three facts that settle it:

1. **A `CallRsp` never touches the session lock.** `machoNet.HandleMessage`
   routes a `__responsetypes__` packet (command 7) straight to the waiting
   tasklet: `self.calls[theMessage.destination.callID][0].send(theMessage)`
   (`carbon/common/script/net/machoNet.py:5113-5117`). It does **not** go
   through `srcTransport.SessionCall` / `SessionNotification`, which are the
   only two callers of `MachoCallOrNotification.__enter__` →
   `LockSession`. **[V]**
2. **A session change takes the session *write* lock and runs its whole
   handler chain inline.** `LockSession` takes `WRLock` when
   `packet.command in self.__sessioninitorchangenotification__` (16 and 18)
   and `RDLock` for everything else
   (`carbon/common/script/net/machoNetTransport.py:833-845`). The lock is a
   real `uthread.RWLock`, installed on the session by
   `_AssociateWithSession` the first time a packet is associated with it
   (`machoNetTransport.py:949`).
3. **The version-based ordering mechanism that exists in this code is
   unreachable from the wire.** `LockSession` will sleep 500 ms in a loop
   `while packet.sessionVersion > sess.version`
   (`machoNetTransport.py:816-829`) — but `sessionVersion` is set as a packet
   *attribute* only by `machoNet.TransportReader` inside its
   `macho.mode == 'proxy'` branch (`machoNet.py:4890`, `:4920`), and
   `MachoPacket.__setstate__` sets only the 14 state fields and the
   `__machodesc__['params']` names, so a packet that arrived over a socket
   never has the attribute. `sess.version` is also set to `1` once in
   `CoreSession.__init__` (`carbon/common/script/sys/basesession.py:795`) and
   never incremented on a client. **So the one piece of protocol that could
   have let us say "this answer is for session version N" does not reach
   us.** **[V]**

### What follows for the design

- **Send the change first, then the answer, and keep both in one write.**
  That is what `dispatch.rs` already does and it is right, for a reason the
  code does not state: the notification's handler chain runs *synchronously
  in the reader tasklet* and issues blocking calls of its own, which arrive
  as new `CallReq`s. Putting the change first means the handlers' questions
  are asked against a world in which the change has happened. **[I,
  well-supported]**
- **Answer-first is observably wrong, not merely untidy.** `PerformSessionChange`
  *returns the remote call's value to its caller*
  (`carbon/common/script/sys/sessions.py:522`), so the caller's
  post-call code resumes as soon as the `CallRsp` lands. Any of that code
  that reads a session attribute would read the pre-change value. **[V]**
- **The two must not be computed against different worlds.** Nothing on the
  client detects it; the symptom is a client that believes one thing and a
  server that believes another, which is `docs/reference/world-state.md`'s "wrong-but-self-
  consistent world" failure except that it is not self-consistent.
  `docs/reference/world-state.md` §5's hazard is therefore real and the fix it proposes —
  hoist the lock into `GpsSession`, take it once per payload, pass
  `&WorldState` down — is the right one. **There is no protocol-level excuse
  available: the client will not wait for us to agree with ourselves.**
- **Never push a command-12 notification that the handler chain is waiting
  on.** A notification takes `RDLock`, so it sleeps until the change's
  `WRLock` is released; if the change's handlers are blocked on a call whose
  answer we have made conditional on that push, that is a deadlock on the
  client with nothing in either log. `dispatch.rs`'s comment on
  `handle_select_character` names this, correctly.

### The client's own rate limiter, which is the next ordering trap

`SessionMgr.PerformSessionChange` sets `sess.nextSessionChange =
GetSimTime() + sessionChangeDelay` **before** issuing the remote call
(`sessions.py:516`), where `sessionChangeDelay` starts as
`SESSIONCHANGEDELAY = 10 * const.SEC` (`basesession.py:43`,
`sessions.py:1147`). A second session-change-triggering call inside that
window does not reach us at all: the client raises
`UserError('PerformSessionChangeInProgress', {'reason': ...})`
(`sessions.py:489`, `:429`). The escape hatches are
`sessionChangeShortCircuitReasons` — **`['autopilot']` and nothing else** in
EVE (`eve/common/script/sys/eveSessions.py:172`), which returns `None`
silently instead of raising — and `PerformSelectiveSessionChange`, which sets
`violateSafetyTimer` when the *current* reason starts with the caller's
`reasonAllowedToViolate` (`sessions.py:552-556`). That is why boarding a ship
right after undocking works and clicking undock twice does not. **[V]**

### The hook order inside one change

`CoreSession.ApplyRemoteAttributeChanges(changes, initialState)`
(`basesession.py:1575-1636`), in order: **[V]**

| line | what happens |
| --- | --- |
| 1582 | `StartSessionChange(reason=...)` sets `self.changing` |
| 1585 | `role & ROLE_SERVICE` → `RuntimeError` |
| 1589 | no `ROLE_LOGIN\|ROLE_PLAYER` → `LogSessionError` (not fatal) |
| 1592 | `ValidateSession('pre-change')` |
| 1594/1596 | `RecalculateDependantAttributes(changes)` for an initial state, `…WithChanges(changes)` for a change — **this is where `locationid` and `corprole` are injected into the change dict** |
| 1598 | `ComputeNotifySets(changes, pairs=not initialState)` → `(notify, disappeared)` |
| **1599** | **`if notify:` — an empty notify set means nothing below runs at all** |
| 1600 | `userid` old-and-new both truthy → `RuntimeError("A session's userID may not change")` |
| 1603 | the role-change-for-an-active-character warning |
| 1619 | `sm.NotifySessionChange('DoSessionChanging', True, self, notify)` — **before** the attributes move |
| 1620 | `DisconnectFilteredObjects(disappeared)` |
| 1622 | `__ChangeAttribute` per attribute: the values actually move here |
| 1625 | `ValidateSession('post-change')` |
| 1628 | `NotifySessionChange('ProcessSessionChange', True, self, notify)` |
| 1629 | `CallProcessChangeOnObjects(notify, isRemote=True)` |
| 1630 | `NotifySessionChange('OnSessionChanged', True, self, notify)` |

### Three wire-shape facts worth pinning

- **`change = (flag, {attr: (old, new)})` and the `old` we send is discarded.**
  `ComputeNotifySets` (`basesession.py:1397-1414`) rebuilds every pair as
  `(self.__dict__.get(attr, default), newValue)` — the client's *own* current
  value — and it is `notify`, not `changes`, that reaches the handlers at
  1619/1628/1630. So `service.rs`'s comment that `None` is correct for the
  old value is **right**, with one exception: the role guard at
  `basesession.py:1604` unpacks `changes['role']` directly, and a falsy
  `initial_roles` is what makes it skip. Sending `('role', (None, mask))` is
  therefore the *safe* form. **[V]**
- **A pair whose new value equals the client's current one is dropped**
  (`basesession.py:1410`), and if that empties `notify` the entire change is
  a no-op with no log line (line 1599). A re-sent identical change is
  silence, not idempotence. **[V]**
- **`sid` is cosmetic.** No client code path assigns `session.sid` from a
  packet; `base.CreateSession(packet.sid, …)` is reached only on the
  server/proxy side of `_AssociateWithSession`. The client keeps the sid it
  generated at boot. **[V]** (`machoNetTransport.py:921-922`,
  `basesession.py:798`.)
- **`MachoPacket.userID` (state field 3) is a session key in disguise.**
  `_AssociateWithSession` does `sess.SetAttributes({'userid': packet.userID})`
  when it installs the session, i.e. on the first packet of the connection
  (`machoNetTransport.py:951-953`). We send `None` there, so the key arrives
  only via `initialstate` — fine, but **if we ever set it, it must equal
  `initialstate['userid']`**, or the 1600 guard fires and the session is
  poisoned. **[V]**

---

## 2. What we would get wrong next, in the order it bites

Each row names the symptom first, because the symptom here is almost always
silence and knowing which silence is which is the point of this lane.

### 2.1 The session change is computed against a different world than its answer — **undock, today**

`handle_bind` dispatches the fused `ship.Undock` and *then* calls
`with_session_change` → `undock_change`, which re-reads the world
(`dispatch.rs`, `service.rs:992`). Under one `RwLock` that is two
acquisitions, and §1 says the client will not reconcile them.
**Symptom:** undock succeeds, the station view tears down, and the scene is
built from a ship/system pair that no longer matches what the answer
described — `docs/guides/debugging-the-client.md` §1's black screen with nothing in
either log. **Fix is `docs/reference/world-state.md` §5's hoist, and it has to land with
the lock, not after it.**

### 2.2 `undock_change` carries only two keys, and `shipid` is not one of them

`undock_change` sends `stationid: (station, None)` and
`solarsystemid: (None, system)`. That is enough for
`gameui.OnSessionChanged`'s gate (`gameui.py:555`) and enough for
`invCache._ShouldInvalidateCache` (`invCache.py:196`). But
`character_change` already set `shipid`, and the real server's own
`GetSessionValuesFromRowset` (`eveSessions.py:482`) treats
`shipid`/`stationid`/`solarsystemid` as one tuple. **Symptom if the ship
ever changes on undock** (it does not today, because we never board):
`targetMgr.ProcessSessionChange` computes `spaceShipChange = 'shipid' in
change and 'solarsystemid' not in change` and
`changingShipsInSpace`/`undocking` from it (`targetMgr.py:545`), and
`invCache.ProcessSessionChange` deletes the cached inventory for
`change['shipid'][0]` (`invCache.py:162`). Wrong grouping here deletes the
wrong cache entry. **[I]**

### 2.3 Two connections share one `user_id`, one `session_id` and one `node_id`

`handshake::Config::default` is `user_id: 1, proxy_node_id: 1` and
`Session::default` is `session_id: 1, user_id: 1`. Two clients therefore
present as the same user to a server that will soon have one world.
**Symptom:** nothing, until the session table of `docs/reference/world-state.md` §5 step 6
exists; then `station.GetGuests` lists the same character twice, and
`multiLoginBlocker.Login` cannot tell the two apart. The deeper one is that
`machoNet.GetClientSessionID()` is `(self.clientSessionID, session.userid)`
(`machoNet.py:3634`), so **`userid` is half of the token that invalidates
every bound-object handle** — sharing it across connections means we have
given two clients handles stamped with the same identity.

### 2.4 A reconnect, and `DirtySession`

Before connecting, `machoNet.ConnectToServer` walks `session.__persistvars__`
and raises `UserError('DirtySession')` if **any** attribute differs from its
default (`machoNet.py:1865-1872`). What saves us is
`machoNetTransport.SessionClosed` → `session.ClearAttributes()`
(`machoNetTransport.py:375`, `basesession.py:1266`), which resets every
persistvar to its default and empties `machoObjectsByID` and
`connectedObjects`. **Symptom if a disconnect path ever skips it** — e.g. we
close the socket in a way that does not produce a transport-closed event —
is a client that cannot reconnect without a restart, and whose error is
`DirtySession` rather than anything about us. We have never tested a
reconnect against a server that has state. **[V] on the mechanism,
**[unknown]** on whether our close path always produces it.

### 2.5 `corprole` the moment a corporation is not the default

Every `rolesAt*` is 0 today, deliberately, and `service.rs` says why. The
cost of changing that is specific: a non-zero `rolesAtAll` puts the derived
`corprole` into the change, and `invCache.ProcessSessionChange` then makes a
**blocking** `officeManager.GetCorpOfficeAtLocation()` call from inside the
session-change handler chain, while the write lock is held
(`invCache.py:162`). **Symptom:** the character-select click hangs, because
the handler is waiting for a call we do not answer and the whole chain is
holding the session's write lock. This is the cheapest way to turn a working
login into a freeze, and it is one non-zero integer away.

### 2.6 `locationid` is the invalidation key for every inventory handle

`invCache` stamps each inventory it binds with a session check:
`{'locationid': self.__invlocID}` for a non-station container,
`{'stationid': …}` in a station, `{'locationid': …, 'structureid': …}` in a
structure (`invCache.py:581`, `:586`, `:621`), and
`MachoObjectConnection.PerformSessionCheck` raises
`RuntimeError('RemoteObjectSessionCheckFailure', {...})` on **every** call
whose stamped attribute no longer matches
(`carbon/common/script/net/ObjectCallGPCS.py:667`). `locationid` is
*derived*, from `changes['stationid'][1] or changes['solarsystemid'][1]`
(`basesession.py:2183`). **Symptom of getting the stationid/solarsystemid
pairing wrong:** inventory calls stop happening — they fail locally, before
the wire, so our `unanswered` map stays empty and the only evidence is a
client-side traceback. This is the silence that looks most like "the server
is fine".

### 2.7 The first `/giveisk`-style push during a session change

A command-12 notification takes the session read lock
(`machoNetTransport.py:845`), so one that arrives while a change's handlers
are running sleeps until they finish. Harmless on its own, and `docs/reference/push-path.md`
§5 item 8 already records it. The rule it implies, and which is not yet
anywhere in the code: **a reply that carries a session change must not also
schedule a delayed push.**

### 2.8 Things we do not do and are right not to do yet

- **No `OID+` in `oob`.** The client only ever *writes* that key; inbound
  registration happens inside `ParseObjectID`. Confirmed: nothing in the
  client reads `oob['OID+']`. **[V]**
- **No `pa` (public attributes) on a reference.** `RegisterObjectChangeHandler`
  has **zero call sites in the whole tree** (12,774 modules scanned: its only
  five occurrences are its own definition at `ObjectCallGPCS.py:705` and
  `Moniker.__remobsuppmeth__` at `moniker.py:124`), so the
  `OnObjectPublicAttributesUpdated` path is dead client-side. The remaining
  exposure is a lowercase attribute read on a bound object, which
  `MachoObjectConnection.__getattr__` serves from `pa` and raises
  `AttributeError` for if absent (`ObjectCallGPCS.py:716`). Whether any live
  client path does that is **[unknown]**.
- **No release bookkeeping.** See §4.3.

---

## 3. Session keys

The full table is `findings/session-keys.tsv`: 36 distributed keys plus the
eight session-internal fields that turned out to matter. The parts that are
not already in `docs/reference/session-roles.md` §2:

**The authoritative "received" list is CCP's own, and it ships in the client.**
`SessionMgr.GetSessionValuesFromRowset` (`eve/common/script/sys/eveSessions.py:482`)
is the real server's session-building code: 23 keys unconditionally, plus
`solarsystemid2` from the station's system when docked and from
`solarSystemID` when in space, plus `constellationid`/`regionid` looked up
from `i2.__primeditems__`. It does **not** include `charid`, `userid`,
`role`, `languageID`, `sessionType`, `countryCode`, `address`, `userType`,
`maxSessionTime` or `inDetention` — those come from elsewhere on the real
server. **[V]**

**Only six keys move the game UI.** `gameui.OnSessionChanged`
(`eve/client/script/parklife/gameui.py:550`) returns immediately unless the
change contains one of `('userid', 'charid', 'solarsystemid', 'stationid',
'structureid', 'shipid')` (`gameui.py:555-562`), after first returning
unless `AnythingImportantInChange` is true — which is the change's keys minus
`('fleetid', 'wingid', 'squadid', 'fleetrole', 'languageID')`
(`gameui.py:526-547`). Then `charid is None` → character select,
`stationid is not None` → station, `structureid` → structure, else space
(`gameui.py:590-594`). **A change carrying only `corpid`, `allianceid` and
the `rolesAt*` family does not move the UI at all, by design.** **[V]**

**Derived versus received, settled.** Two keys and only two are derived:
`locationid` and `corprole`, both by
`Session.RecalculateDependantAttributesWithChanges`
(`basesession.py:2183`), which runs at `ApplyRemoteAttributeChanges` line
1596 — i.e. *before* `ComputeNotifySets`, so the derived keys are in the
change the handlers see. `docs/reference/world-state.md`'s statement that they must not
be stored server-side is correct, and the reason is sharper than "the client
derives them": the client derives them **into the change dict**, and
`invCache`/`officeManager` branch on their presence there. Sending them
ourselves does not stop that; it just adds a value that will be overwritten
whenever `stationid`/`solarsystemid` are also present and is a lie whenever
they are not. **[V]**

`hqID`, `baseID` and the four `rolesAt*` keys have **zero direct readers** in
the tree — they exist only to feed `corprole`. That census is
`docs/reference/session-roles.md` §2.3's and I did not re-run it; what I did check is that
the only consumer of `corprole`'s *presence* on the critical path is
`invCache.ProcessSessionChange` (§2.5). **[V]**

---

## 4. Binding: the paths, and what the client caches

`docs/reference/bound-objects.md` §1 is correct on the mechanism and is not restated.
What follows is the part it leaves open: how many ways a reference reaches
the client, and for how long each cache holds.

### 4.1 Four routes, not two

| route | shape | where |
| --- | --- | --- |
| `MachoResolveObject(bindParams)` | an ordinary service call returning a **nodeID**; `None` is an error (`RuntimeError('Resolution failure: ResolveObject returned None')`) | `moniker.py:284-330` (`__ResolveImpl`) |
| `MachoBindObject(bindParams, call)` | reply is the 2-tuple `(boundObject, retval)`; `call` is `None` or `(method, args, kw)` | `moniker.py:154` (`Bind`) |
| an ordinary `CallRsp` whose payload *is* a reference | the client assigns it and calls uppercase methods on it | `skillMgr2.GetMySkillHandler`, `scanMgr.GetSystemScanMgr`, `invbroker.GetInventory`/`GetInventoryFromId`, `entity.GetBehaviorTreeManager`, `keeper.GetLevelEditor` |
| **a `Notification`'s arguments** | the server pushes a reference to a client-side notify handler | `trademgr` → `OnTradeInitiate(charID, tradeSession, tradeItems)`, `eve/client/script/ui/station/pvptrade/pvptradesvc.py:41` |

The fourth route is new here and worth stating plainly: `ObjectCall.NotifyUp`
runs `self.FromPickle(packet.payload[1], packet.source)`
(`ObjectCallGPCS.py:243`), which is `blue.marshal.Load(…, self.ParseObjectID)`
(`:485`) — **the same decoder as a call reply**. So a notification can carry
a live object reference, and `OnTradeInitiate` is the worked example: the
receiving client then calls `tradeSession.List().tradeContainerID`
(`pvptradesvc.py:23`). Any push path we build must be able to mint objects,
not only data.

**Object-returning plain calls found so far: five.** Four are already in
`Registry::OBJECT_RETURNING`; the fifth is `trademgr.InitiateTrade(charID)`
(`pvptradesvc.py:35`), whose result takes `.List()`. A tree-wide
"remote call result, uppercase method taken on it later" scan produced only
three further candidates and all three were false positives —
`corpmgr.GetAggressionSettings` returns a **pass-by-value** instance of
`crimewatch/corp_aggression/settings.py` (its `IsFriendlyFireLegalAtTime` and
`GetNextPendingChangeTime` are local methods), and two were my scanner
tripping over `sm.RemoteSvc(...)` chains. **The scan's recall is poor** (it
only follows a store-then-load of a name within one module), so **five is a
floor, not a census** — `docs/reference/bound-objects.md` gap 9 says the same thing about
its own scanner and for the same reason. **[I]**

### 4.2 Resolve: cached forever, per `(service, bindParams)`

`Moniker.__ResolveImpl` (`moniker.py:284`) tries, in order:
`machoNet.GuessNodeIDFromAddress` — which is literally `return None` on a
client (`machoNet.py:3515`) — then `LookupAddressCache`
(`machoNet.py:3334`), then the remote `MachoResolveObject`, whose answer it
writes back with `SetNodeOfAddress` (`machoNet.py:3299`).

**The address cache has no TTL.** `MachoAddressCache` is a plain dict keyed
`(service, address) → nodeID` (`carbon/common/script/net/addressCache.py`,
`GetByServiceAddress`/`SetServiceAddress`). It is invalidated only by
`SetNodeOfAddress(…, None)` — which `Moniker.Unresolve` (`moniker.py:331`)
calls — by `RemoveAllForNode` / `MarkDeadNode` (`machoNet.py:3561`,
`HandleUnMachoDestination`), or by an explicit `ClearAddressCache()`
(`machoNet.py:3547`). So **our node id is cached for the life of the
process after the first resolve of each `(service, bindParams)` pair.**
**[V]**

`Moniker.MonikeredCall` (`moniker.py:231`) retries **once** through
`Unresolve()` on `UnMachoDestination` or `UserError('UnMachoDestination')`,
and re-binds on an `UpdateMoniker(serviceName, bindParams, nodeID)`
exception — refusing to follow one twice to the same place
(`RuntimeError('UpdateMoniker referred us to the same location twice')`).
`UpdateMoniker` is the server's way to say "that object moved", and we have
never raised one. **[V]**

### 4.3 Object identity and lifetime — settled

**The identity of a bound object, to the client, is the `objectID` string and
nothing else.** `ObjectCall.ParseObjectID` (`ObjectCallGPCS.py:379`) does:

```python
t = blue.marshal.Load(objectID, self.ParseObjectID)   # (oid, refID) or (oid, pa, refID)
if objectID not in session.machoObjectsByID:
    nid, oid = objectID.split(':')
    if nid != self.identifier:            # 'C=0' on a client
        ...
        return MachoObjectConnection(session.GetActualSession(), self, objectID, pa, refID)
    raise RuntimeError('The specified object (%s) does not exist within this session (%s)')
return weakref.proxy(session.machoObjectsByID[objectID][1])
```

Three constraints on an id we mint, all **[V]**:

1. **Exactly one `:`** — `objectID.split(':')` unpacks into two names.
2. **The prefix must not be `C=0`**, which is the client's own
   `identifier` (`ObjectCallGPCS.py:42`). `N=<node>:<n>` is right.
3. **If the id is already in `session.machoObjectsByID`, everything else in
   the reference is ignored** and the client hands back its own object. That
   dict holds objects the *client* published (`RegisterMachoObject`,
   `basesession.py:961`), so an `N=…` id can never collide with it — but it
   is the mechanism by which an id *can* be silently taken to mean something
   the sender did not intend.

**Lifetime is Python refcount, plus a 30-second batch.**
`MachoObjectConnection.__del__` (`ObjectCallGPCS.py:679`) calls
`session.UnregisterMachoObjectConnection`, which removes the id only when the
last connection for it is gone and then returns the stored `refID`
(`basesession.py:925`); that queues into `gpcs.releasedObjects`, and
`ObjectCall.__ReleaseObjectHelper` (`ObjectCallGPCS.py`, the 30 s loop in
`ObjectCall.__init__`'s worker) sends one `ClientHasReleasedTheseObjects`
**Notification** per `(sid, nodeID)` with `oob['OID-'] = {objectID: maxRefID}`
— and only for ids that are *still* unreferenced when the batch flushes.
`refID` is a release token, not an identity: `RegisterMachoObjectConnection`
stores `max(existing, refID)` (`basesession.py:909`). **[V]**

**Three independent local invalidations, none of which costs a packet:**

- `clisid`. `MachoObjectConnection.__init__` records
  `gpcs.machoNet.GetClientSessionID()` (`ObjectCallGPCS.py:630`) and
  `MachoObjectCallWrapper.__call__` raises
  `RuntimeError('This object was acquired by a different session')` if it has
  changed (`:750`). `GetClientSessionID` is
  `(self.clientSessionID, session.userid)` (`machoNet.py:3634`) and
  `clientSessionID` is incremented on **every** `ConnectToServer`
  (`machoNet.py:1862`).
- `sessionCheck`, per handle, enforced before every call
  (`ObjectCallGPCS.py:667`) — §2.6.
- `DisconnectFilteredObjects(disappeared)` at `basesession.py:1620`, for
  objects carrying a `__sessionfilter__`. On the client `connectedObjects` is
  empty, so this is the server's half of the same idea; **[unknown]** whether
  any client-hosted object ever declares one.

**So: is id reuse across a restart safe?** For *bound objects specifically,
yes, and for a reason stronger than hope.* A restart means a new TCP
connection, which means `ConnectToServer`, which increments
`clientSessionID`, which makes every surviving handle raise on first use —
and `machoNetTransport.SessionClosed` has cleared the session anyway
(`:375`). **What is not safe is id reuse for *game* ids** — `docs/reference/world-state.md`
§2 item 4 is about `itemID`/`shipID`/`notificationID`, which the client
caches by value in `invCache`, `godma` and the notification latches, with no
`clisid` equivalent. The risk is real; it is just not the bound-object
risk. **[V]**

### 4.4 One place our bookkeeping already diverges

`handle_notification` removes an id from `bound_objects` when
`ClientHasReleasedTheseObjects` names it, unconditionally. The client's own
rule is `refID >= stored` (`basesession.py:981`,
`UnregisterMachoObject`). With one object per service and a monotonic
minter this cannot bite today; it will the first time a bind is re-issued
for the same `(service, bindParams)` within one connection, because the
release of the *old* handle would then drop the *new* one. **[I]**

---

## 5. Node identity and address kinds

### 5.1 Is a notification's source required to be a Node address? — **Refuted as stated; the real rule is narrower and broader at once**

`docs/reference/push-path.md` records that a notification's source "must be a **Node**
address because a forking address closes the transport". The second half is
exactly right; the first half is stronger than the code requires.

`machoNet.HandleMessage` (`machoNet.py:5053-5058`): **[V]**

```python
if theMessage.command % 2:                             # a response
    if theMessage.destination.addressType in self.__aorb__:
        srcTransport.Close('A response must be sent to a specific address, not a forking one')
else:                                                  # a request or notification
    if theMessage.source.addressType in self.__aorb__:
        srcTransport.Close('A request must be sent from a specific address, not a forking one')
```

with `__aorb__ = (const.ADDRESS_TYPE_ANY, const.ADDRESS_TYPE_BROADCAST)`
(`machoNet.py:5015`) and `__borc__ = (BROADCAST, CLIENT)` (`:5014`). The four
address types are `NODE = 1`, `CLIENT = 2`, `BROADCAST = 4`, `ANY = 8`
(`carbon/common/lib/const.py:376-379`).

So the rule is: **for an even command, the source must not be ANY or
BROADCAST; for an odd command, the destination must not be.** A `CLIENT`
source would also pass. `MachoAddress::node(...)` is correct and should
stay — it is what the real proxy uses — but the *reason* to keep it is not
that NODE is required; it is that NODE is what makes `oob['sn']` sequencing
work if we ever use it (`WaitForSequenceNumber` only consults its sequencer
when `address.addressType == ADDRESS_TYPE_NODE and address.nodeID`,
`machoNet.py:3036`) and it is what the client's own object-id parser
expects to see on the other side of a reference. **The practical correction
to `docs/reference/push-path.md` is one word: "must not be a forking address" rather than
"must be a Node address".**

What each kind means to the client:

| type | meaning | where it appears from us |
| --- | --- | --- |
| `NODE = 1` | one node, `(1, nodeID, service, callID)` | the source of everything we send; the destination of every bound-object call the client makes |
| `CLIENT = 2` | one client | the destination of our session packets and pushes. Never read back into the session: `_AssociateWithSession` takes the `mode == 'client'` branch with `nodeID = 0` unconditionally (`machoNetTransport.py:874`) |
| `BROADCAST = 4` | `(4, broadcastID, narrowcast, idtype)`, a fan-out | the destination of a push to a notify handler. **Illegal as a source.** A reconstructed broadcast address has no `service` attribute at all, which is what stops `SessionCall` firing a client-side `ConnectToService` as a side effect |
| `ANY = 8` | "any node with this service" | we never emit it. **Illegal as a source** for an even command and as a destination for an odd one |

### 5.2 Where the client reads a node id, and what a multi-node answer would look like

Four places, all **[V]**:

1. `proxy_nodeid` from the handshake, stored as `myProxyNodeID` and used to
   address the first real service call.
2. `MachoResolveObject`'s return value, cached per
   `(service, bindParams)` in the address cache (§4.2) and used as the
   destination of the bind.
3. The `N=<nodeID>:<oid>` prefix of every object id, parsed in
   `MachoObjectConnection.__init__` (`ObjectCallGPCS.py:630`) into
   `MachoAddress(nodeID=long(nid))` with `blocking = 1`. A `C=` prefix gives
   `MachoAddress(clientID=…)` with `blocking = 0`, i.e. the server calling
   back into a client object gets **no response at all**.
4. `WaitForSequenceNumber`'s sequencer map, keyed by `address.nodeID`
   (`machoNet.py:3036`).

**A multi-node answer is therefore: `MachoResolveObject` returning a
different node id per `(service, bindParams)`, and object ids minted with
that node id in the prefix.** The client needs nothing else — it will open
or reuse a transport per node and address each handle to its owner. The
server-side check we do not perform is
`ServiceCallGPCS.CallUp`'s `WrongMachoNode(nodeID)`
(`ServiceCallGPCS.py:97`), which the client's `ResolveGPCS.CallUp`
(`ResolveGPCS.py:26`) exempts `MachoBindObject` from. Since we answer as one
node, `dispatch.rs`'s note that the question cannot arise is correct — and
the five places `node_id` must agree
(`docs/archive/architecture-sweep.md`, "The handshake is a constructor argument") are
exactly items 1–3 above plus the session packets' source. **[V]**

---

## 6. Corrections to existing documents

Reported, not edited.

1. **`docs/reference/push-path.md` §2.4** and its proposed test
   `a_notification_source_is_not_a_forking_address`: the prose says the source
   "must be a Node address"; the code says it must not be ANY or BROADCAST
   (`machoNet.py:5053-5058`). The test name is right and the prose is
   stronger than the check. Keep `MachoAddress::node`; fix the sentence.
2. **`server/crates/eve-proto/src/packet.rs:73`** (a code comment, not a
   doc): *"a stray `sessionVersion` in `oob` makes the client's `LockSession`
   spin."* `sessionVersion` is never an `oob` key and never a wire field —
   `MachoPacket.__setstate__` sets only the 14 state fields plus the
   `params` names, and only `machoNet.TransportReader`'s proxy branch ever
   sets the attribute (`machoNet.py:4890`). The key that actually matters in
   `oob` is **`sn`**, and the conclusion (send `None`) is right for that
   reason.
3. **`docs/reference/bound-objects.md` §1.3** says the resolve result "is cached with
   `SetNodeOfAddress`". True, and worth the extra sentence that **there is no
   TTL and no invalidation except `Unresolve` / node death / an explicit
   clear** (`addressCache.py`). The document's §1.6 list of local guards is
   right but understates `PerformSessionCheck`: it is not merely "a session
   change invalidates handles", it is a `RuntimeError` per call with the
   failing attribute named, and `invCache` stamps `locationid` — a *derived*
   key — on most of them.
4. **`docs/reference/bound-objects.md` §4 gap 10** asks which client objects are
   published as `C=` references. Still **[unknown]**, but one inbound case is
   now named: `trademgr` pushes a server object reference *to* the client in
   `OnTradeInitiate` (`pvptradesvc.py:41`), so the notification path carries
   references in both directions.
5. **`docs/archive/session-flow.md` / `docs/reference/login-path.md`** role-mask
   recommendation: already corrected in `docs/reference/session-roles.md` §7 and still
   uncorrected in those two files. Nothing to add.

---

## 7. Open questions

1. **[unknown] Does any live client path read a *lowercase* attribute off a
   bound object?** If so it needs `pa` in the reference, and we send the
   2-tuple form. `Moniker.__getattr__` (`moniker.py:125`) routes a
   non-uppercase, non-dunder attribute through `Bind()` and then
   `getattr(self.boundObject, key)`, which lands in
   `MachoObjectConnection.__getattr__`'s `pa` lookup
   (`ObjectCallGPCS.py:716`) and raises `AttributeError` if the key is
   missing. I did not enumerate the call sites.
2. **[unknown] Whether our disconnect always produces the transport-closed
   event that clears the client's session.** §2.4. Cheap to test: connect,
   kill the server, reconnect without restarting the client. If it raises
   `DirtySession`, we have a reconnect bug that looks like a client bug.
3. **[unknown] The full set of object-returning plain calls.** Five found,
   recall poor (§4.1). The right tool is `tools/scan_bound.py` with the
   object-returning results added as seeds, which is a day's work and would
   also close `docs/reference/bound-objects.md`'s "second-level bound objects" gap.
4. **[unknown] Whether `change[0]`, the flag in `(flag, dict)`, is ever
   load-bearing.** Unchanged from `docs/reference/session-roles.md` §8 item 2; the client
   reads only `change[1]` (`SessionChangeGPCS.py:49`).
5. **[unknown] `maxSessionTime`.** `gameui.__SessionTimeoutTimer`
   (`gameui.py:382`) is its only consumer and I did not read it. We send
   nothing, so the timer presumably never arms; worth 20 minutes before
   anyone sends a value.
