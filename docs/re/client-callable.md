<!-- Provenance: the `client-callable` lane, 2026-10-20. Read-only with
     respect to code: no Rust was written or changed, no cargo was run.
     Three files were written: this one, `findings/client-callable.tsv` and
     `docs/lanes/client-callable.md`. Every client line number below came
     from `co_lnotab` (`tools/pyc27.py`), never from uncompyle6's output --
     and §0.1 records one place where the decompiler printed const-table
     indices where values belong, in exactly the way
     `docs/guides/debugging-the-client.md` warns it does. Marks are the house
     convention: **[V]** read from this build's bytecode or from this tree's
     code, **[I]** inferred, **[unknown]** not established. Nothing here has
     been run against a client. -->

# The server-to-client call path, and the `__exportedcalls__` catalogue

Client build V24.01 / 3619879. Citations are `<client path>.py:LINE`, the form
the client's own tracebacks use; the tree on disk is `out/pyc/<same path>.pyc`.
Companion file: `findings/client-callable.tsv`, one row per callable method.

Read `docs/reference/push-path.md` first if you have not. This document is its mirror
image: same chain, same addresses, opposite direction, and most of the same
hazards.

---

## 0. Question 1, answered first: **yes, and here is the wire shape**

**A server can issue a `CallReq` to this client and read the answer back. The
client's receive path for it is unconditional, needs no capability we do not
already have, and the reply arrives on the same TCP connection. [V]**

The three facts that settle it:

1. **The client handles every `CallReq` it is sent, and never looks at the
   destination to decide.** `machoNet.HandleMessage` guards the whole
   dispatch with `if macho.mode == 'client' or routesTo:`
   (`carbon/common/script/net/machoNet.py:5063`) — on a client the left
   disjunct is a constant, so `routesTo` is never consulted. Inside, command
   6 is in `__calltypes__` (`machoNet.py:467`) and takes the branch at
   `machoNet.py:5096`:

   ```text
   ret = srcTransport.SessionCall(theMessage)     # machoNet.py:5102
   ...
   srcTransport.Write(ret)                        # machoNet.py:5109
   ```

   **[V]** This is the same structural fact `docs/reference/push-path.md` established
   for notifications — `_AssociateWithSession` takes the `mode == 'client'`
   branch with `nodeID = 0` unconditionally (`machoNetTransport.py:874-877`)
   **[V]** — so the destination `clientID` is cosmetic on a direct
   server-to-client socket, exactly as it is for a push.

2. **`srcTransport.Write(ret)` writes the reply to the transport the request
   arrived on.** `machoNet.py:5109` **[V]** We do not have to be routable, we
   do not have to be a node the client knows, and no address we invent affects
   where the answer goes. It comes back down our own socket.

3. **The reply carries our own call id, in the one field designed for it.**
   `MachoPacket.Response` sets `theResponse.source = self.destination` and
   `theResponse.destination = self.source`
   (`carbon/common/script/net/machoNetPacket.py`, `Response`) **[V]**, and a
   node address serialises as `(1, nodeID, service, callID)`
   (`machoNetAddress.py`, `__getstate__`) **[V]**. So whatever `callID` we put
   in our `source` comes back in the reply's `destination.callID`. That is not
   a convention we are choosing; it is how the client matches its *own*
   replies — `self.calls[theMessage.destination.callID][0].send(theMessage)`
   at `machoNet.py:5115-5117`. **[V]**

### 0.1 A decompiler trap, recorded because it nearly changed the answer

uncompyle6 renders the server branch of `CallWrapper.__precall__` as
`[1, 4, 2, 3, 5]`. Those are const-table **indices**. The real value, from
`dis` on the class body at `service.py:90`, is

```text
['PreCall_IsExportedCall', 'PreCall_RoleCheck', 'PreCall_CheckPreArgs',
 'PreCall_CachedMethodCall', 'PreCall_Lock']
```

**[V]** The `docs/guides/debugging-the-client.md` warning about small-integer lists
in decompiled output is live, and this is a fresh instance of it. It matters
here because the *client* branch of the same list is what decides whether a
role is checked (§5.2).

### 0.2 The exact wire shape

A server-to-client call is a **`CallReq`, command 6**
(`machoNetPacket.py`, `CallReq.__machodesc__`) **[V]** — the *same* subclass the
client already sends us, which `eve-proto` already round-trips
(`packet.rs:790`). What differs between the two directions is only the
addresses and the payload's framing:

| field | client -> server (what we answer today) | server -> client (what this enables) |
| --- | --- | --- |
| `command` | 6 `CallReq` | 6 `CallReq` |
| `source` | `MachoAddress(clientID=0, callID=N)` | `MachoAddress(nodeID=<ours>, callID=N)` — a **node** address, carrying the call id we will match the reply on. Never a broadcast or ANY address (see below). |
| `destination` | `MachoAddress(nodeID, service=...)` | `MachoAddress(clientID=<id>, service='<name>')` — a **client** address naming the target service. On our direct socket the `clientID` is cosmetic; the `service` is not. |
| `payload` | `(1, method, args, kwargs)` for a service call | **same**: `(1, method, args, kwargs)` |

**[V]** for the client-to-server row (it is what `dispatch.rs` parses). **[I]**
for the server-to-client `source`/`destination` shapes — they are the mirror of
what `RemoteServiceCallWithoutTheStars` builds for the other direction
(`ServiceCallGPCS.py:306-307`, `payload = (1, method, args, kw2)` then
`CallReq(destination=..., payload=payload)`) **[V]**, with the node/client
address roles swapped, and that swap is unverified against a client.

**The payload framing is the subtle part, and it is the push path's framing
read backwards.** `docs/reference/push-path.md` §2.2 established that an *inbound*
notification climbs `ObjectCall.NotifyUp` -> `ServiceCall.NotifyUp` ->
`BroadcastStuff.NotifyUp` and so must be built as `(0, pickle((0, (1, args))))`.
A `CallReq` climbs the **call** methods of the same chain, which are shorter:

```text
ObjectCall.CallUp     payload[0] != 1  -> payload = FromPickle(payload[1])   (ObjectCallGPCS.py:213)
ServiceCall.CallUp    payload[0] == 0  -> payload = payload[1]; ForwardCallUp (ServiceCallGPCS.py:99-101)
```

**[V]** `ServiceCall.CallUp` branches on `if not packet.payload[0]:` and, when
false (`payload[0]` truthy), takes `destinationMethod = packet.payload[1]`
directly (`ServiceCallGPCS.py:107`). So there are **two valid shapes**, and the
real server uses the second:

- `payload = (0, pickle((1, method, args, kwargs)))` — climbs `ObjectCall` then
  `ServiceCall`, both unwrapping. This is what `ObjectCall.CallDown` produces
  for an *outbound* call: `packet.payload = (0, self.ToPickle(packet.payload, 0))`
  (`ObjectCallGPCS.py:234`). **[I]** mirror of the client's own send path.
- `payload = (1, method, args, kwargs)` **unpickled**, if the `ObjectCall`
  layer is bypassed. The client-to-server requests we already decode are this
  shape (`CallPayload::Service`, `call.rs`), and `ServiceCall.CallUp` reads
  `payload[1]` and `payload[2]` as method and args with no `FromPickle` in
  between (`ServiceCallGPCS.py:107`, `:161`). **[V]**

The reply is the asymmetry `docs/reference/push-path.md` and `call.rs` already document:
**a `CallRsp`'s payload is a pickle (a byte string), not the live value**,
because `ObjectCall.CallDown` on the *client's* send side hands the response
straight to `FromPickle` = `blue.marshal.Load` (`ObjectCallGPCS.py:239`,
`:495`). Reading the client's answer is therefore the same decode `dispatch.rs`
does for a request payload, and `eve-proto` already has every piece
(`MachoPacket::call_rsp`, `call::pickled`, the `0x17` wrapper).

### 0.3 `oob` and sequence numbers — the push path's rule **does** transfer

`docs/reference/push-path.md` records that a non-zero `oob['sn']` parks a notification's
tasklet forever in the client's `Sequencer`, and that `oob` must therefore stay
absent. **The same mechanism gates a call, for the same reason, and the rule
is the same: leave `oob` empty.** [V]

`machoNetTransport.LockSession` runs `self.machoNet.WaitForSequenceNumber(
packet.source, packet.oob.get('sn', 0))` for **every even-numbered command**
(`machoNetTransport.py:830`, guarded by `if ... not packet.command % 2:` at
`:815`). **[V]** A `CallReq` is command 6 — even — so a call goes through
`LockSession` and hits `WaitForSequenceNumber` just as a `SessionChange`
notification does. `WaitForSequenceNumber` returns early only `if not seq`
(`machoNet.py:3033-3034`) **[V]**, so a zero/absent `sn` is inert and a
non-zero one waits on sequence numbers we never send.

So the push path's "`oob` must stay absent" rule **does** transfer to a
server-initiated call. The one subtlety: `WaitForSequenceNumber` keys on
`packet.source.nodeID` (`machoNet.py:3036`, `:3038`), so our call's `source`
must be a node address with a nodeID — which it already is by §0.2 — but with
`sn` absent it never gets as far as using it.

`machoTimeout` is the *only* `oob` key either direction sets on a call
(`ServiceCallGPCS.py`, `RemoteServiceCallWithoutTheStars`; consumed by
`_BlockingCall` at `machoNet.py:3532`), and it is the **caller's** budget for
its own wait, not something the receiver acts on. We are the caller here, so we
would keep it in our own timer (§4), not on the wire.

### 0.4 One receive-path hazard worth carrying forward

The client reads with up to **20 concurrent reader tasklets** on a
server/proxy transport but only **2 on the client's own `tcp:packet:client`
transport** (`machoNetTransport.py:144-147`, `desiredReaders = 2` vs `20`).
**[V]** So the client will process at most two of our calls at once and the
rest queue. For agent dialogue — one modal box at a time — this is irrelevant;
it is noted only so a future batch-call design does not assume unbounded
parallelism.

Nothing we send on a call can kill the client's receive tasklet:
`machoNet.HandleMessage`'s whole body is wrapped in `except Exception:
log.LogException(...)` (`machoNet.py`, the `TransportReader standard error`
arm), the same backstop `docs/reference/push-path.md` §3 relied on for notifications.
**[V]**

---

## 1. How the client decides *what* to invoke

A `CallReq` names a service in `destination.service`. On the client,
`SessionCall` resolves that name to a live service object:

```text
msgSession.connectedServices[packet.destination.service] =
    packet.service = msgSession.ConnectToService(packet.destination.service, remote=1)   # machoNetTransport.py:697
...
ret = self.machoNet.channelHandlersUp[channel].CallUp(packet)                            # machoNetTransport.py:706
```

**[V]** `channel` is `None` for a `CallReq` (only session-change commands map
to the `'sessionchange'` channel — `machobase.packetTypeChannelMap`, which
holds only commands 16 and 18) **[V]**, so `channelHandlersUp[None]` is the
main GPCS chain head, `ExceptionWrapper` (`machoNet.py:822`). The call climbs to
`ServiceCall.CallUp`, which does the dispatch:

```text
destinationMethod = packet.payload[1]
if destinationMethod in packet.service.__dict__.get('__restrictedcalls__', {}) \
   or destinationMethod.startswith('_'):
    raise RuntimeError('This call can only be the result of a hack attack', destinationMethod)   # ServiceCallGPCS.py:108-109
...
method = getattr(packet.service, destinationMethod)                                               # ServiceGPCS.py:115
payload = ClockThisWithoutTheStars(..., method, args)                                             # :161/:164
...
return packet.Response(payload=payload)                                                           # ServiceCallGPCS.py:184
```

**[V]** The method is looked up on the resolved service object by name. There
is **no** check at this layer that the method is in `__exportedcalls__` — that
check lives one layer in, in the `CallWrapper` the service object's
`__getattr__` returns (§5). What `ServiceCall.CallUp` *does* enforce is only:
the name must not be in `__restrictedcalls__` and must not start with `_`.

So "what a server may ask a client to do" is governed by two things, and
`findings/client-callable.tsv` is the catalogue of the first:

1. the service must exist in the client's `svc.py` registry under the name we
   put in `destination.service` (the TSV `service` column); and
2. the method must be in that service class's `__exportedcalls__` (enforced by
   `CallWrapper.PreCall_IsExportedCall`, `service.py:279-281`) **[V]**.

The `service` column is **the registry name, not `__guid__.split('.')[1]`**.
They usually coincide, but the registry (`svc.py`, 298 entries) is authoritative
because it is what `ConnectToService` -> `GetServiceImplementation` consults.
Two rows have no registry entry at all (`webtools`' `WebTools`/`LoggingRoutines`
are imported differently); they are marked `(not-in-svc)` in the TSV and are
not server-callable by this path.

---

## 2. The inventory (question 2)

`findings/client-callable.tsv` has one row per callable method. Sweeping the
whole 12,774-file tree for `__exportedcalls__`:

- **103 classes declare `__exportedcalls__`.** 22 declare it empty (`{}`);
  **81 declare at least one method.** **[V]**
- **608 callable methods** across those 81 classes. **[V]**
- **79 of the 81 classes** map to a service name in `svc.py`; the 2 that do not
  are `webtools`'s pair. **[V]**

Each row carries: the `service` name to address, the class and `__guid__`, the
module and the `__exportedcalls__` declaration line, the method and its own
`co_firstlineno`, the argument names, the declared metadata (the `[role,
*preargs]` list or dict), and a `returns` flag derived from what precedes each
`RETURN_VALUE` in the method body (`None` / `value` / `None-or-value` /
`generator` / `unknown`). The `returns` flag says only whether a reply can
carry a value at all, not its shape; treat it as a filter, not a spec.

The largest services by method count: `machoNet` (76 — but see §5.3, most are
server-only no-ops on a client), `audio` (48), `mailSvc` (31), `facwar` (23),
`contracts` (21), `mailinglists` / `agents` / `addressbook` / `pwn` (15-20
each). The full per-service method roster is in the TSV.

**A caveat on completeness, in the house spirit.** 608 is the count of methods
*declared* in a class body whose `__exportedcalls__` the symbolic evaluator
could read. The evaluator handles `BUILD_MAP`/`STORE_MAP` with constant string
keys, which is every declaration in this build; a declaration built by some
other means (a dict comprehension, an `.update()`) would be missed. A spot
grep for `exportedcalls` in the 103 files found none built that way, so this is
believed complete, but it is **[I]**, not **[V]**, that 608 is the whole set.

---

## 3. The ones that matter, ranked (question 3)

Most of the 608 are irrelevant to us: UI plumbing the client calls on itself
(`loading.Cycle`, `device.SetDevice`), or `machoNet`/`sessionMgr` cluster
methods that are server-role no-ops on a client (§5.3). The ones that unlock
something we want are the agent-dialogue primitives on the `agents` service,
and they are worth having because they are the mechanism by which TQ runs a
*conversation* — the server interrupts the player and reads the answer, rather
than the player polling.

**The top three:**

1. **`agents.CheckCargoCapacity(mandatoryCargoUnits, extraFlagsToCheck)`** —
   `agents.py:387`, metadata `[ROLE_SERVICE]`, returns `None-or-value`. **[V]**
   This is the concrete one. It asks the *client* to total the active ship's
   cargo capacity across `flagCargo` plus any extra flags and compare it to the
   units a mission requires:

   ```text
   capacity, used = invCache.GetInventoryFromId(activeShipID).GetCapacity(flagCargo)
   ...
   if session.stationid is None and capacity - (used + mandatoryCargoUnits) < 0:
       rejectMessage = localization.GetByLabel('UI/Agents/StandardMissionCargoCapWarning', cargoUnits=...)
       self.MessageBox(localization.GetByLabel('UI/Agents/CannotAcceptMission'), rejectMessage)
       return ('mission.offeredinsufficientcargospace', rejectMessage)
   # else: falls off the end -> None                                      (agents.py:395-412)
   ```

   **[V]** So **it returns `None` when there is room, and the 2-tuple
   `('mission.offeredinsufficientcargospace', <localised reject string>)` when
   there is not** — and it pops the "cannot accept" box on the client *itself*
   before returning. For us this means the courier cargo check is the client's
   job: we offer the mission, call `CheckCargoCapacity`, and read the reply — a
   `None` lets us proceed, a tuple is the client already having told the player
   no, and we abandon the offer. It removes the whole server-side arithmetic of
   knowing the player's ship's fitted cargo/ore-hold/etc. capacities. (Note the
   `session.stationid is None` guard: the real check only bites in space;
   docked, it always returns `None`. **[V]**)

2. **`agents.MessageBox` / `YesNo` / `SingleChoiceBox` / `GetQuantity` /
   `PopupSelect` / `RemoteNamePopup`** — `agents.py:429` / `:415` / `:458` /
   `:496` / `:525` / `:507`, all `[ROLE_SERVICE]`. **[V]** These are the
   dialogue primitives. Each takes a title/body that is run through
   `ProcessMessage` **unless it is already a `basestring`** (`YesNo`:
   `agents.py:416`,`:420`; `MessageBox`: `:430`,`:434`; etc.) **[V]**, so a
   server can pass either CCP message-IDs (resolved against the localisation
   corpus, tying straight into `docs/re/agents-missions.md` §9's 27-slot
   vocabulary) or literal strings. Their returns are the player's answer:
   `YesNo` -> bool (`ret == uiconst.ID_YES`, `agents.py:426`), `SingleChoiceBox`
   -> `(ok_bool, choice_index)` (`:493`), `GetQuantity` -> the entered quantity
   or `None` (`:504`), `PopupSelect` -> the chosen typeID or `None` (`:541`).
   **[V]** This is what makes an agent conversation conversational: the server
   asks, the player clicks, the server reads the click. It is the cheapest
   route to the "conversational agent later" that `agents-missions` §3.3
   flagged, and it needs no new UI from us.

3. **`agents.PopupMission(agentID, charID, contentID)` and
   `agents.ShowMissionObjectives(agentID, contentID)`** — `agents.py:799`,
   `:857`, both `[ROLE_SERVICE]`, both return `None`. **[V]** These drive the
   mission UI from the server side: `PopupMission` opens the mission
   journal/offer window for an agent, `ShowMissionObjectives` refreshes the
   objectives in an open dialogue. They are fire-and-forget (no value to read),
   so they are really server-initiated calls used as *commands* — the server
   making the client's mission window appear or update without the player
   having clicked. Useful once the §1 mission slice from `agents-missions` is
   standing and we want the server to drive the window rather than wait for the
   client to poll.

Below the top three, still on `agents`: `PopupDungeonShipRestrictionList`
(`:838`) opens a dungeon ship-restriction window; `RemoveOfferFromJournal`
(`:854`) and `OpenDialogueWindow` (`:597`) round out the set. The non-dialogue
`agents` methods (`GetAgentByID`, `IsAgent`, `IsCareerAgentStation`, …) are
client-local queries with empty metadata and are not interesting to a server.

One service worth a glance for later, not now: **`michelle`** (14 methods,
`eve/client/script/remote/michelle.py`) is the client's ball/ballpark cache;
`michelle.Refresh` and friends could in principle be poked from the server, but
the Destiny push path (`docs/reference/push-path.md`) is the established way to move that
state and there is no reason to call into `michelle` instead. Likewise
`invCache`, `godma`, `target`: all exist as callable, none buys us anything the
notification path does not already.

---

## 4. What it costs us to implement (question 4)

The push path (`push.rs`) is a *write* path: a frame goes out and nothing is
expected back. A call needs three things a write does not, and none of them
requires blocking the read loop.

**4.1 The minimum addition.**

1. **A call-id allocator.** A process-wide (or per-connection) monotonic
   counter. We put it in the `CallReq`'s `source` as
   `MachoAddress::node(node_id, call_id=N)`. `eve-proto`'s `MachoAddress::Node`
   already carries `call_id` (`address.rs:21`, and `call_id()` at `:135`), so
   nothing in the proto layer changes.

2. **A pending-call table: `callID -> Sender<MachoPacket>`.** When we send a
   `CallReq`, we register `N -> sender` and keep the receiver. This is the
   mirror of the client's own `self.calls` dict (`machoNet.py:3531-3533`), and
   it is tiny.

3. **A matcher in the read loop.** `conn::serve` already owns the only place
   inbound frames are decoded. Today `dispatch.on_payload` handles `CallReq`,
   `Notification`, `PingReq` and logs the rest (`dispatch.rs:252-260`). Add one
   arm: a `CallRsp` (command 7) or `ErrorResponse` (15) whose
   `destination.callID` is in the pending table gets `sender.send(packet)` and
   produces no reply — exactly what the client does at `machoNet.py:5115-5117`.
   The caller, parked on the receiver, wakes with the answer.

**4.2 It does not block the read loop, and that is the whole point.** The
server is thread-per-connection with blocking I/O and no async runtime, and
`lib.rs` argues that is load-bearing for one-to-ten players. A call fits that
model without strain **because the thread that waits for the reply is not the
connection's read thread.** The read thread's only new job is the one-arm
match above — a table lookup and a channel send, microseconds, never a block.
The *caller* (a dispatch arm, a GM command, a mission step) blocks on a
`std::sync::mpsc::Receiver::recv_timeout`, on whatever thread it is already
running on. This is plain `std` — `mpsc` + a `Mutex<HashMap>` — and adds no
dependency, consistent with the empty `deps.allow`.

**4.3 The timeout.** `recv_timeout` is the whole mechanism; there is no ticker
to involve. A sensible budget is the client's own `callTimeOutInterval`, which
for a non-client role is **480 seconds** (`machoNet.py:148`) **[V]** — though
for an interactive dialogue (a `YesNo` the player may stare at) a server would
want its own, longer budget, since the human, not the network, is the latency.
On timeout we drop the pending-table entry; a late reply then finds no entry
and is logged and discarded, which is precisely the client's own behaviour
("the response will disappear", `machoNet.py:5119`). **[V]**

**4.4 The one real interaction with `push.rs`.** The outbound `CallReq` is a
frame, and the only correct way to put a frame on the wire while the read loop
may also be writing replies is through the existing `Wire` mutex. So the send
half of a call reuses `push.rs` unchanged — it is a `Wire::send_all` of one
framed `CallReq` — and only the *receive/match/park* half is new. That keeps
the "one lock, both the socket and the capture" invariant (`push.rs`) intact:
a call we issue is captured in order like any other frame.

**What this is not.** It is not a session table and not step C. It needs the
connection's `Wire` (to send) and a pending-call map (to match); it does not
need to know who is online. A GM command that calls `YesNo` on "the connection
I am handling" needs nothing more than the `Arc<Mutex<Wire>>` that
`conn::serve` already holds.

**[I]** on the shape above — it is a design, not code, and no byte of it has
run. What is **[V]** is the client-side contract it must satisfy: §0 (the
reply comes back on our socket, matched by `callID`) and §0.3 (`oob` empty).

---

## 5. The danger, stated plainly (question 5)

The `evejs-crosscheck` lane documented `GetHandshakeFunc` as arbitrary
unauthenticated Python execution on every connecting client. `__exportedcalls__`
is narrower by design. Here is exactly *how* narrow, and where it is not.

**5.1 The method set is genuinely restricted to what the class declares — at
one layer.** `CallWrapper.PreCall_IsExportedCall` raises `'... is not
exported'` for any method not in `__callable__.__exportedcalls__` (and not in
the four `__machomethods__`) (`service.py:279-281`) **[V]**, and
`ServiceCall.CallUp` independently refuses anything in `__restrictedcalls__` or
starting with `_` as *"This call can only be the result of a hack attack"*
(`ServiceCallGPCS.py:108`) **[V]**. So a server cannot reach an arbitrary
method; it can reach the 608 in the TSV and no others. That is a real boundary,
and it is the reason this is not another `GetHandshakeFunc`.

**5.2 But there is no role check and no session check on the client.** This is
the sharp edge. On the **server** side, `CallWrapper.__precall__` includes
`PreCall_RoleCheck` (`service.py:90`), which enforces the `[ROLE_SERVICE]` you
see in the metadata column. On the **client** side it does not: the client's
`__precall__` is `['PreCall_IsExportedCall', 'PreCall_CheckPreArgs',
'PreCall_CachedMethodCall']` (`service.py:88`) **[V]** — `PreCall_RoleCheck` is
absent. The `ServiceCallWrapper` the client builds for an inbound call
(`basesession.py:1555`, chosen when `session.role & ROLE_SERVICE` is false)
has `__precall__` = `['PreCall_WaitForRunningState', 'PreCall_IsExportedCall',
'PreCall_CheckPreArgs', 'PreCall_CachedMethodCall']` (`service.py:577`) — still
no role check. **[V]**

So the `[ROLE_SERVICE]` metadata on `CheckCargoCapacity`, `MessageBox` and the
rest is enforced **when the client calls the server**, never when the **server
calls the client**. The client trusts that anything arriving on its
authenticated server socket is entitled to call any exported method. The
`ObjectCall.CallUp` path *does* have a role branch for bound objects
(`ObjectCallGPCS.py`, the `0 == session.role & service.ROLE_SERVICE` block),
but that is the bound-object path, not the service-call path, and it checks the
*client's* session role, which on the client is itself. **[V]**

**5.3 The worst a malicious server could do through it.** The method set
bounds it, but the set is not harmless. Three categories:

- **Disruption.** `machoNet.Shutdown(who)` is exported `[ROLE_SERVICE]`
  (`machoNet.py:2824`). On a client its body is `bluepy.Terminate(0, ...)` —
  it **quits the client process** (`machoNet.py:2828`). **[V]**
  `connection.Disconnect` / `machoNet.DisconnectFromServer` likewise drop the
  connection. A hostile server can kill or disconnect its own clients at will.
- **Annoyance / social.** The dialogue primitives can pop arbitrary (even
  arbitrary-*string*, bypassing `ProcessMessage`) modal boxes at the player:
  `MessageBox`, `YesNo`, `GetQuantity`. Harmless individually, a griefing tool
  in a loop.
- **Nothing that escapes the sandbox *by this path*.** Crucially, **the one
  genuinely dangerous method is already closed on the client**:
  `debug.Eval` / `debug.Exec` carry `[ROLE_PROGRAMMER]`, but their *bodies*
  begin `if machoNet.mode == 'client': return` (`debugSvc.py:115`,
  `Exec` similarly) **[V]** — on a client they are no-ops regardless of role.
  So `debug.Eval` is exported and reachable, but it does nothing on a client;
  it is not a `GetHandshakeFunc`-class hole. I checked for any exported method
  whose client body calls `eval`, `exec`, `marshal.loads` or `__import__`
  unconditionally and found none in the 608.

**5.4 Does shipping this weaken our client-jail argument? A little, honestly
stated.** Our jail argument has been "the server can only answer calls and send
notifications; it cannot make the client *do* things." That is already false in
the client's design — the server can call 608 methods — and our *not* using the
path does not change what the path is. What shipping a call primitive changes
is only that *our* server would exercise a capability the client already grants
every server. The capability is bounded (§5.1) and the one unbounded method is
self-disarming on the client (§5.3), so the honest statement is: **this does
not open a new hole in the client; it uses a door the client holds open for any
server, and the blast radius of that door is "disrupt/annoy its own clients,"
not "arbitrary code on them."** We are the server, so none of it is a threat to
*us*; it is a correction to the jail *claim*, which should now read "the server
can issue calls, but the exported-call set bounds them and excludes client-side
code execution."

---

## 6. What is `unknown`

- **The server-to-client `source`/`destination` addressing is [I], not [V].**
  §0 proves the client *handles* a `CallReq` and *replies on our socket*; the
  exact `MachoAddress` field values a direct (non-proxy) server must put in a
  server-initiated `CallReq` are the mirror of the client's send path and have
  **not** been confirmed against a client. The first real server-initiated call
  settles it. In particular, whether `destination` should be a `Client` address
  (`clientID=…`) or whether a `Node`/`Any` address also works on our direct
  socket is **unknown**; the client's own `ConnectToClientService` builds a
  `MachoAddress(clientID=…, service=…)` (`sessions.py:773`) **[V]**, which is
  the basis for the `Client`-address guess.
- **Which payload framing to use — `(0, pickle((1, …)))` vs bare
  `(1, method, args, kwargs)` — is [I].** Both are valid at the
  `ServiceCall.CallUp` branch (§0.2); which one a real server emits for a
  service (as opposed to bound-object) call is not confirmed. The bare form is
  the one our own `dispatch.rs` already round-trips in the other direction, so
  it is the better first bet, but this is a guess.
- **Whether `agents` is the right service name to address for dialogue is
  settled ([V], `svc.py` maps `agents` -> the class), but whether the server
  addresses the *UI* `agents` service or a server-side `agentMgr` that then
  relays is a TQ-topology question we cannot see** — on our single-node direct
  socket it is moot (we address the client's `agents` directly), but it is
  flagged so nobody reads a multi-node assumption into this.
- **No call has been issued to a client.** Everything in §4 is a design. The
  client-side *contract* (§0, §0.3) is `[V]`; the server-side *implementation*
  is unbuilt and unverified end to end.
