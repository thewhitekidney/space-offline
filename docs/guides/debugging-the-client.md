# Debugging the real client against our server

This is the loop that M1 is being built by. It exists because the work changed
character once a real client first connected: before that, every shape was
read off the client's own bytecode and written into a doc; now the client
tells us what is wrong, and the job is to *hear* it. The instruments below are
the difference between that and guessing.

Read `docs/reference/login-path.md` §8 for the call table itself. This file is about method.

---

## 1. The one rule that explains most symptoms

**An unanswered `CallReq` is a hang, not a no-op.**

This is a fact about the protocol, and it is why our server now *always*
answers — see the end of this section. The rule still governs how to read a
symptom, because it explains every silent failure in the project's history.

`ServiceCallGPCS.RemoteServiceCallWithoutTheStars` blocks its tasklet until a
response arrives. There is no timeout on the client side that recovers into a
drawn screen. So a single service method we do not implement produces:

- a **black screen**, or a login that stops on some progress string, and
- **nothing in the client's own log**, because nothing threw.

Three debugging rounds were spent on this before it was understood. If the
client goes quiet, the first hypothesis is always "we failed to answer
something", not "we sent something malformed".

The corollary matters when picking return values: a call that is *answered
wrongly* usually produces a traceback, which is evidence. A call that is not
answered at all produces silence. **Answering with a plausible empty value is
therefore strictly better than not answering**, even when the shape is a
guess — a wrong guess is self-reporting.

That corollary is now built in. `Dispatcher::unhandled` answers **`None`** to
any call the registry cannot serve, logs the `UNANSWERED` line, and keeps the
call on the unanswered list for the summary and the guard tests. A bind whose
fused call we cannot serve still hands out its object, with `None` as the
`retval`.

The effect on the debugging loop is the reason to care. Silence surfaces one
gap per run and needs a human in the loop for each; `None` surfaces as many as
the client can walk past, so a single run yields a worklist. It cannot do
better than the client's own tolerance — the first call that genuinely needs a
shape still stops progress there — but it never stops *earlier* than that.

Note that the client has several calls outstanding at once, one per tasklet.
Later runs showed it issuing six calls and continuing to answer pings while
four of them hung, so "the client is still responsive" does not mean "nothing
is hung", and the hung call is not necessarily the last one in the capture.

## 2. Bringing it up

```sh
sudo -v && tools/up.sh
```

`up.sh` starts `eve-dns` and `eve-server`, verifies the jail, launches the
client inside it, and tears the whole thing down when the client exits. It
must wait for the *client*, not for `proton run` — see `docs/guides/jail.md`, because
getting that wrong produces a convincing "unable to connect to server" that is
not a server bug at all.

Sharp edge in that wait: it polls `pgrep -f <client dir>.bin64.exefile`, which
matches **any** process whose command line contains that string — including a
shell command you type while debugging. So avoid putting the client path in a
command line during a session, or the loop will wait on your own shell. Left
as is deliberately: the alternative is guessing at what Wine sets `comm` to,
and getting *that* wrong reintroduces the premature-teardown bug above.

## 3. The instruments, in the order to reach for them

### 0. `tools/evidence.sh` — all of them at once

```sh
tools/evidence.sh            # the newest run
tools/evidence.sh --calls    # ... plus the decoded wire capture
tools/evidence.sh --watch    # re-print whenever either log grows
```

Start here. It prints the four instruments below in the order that diagnoses
fastest, from the newest run, with the `UNANSWERED` lines repeated under their
own heading because they are usually the entire answer.

**It is a script and not a service on purpose.** For several rounds the logs
were read by hand and pasted into the conversation, and the obvious fix looked
like a log-shipping daemon in a container. It is not needed. Every artifact
here is a plain file on this machine owned by the launching user:

| what | where |
|---|---|
| our server log | `logs/eve-server.log` (truncated per `up.sh` run) |
| the client's log | `$STEAM_ROOT/steamapps/compatdata/8500/pfx/drive_c/eve-logs/client-<stamp>-<pid>.log`, a fresh file per client process |
| wire captures | `captures/<epoch>-<nnnn>.evecap`, one per connection |

Any agent with shell access can read all of them directly, live, while the
client is still running — and **should**, rather than asking for a paste. A
daemon would add a process, an image and a port to re-expose files that were
never hidden. What was actually missing was knowing where to look and in what
order, which is this script.

The logs are also readable *while the run is live*, which matters more than it
sounds: a hung client never closes its connection, so the evidence is sitting
there complete long before anyone decides the run has failed.

### 0b. `tools/jail-drops.sh` — what the client tried to reach

Separate question, same run: **did anything try to phone home?**

The jail was always default-deny with no route out, so an outbound connection
we missed while patching the client was already impossible. What was missing
is that it was *silent* — a leak that never happened and a leak that was
blocked looked identical from outside, so "we found all the phone-home code"
and "nothing tried" could not be told apart.

**The evidence is DNS, and the firewall is the wrong place to look.** The
first version of this counted packets in the jail's nftables output chain.
That is nearly useless on its own, for a reason that is a property of the
jail's own strength: the namespace has **no default route**, so a packet to
anything outside `10.137.0.0/30` is rejected by *routing* -- `connect()`
returns `ENETUNREACH` immediately -- and never reaches the output hook at
all. The counter only ever sees traffic aimed at the one routable address on
a port we did not allow. A zero there means much less than it looks.

The resolver sees everything instead. The client must turn a hostname into an
address before it can try anything, and the only resolver the jail has is
ours -- so `logs/eve-dns.log` is a complete list of every service the client
wanted, by name, including every attempt that died at routing. It needs no
root and it outlives the jail, because it is our own file.

A real run looks like this:

```text
    28  clientstream.launchdarkly.com      REFUSED (NXDOMAIN)
    26  dev-public-gateway.evetech.net     REFUSED (NXDOMAIN)
    20  mobile.launchdarkly.com            REFUSED (NXDOMAIN)
     6  sentry.io                          REFUSED (NXDOMAIN)
     5  tranquility.servers.eveonline.com  resolved to us
     2  clientresources.eveonline.com      REFUSED (NXDOMAIN)
     2  api.ipify.org                      REFUSED (NXDOMAIN)
```

**A hit is not a breach.** Every one was answered `NXDOMAIN` and no packet was
sent. It is a to-do: the name says which subsystem still believes it has an
internet connection. Label by what the resolver *answered*, never by the
name -- our own hostname shares a domain with several we refuse, and guessing
from the name got this wrong on the first attempt.

The nftables counters stay as a secondary, live-only check, and
`jail-setup.sh` still ends both chains with a `counter` and a rate-limited
`log`. They are read with `sudo` while the jail is up, and they die with the
namespace -- which is fine, now that the durable record is elsewhere.

### a. `[svc] UNANSWERED` in the server log

`logs/eve-server.log` (and the terminal) names a missing call **the moment it
happens**:

```
[svc] UNANSWERED paperDollServer.GetPaperDollDataBatch/1: no such service paperDollServer -- answering None so the client keeps going
```

The `/1` is the positional argument count. This is printed immediately rather
than when the connection closes, because a hung client never closes the
connection, so the old end-of-session summary was never reached.

This single line is usually the whole diagnosis. Check it first.

**We answer `None` rather than nothing.** The line says so, and the
distinction is the point of §1 below: we used to reply with silence, which
hangs the client, so each missing method cost one launch–hang–read–implement
–launch cycle and surfaced exactly one gap per run. Now the client either
tolerates the `None` and carries on to reveal the *next* gap in the same run,
or it throws and names the shape it wanted. So expect **several** of these
lines per run, and treat them as a worklist in order rather than as one
diagnosis — the first one is still the most likely cause of whatever you are
looking at, because the later ones may only have been reached thanks to the
earlier fallback.

`None` is not a fix and the line does not claim it is. A service still owes
its real shape; see §4 for how to read one off the bytecode.

### b. `evecap --calls` — the call table

```sh
tools/dev.sh cargo run --locked -q --bin evecap -- /repo/captures/<newest>.evecap --calls
```

One line per service call, requests paired to responses **by callID** (not by
order — see above, calls come back out of order), with what we answered and
how long we took:

```
[ 39]  2370.586ms OK   charUnboundMgr   GetCharacterSelectionData()  -> ([<utillib.KeyVal ...>], ...)  (0.1ms)
[ 41]  2372.597ms OK   config           GetMultiOwnersEx([90000001])  -> (["ownerID", ...], [[...]])  (0.1ms)
[ 45]  2873.482ms HANG seasonalLoginCampaignManager get_active_campaign()  <- NO ANSWER; the client is still waiting
```

This is the view that drives the loop, because it shows both halves: the
arguments the client sent (so you can see *which* ids it wants) and the exact
value we returned (decoded through the same marshal code that produced it).
`--calls` reads the response payload twice on purpose: a `CallRsp` body is a
*pickle*, not the value.

Captures are written to `captures/` as `<epoch>-<nnnn>.evecap`, one file per
connection, newest last. The paths are inside the build container, so they are
`/repo/captures/...`, not `captures/...`.

Without `--calls` you get every frame in both directions, which is the right
view for handshake and framing problems, and `--tags` summarises type codes
the decoder could not read.

### c. `tools/client-log.sh --errors` — the client's own tracebacks

The client logs to `C:\eve-logs` inside the Proton prefix because we pass
`/fileLogDirectory`; `logmodule.LogException` writes the full formatted
traceback with locals as a single record.

**There are two markers, not one.** `EXCEPTION #n` .. `EXCEPTION END` is an
exception; `STACKTRACE #n` .. `STACKTRACE END` is a `log.LogTraceback` — a
traceback with no exception behind it. `--errors` extracts both. It used to
extract only the first, and silently dropped every traceback the client
logged.

The tell for that, if it ever regresses: **a gap in the `EXCEPTION #`
numbering.** The numbers are shared between the two kinds, so a jump from #3
to #5 means a STACKTRACE you are not being shown.

A `STACKTRACE` block is also worth reading for its *session dump*, which
lists every session attribute as the client currently holds it. That is the
cheapest way to check a `SessionChangeNotification` actually applied, and
which values the client derived for itself.

Use this when something *threw*. It will not help with a hang.

### c2. `evecap --alerts` — the same tracebacks, from *our* side of the wire

The client does not only log its exceptions locally. It **mails them to the
server**, by calling `alert.SendClientStackTraceAlert` with the formatted
traceback and the locals. We have always answered that call — it is in the
registry, it returns `None`, it has never once appeared on the unanswered
list — and that is exactly why it went unread for so long: *an answered call
leaves no trace in any server-side instrument.*

```sh
tools/dev.sh cargo run -q -p eve-server --bin evecap -- /repo/captures/FILE --alerts
```

The first capture this was pointed at held **fourteen** complete tracebacks
from a single session, each naming the frame that broke and the local that
was `None`.

Why bother, when `tools/client-log.sh --errors` shows the same records? Three
reasons, all about where the evidence lives:

- **A capture is ours.** It is in `captures/`, it survives the Proton prefix
  being wiped, and reading it needs nothing from the person who ran the
  client.
- **It is already aligned with the traffic.** The alert is frame *n* in the
  same file as the call that caused it, so `--calls` and `--alerts` on one
  capture are two views of one timeline.
- **It cannot be forgotten.** The client log has to be fetched; the capture
  is written by the run that produced the bug.

Read this immediately after `--calls`. `--calls` says which calls we fumbled;
`--alerts` says, in the client's own words, *what shape it wanted instead*.

### c3. `evecap --triage` — the same tracebacks, grouped and classified

`--alerts` prints everything, which is right when you are going to read
every word. After a session where the player opened a lot of windows it is
twenty-one tracebacks and several hundred lines, and the first thing anyone
does with it is the same two reductions: group the repeats, and throw out the
ones that are not machoNet.

```sh
tools/dev.sh cargo run -q -p eve-server --bin evecap -- /repo/captures/FILE --triage
```

```
14 tracebacks, 13 distinct: 7 ours, 6 gateway

OURS -- machoNet. A call of ours is missing or wrong.

   1x    15.7s  AttributeError: 'NoneType' object has no attribute 'clones'
          /eve/client/script/ui/services/clonejumpsvc.py(94) GetCloneState
          /eve/client/script/ui/services/clonejumpsvc.py(140) LastCloneJumpTime
   ...
GATEWAY -- the protobuf gateway we do not serve. See 3c.
```

Two things it does that are worth knowing about.

**It groups on the exception plus the deepest frame**, not on the whole
stack. The same break is commonly reached from several callers — a neocom
badge refresh and a panel load both landing on one repository method — and
those are one bug, not two. A list of twenty-one entries that is really eight
distinct bugs invites fixing the loudest instead of the worst.

**It classifies by protocol, and it leans towards `GATEWAY`.** The rule that
does most of the work is that *a `TimeoutException` is never ours*: machoNet
calls either get an answer or appear as `[svc] UNANSWERED` in our own log, so
nothing on our transport leaves the client waiting on a channel. On top of
that it looks for the §3c frame tells (`*messenger*`, `message_bus`,
`*gateway*`, `stacklesslib/util.py(247)`).

The lean is deliberate. A gateway failure mislabelled as ours costs an
afternoon hunting a service method that was never called; one of ours
mislabelled as gateway is still in the list, with its exception text intact,
under a heading that says we do not serve that. Only one of those two
mistakes is expensive.

Use `--triage` to pick what to work on and `--alerts` to actually work on it
— the locals, which are the most useful thing in the full dump, are exactly
what does not fit on one line.

### d. the client log again — it also shows how the client *read* our replies

Easy to miss, and the best validation tool we have. The log is not only
tracebacks: it carries the client's own decoded view of the traffic.

```text
Read:  Packet::CallRsp (Address::Any(service="invbroker",...),...,361 bytes,[...])
invbroker::GetItemDescriptor args=(), retval=<DBRowDescriptor object
  [('itemID', 20, 8), ('typeID', 3, 4), ...] 48>
UpdateItem: <DBRow object [90000001L, 1373, 90000001, 60000004L, 0, 1, 1, 1, 1, 1]>
Godma:UpdateItem - Inserting item 90000001
```

So for anything we send, the client will tell you **what it decoded it as**.
That is the difference between "the client did not complain" and "the client
read the row we meant": the second line above is what closed `docs/reference/dbrow.md`'s
first gap, because a packed row whose offsets were wrong would have come back
with the fields shuffled and been dropped by a category filter instead.

Grep this log for the service or method name whenever you want to know
whether a reply landed as intended, rather than merely landing.

## 3a. When every call is answered and it still breaks

Once the hangs are gone the next failure mode is the opposite one, and the
instruments above will not show it: **the client raises while reading a reply
we already sent.** A `KeyVal` short of one attribute, a tuple of the wrong
length, a column the client indexes on — all of these are an exception on the
far side of a reply that looked fine from here.

There is no server-side signal for this. No hang, no refusal, nothing in
`GpsSession.unanswered`, and `--calls` shows a contented `OK`. It appears only
in `tools/client-log.sh --errors`.

So once the server log is clean, **read the client log anyway**. The first run
to reach the character-select screen froze on it with 29 of 29 calls answered,
because the character row was missing `skillTypeID` and `deletePrepareDateTime`
— the second thrown from the mouse-over handler, which is why nothing could be
clicked (see `docs/reference/login-path.md`).

Two symptom shapes worth recognising:

- **frozen but drawn** — something threw partway through building the UI.
- **a flood of identical exceptions, then `Exception in Error Logging,
  preventing infinite recursion`** — the throw is in an input or render
  handler, so it repeats every frame or every mouse move.

When you find one, prefer a test over a fix: assert that the reply carries
what the client reads. `service.rs` has `ROW_ATTRIBUTES` and two tests doing
this for the character row, in both directions — nothing missing, and nothing
invented.

## 3b. When the client is right and the *installation* is wrong

A third failure mode, which cost a whole round because the symptom is
indistinguishable from a bad shape: **the client has our data and is missing
its own files.**

The hangar stayed black with every call answered. The traceback said:

```
hangarView.py LoadScene
TypeError: 'NoneType' object is not iterable
  stationGraphicFile = 'res:/dx9/model/hangar/minmatar/MH1/MH1_scene.red'
```

which is `scene, _ = self.sceneManager.LoadScene(stationGraphicFile, ...)`.
`sceneManager.LoadScene` returns `None` for a file it cannot open, and the
unpack raises — reading exactly like a 2-tuple we got wrong. It was not ours
at all. **41% of the client's indexed resources were absent**: 50464 of
122754 files, 17.5 GiB, including every byte of the hangar.

EVE streams its art. `resfileindex.txt` names every resource, the blobs live
in a `ResFiles/` store **shared between installs** (`tq`, `sisi`, ours), and
only what some client actually asked for is on disk. A copy that has never
rendered a hangar has never downloaded one.

```sh
tools/check-resources.sh                              # how complete is it?
tools/check-resources.sh --prefix res:/dx9/model/hangar --missing
```

**The tell** is a traceback whose locals name a `res:/` path, in a client
module that loads or draws rather than one that unmarshals. When you see one,
check the file before you re-read the bytecode.

**A red herring inside the red herring:** the error names a `.red` file, and
there are **zero** `.red` entries in the index against ~18700 `.black` ones.
The client translates the extension on its way to the cache, so a `.red` path
in a traceback is normal and is never itself the problem.

Two practical notes if this ever recurs:

- The resources are obtained through the official launcher — it is the
  intended distribution channel, and nothing of this project's needs to be
  shipped to make it work.
- It may update the install it manages and prune the shared store. That is
  safe for us, but check rather than assume: our copy is separate, and after
  one such update every one of the 50464 files we needed was still reachable
  under an **identical** hash. Measure it with `check-resources.sh` and by
  diffing the two `resfileindex.txt` files; do not take it on trust.

## 3c. When the traceback has nothing to do with machoNet

**There are two protocols in this client, and only one of them is ours.**
Half the tracebacks in a healthy-looking session come from the other one, and
no amount of adding service methods will ever silence them.

The tell is the exception type and the bottom frame:

| exception | bottom frame looks like | protocol |
|---|---|---|
| `TypeError: 'NoneType' object is not iterable`, `has no len()`, `has no attribute 'itervalues'` | a client UI module, with the `None` we sent sitting in the locals | **machoNet — ours.** Fix it. |
| `TimeoutException`, `ServiceUnavailableError`, `GoalMessengerError`, `CampaignRequestError`, `RuntimeError: Could not fetch ...` | `*/client/*messenger*.py` → `stacklesslib/util.py(247) receive` | **the protobuf gateway — not ours yet.** |

One capture of a session that reached the hangar carried fourteen tracebacks.
Six were the gateway: `corporation/client/goals/goalMessenger`,
`careergoals/client/messenger/request_messenger`,
`campaigns/client/v1/messengers/campaigns_messenger`,
`shipcosmetics/client/licensegateway/quasar`, and `jobboard` by way of
`careergoals`. Every one of them ends in `stacklesslib.util.receive`, which is
a channel with a timeout, not a call that can be answered over machoNet.

### When the tell is missing: a timeout the client swallowed itself

The `*messenger*` → `stacklesslib/util.py(247) receive` signature only shows
up when the `TimeoutException` escapes. Several gateway clients catch it
themselves and leave a cache set to `None`, and the exception you then get is
an ordinary `TypeError` or `AttributeError`, several seconds later, in a
frame with nothing gateway-shaped about it at all. Achievements is the worked
example:

```python
except ServiceUnavailableError as e:
    logger.info('unable to cache unclaimed rewards - ServiceUnavailableError silenced', ...)
except Exception as e:
    logger.error('unable to cache unclaimed rewards', ...)
```

`self._unclaimed_rewards` stays `None`, `get_all_unclaimed_rewards` falls off
its `if` and implicitly returns `None`, and the throw surfaces at
`achievement_rewards, category_rewards = self.get_all_unclaimed_rewards()` —
the third row of the §4 table, except that the `None` is CCP's own, generated
inside the client, and no answer of ours could have prevented it.

**How to tell anyway:** look for an earlier traceback from the *same object*.
The repository that timed out and the repository that threw are related by
ownership, and the alert dump prints the `self` of every frame with its
address. In `1792431326-0002.evecap` tracebacks 4 and 5 are 5.3 s apart, and
`0x32FC1CC0` is owned by `0x32FC1C50`. That identity is the proof; the
absence of a `receive` frame proves nothing either way.

This is why `evecap --triage` leans towards `GATEWAY` on ambiguity — and for
a while it was also the one case `--triage` got wrong, filing this traceback
under OURS, because a `TypeError` with no messenger frame is indistinguishable
from a real one by any rule derived from the *transport*.

The rule that does work is derived from the **package** instead. There is no
machoNet client anywhere in `achievement_system/`:

```sh
grep -ra 'RemoteSvc\|ConnectToRemoteService\|RemoteServiceCall' out/pyc/achievement_system/ | wc -l
0
```

A frame from a package that cannot call us is not ours, whatever the
exception says. `--triage` carries that list as `GATEWAY_PACKAGES`, kept
separate from the frame tells so that membership stays auditable: the test
for adding a package is the grep above returning zero, and nothing else.
`careergoals`, `campaigns`, `content`, `freelanceprojects` and `jobboard` all
pass it too; `cosmetics` looks like a candidate and **fails** it, because
`cosmetics/client/cosmeticsSvc.pyc` has a real machoNet client.

### Why they cannot be made to fail faster

`RequestBroker.send_character_request` registers a response handler with the
messenger's own `timeout_seconds` and hands the request to a **native** broker
(`eve_public_grpc_client.CqrsBroker`, a `.pyd`). There is no disabled path and
no short circuit: `ResponseChannel.receive()` waits out the timeout whatever
the connection is doing. `FakePublicGatewaySvc` exists in the tree but only
collects published *events* — it answers no requests.

So each of these costs one tasklet and one logged exception, ten to thirty
seconds apart, and **blocks nothing**. The UI drew the hangar with all six
outstanding. Treat them as noise until we serve the gateway.

**The clock is local, which settles the question completely.** The wait is a
deadline armed at send, not a network timeout:

```python
DEFAULT_TIMEOUT_SECONDS = 10
class ResponseHandler:
    def __init__(self, request, expected_response_class, timeout_seconds):
        self.response_channel = uthread2.queue_channel()
        self.deadline = time.time() + timeout_seconds
```

`RequestBroker.consume_responses` runs a tasklet at `uthread2.sleep(0.001)`
and calls `handle_request_timeouts()` on every pass; that is what raises
`TimeoutException`, and the socket is never consulted. Measured against a
real run: `GetMineWithRewardsRequest` sent at `17:35:39:878`, traceback at
`17:35:49` — 10.0 s to the digit, with the name `NXDOMAIN`ing and no TCP
connection in existence.

**So none of the obvious mitigations buys anything.** Resolving the name to a
closed port, or answering with an `RST` instead of `NXDOMAIN`, produces the
same 10.0 s. Neither path is a hot loop either: both the c-ares DNS retry and
the gRPC subchannel reconnect are exponential with the same ceiling, so the
churn in `logs/eve-dns.log` is cosmetic. **Leave it alone.**

One mitigation would actively *hurt*. A listener that accepts and immediately
closes lets TCP succeed while HTTP/2 never reaches `READY`, so the backoff
never resets — and the retry cadence is then set by native code we cannot
read. It buys nothing that pointing at a closed port does not.

Two call sites also make instant failure worse than slow failure, which is
the counter-intuitive part worth remembering if anyone revisits this.
`careergoals`' `_load_definitions` and `content/client/flatbuffer`'s
`refresh()` both assign their cache **only on success**, so the ten-second
wall clock is currently the only thing throttling them. Make them fail
instantly and the ceiling becomes UI refresh rate — the §4 live lock again,
arrived at by trying to help.

**And when we do serve it, answer `status_code = 200` with an unset payload,
not an error.** `ResponseHandler.handle` only unpacks
`if response_primitive.payload.type_url`, so an unset payload yields `None`,
which every messenger already branches on. It is the same work as returning
an error and it is the only version that fills those two caches. Note also
that 500, 502, 503 and 504 are in `STATUS_CODES_THAT_TRIGGER_RETRIES` and 4xx
is never retried — so an error answer should be 4xx if it is ever an error.

### The good news, for when we do serve it

`publicGateway/grpc/connectionConfig.py` resolves the endpoint like this:

```python
def get_address(self):
    address = os.getenv('EVE_PUBLIC_GATEWAY_ADDRESS')
    if address:
        return (address, False)          # <- False means INSECURE: no TLS
    return (self.discover_address(), True)

def discover_address(self):
    return '{}-public-gateway.{}'.format(self.tier, 'evetech.net')
```

Three things follow, and they are all in our favour:

- **No client patch is needed.** An environment variable redirects the whole
  gateway, so this costs nothing in client patching -- which is the rule that
  actually binds here: CCP's own files stay as they shipped.
- **It turns TLS off for us.** The env-var branch returns `secure=False`, so a
  stub gateway needs no certificate — the `certifi.where()` root bundle is
  only used on the discovery path.
- **The jail is already shaped for it.** `jail-setup.sh` has allowed tcp 8443
  to the host address since it was written.

`tier` comes from `monolithconfig.get_client_tier()`; ours resolves to `dev`,
which is why `logs/eve-dns.log` shows 26 refused lookups of
`dev-public-gateway.evetech.net` and no other gateway name.

### What one real run actually asks the gateway for

Measured from `throttle_and_warp` on 2026-10-24 by grepping the client log
for `Request <class> sent for`, so this is the live ask-list rather than a
reading of the packages:

| requests | `eve_public` request class |
|---|---|
| 4 | `pirate.corruption.api.requests_pb2.GetSystemInfoRequest` |
| 3 | `cosmetic.ship.api.request_pb2.GetRequest` |
| 2 | `pirate.suppression.api.requests_pb2.GetStageThresholdsRequest` |
| 1 | `user.license.api.requests_pb2.GetRequest` |
| 1 | `stargate.lock.api.requests_pb2.GetRestrictedSystemsRequest` |
| 1 | `cosmetic.structure.paintwork.api.api_pb2.GetAllInSolarSystemRequest` |
| 1 | `corporationgoal.api.requests_pb2.GetMineWithRewardsRequest` |
| 1 | `character.skill.plan.plan_pb2.GetActiveRequest` |
| 1 | `career.goal.api.requests_pb2.GetDefinitionsRequest` |

**15 requests, 9 classes** — and a tenth area that this count misses, worth
knowing because it shows the count is a floor:
`cosmetic.character.title`'s `GetEquippedForCharacterRequest` goes out
through `_send_user_request` instead of `send_character_request`, so it logs
no `sent for` line and is visible only through the exception it raises ten
seconds later. **Do not read a `sent for` census as the whole gateway ask.**

Every one of them timed out, and the session still reached space with all 20
scenario checks passing — which is the clearest statement available of how
little the gateway blocks.

**One of them is on the critical path for planned work.**
`stargate.lock...GetRestrictedSystemsRequest`: stargates are one of the four
features the bot plan names as blocking, and they have a gateway component.
Scope that feature as macho *plus* a gateway answer, not macho alone.

See `docs/port-tracker.md` §41.14.

## 4. Finding out what a call must return

The client is its own specification. The extracted bytecode tree is at
`out/pyc/`.

```sh
grep -ra GetPaperDollDataBatch out/pyc/ | head
tools/analysis.sh uncompyle6 /work/out/pyc/<path>.pyc
```

**Read the CALLER, not the method name.** What matters is what the client
*does* with the result: iterates it, subscripts it by id, unpacks a tuple,
feeds it to a rowset constructor. That tells you the minimum shape. A method
named `Get...Data` whose caller does `if not result: return` only needs a
falsy value, and `None` is then the *honest* answer rather than a stub.

### "Empty" is not one answer, it is five

The single most common mistake in this layer is reaching for `None` because
the honest answer is "nothing". Which *kind* of nothing depends entirely on
what the caller does with it:

| the caller does | the empty answer | getting it wrong |
|---|---|---|
| `if not x: return` | `None` | — |
| `x.iteritems()` / `for .. in x` | `{}` / `[]` | `None` → `AttributeError` / `TypeError` |
| `a, b, c, d = x` before testing | a 4-tuple of `None` | `None` → `TypeError`, *even though* the next line tests for None |
| `x.Index('col')` | a real rowset with columns declared | `None`, `{}`, `[]`, `(header, data)` all → `AttributeError` |
| reads the answer inside `except UserError` | **a raised exception** | returning *anything* → `UnboundLocalError` |
| `if x <= -4.5:` — compares it | a real **number**, `0.0` | `None` → **no error at all**, and the wrong branch |
| `if not x: self.available = False; raise` | a **truthy** value that filters itself empty | `None`/`[]`/`{}`/`()` → a latch that kills the subsystem for the session |
| `if self._c is None: self._c = {r[0]: r[1] for r in remote()}` | a real **iterable**, `[]` | `None` → a **live lock**: dropped frames |
| hands it to **native** code (`blue.dll`) | the exact type that code wants — `()` is **not** `[]` | `[]` where a tuple is wanted → `TypeError: must be tuple, not list` |

### The ninth kind of empty: the one native consumer

Eight of the nine rows above are about Python callers, which are relaxed
about types: a list and a tuple are both iterable, both falsy when empty,
and nothing in `carbonui` can tell them apart. The ninth row is different in
kind, and there is currently exactly one call in the whole server that hits
it.

`beyonce.GetFormations` is answered by `service.rs` and consumed by
`destiny.Ballpark.LoadFormations` in `blue.dll`. `michelle.AddBallpark` gets
our answer and passes it straight through. An empty **list** there raises
`TypeError: must be tuple, not list` at `michelle.py(453)`, and the player
sees a **black screen** after undocking: the session change has applied, the
station is gone, and no scene was ever built.

Two things are worth carrying forward from it.

- **The grep that finds the next one** is for a service answer that reaches
  native code without passing through Python that would have coerced it.
  `destiny`, `trinity`, `blue` and `bloodyPlugin` are the module names to
  look for in a traceback's *consumer* side.
- **It was not silent.** The arm's own comment predicted this failure and
  predicted it would leave no traceback; it left a perfectly good one,
  because `michelle.OnPrimaryViewChanged` runs under
  `serviceManager.MollycoddledUthread`, which catches everything and mails it
  to us. So `--alerts` found it in one pass. Native consumers are dangerous
  because they are *strict about types Python is relaxed about*, not because
  they fail quietly — and the distinction matters, because the first version
  of this note would have sent the next reader looking for a crash dump.

### The live lock — a symptom we had not seen before

The last row deserves its own name, because it is the first bad answer in this
project whose symptom was neither a traceback nor silence. The player reported
it as *"it seems to be looping over something and causing fps issues"*, and the
server log said so in one line — the same two calls, 89 and 88 times:

```
[svc] UNANSWERED LPSvc.GetAllMyCharacterWalletLPBalances/0 ...
[svc] UNANSWERED charMgr.GetPrivateInfoOnCorpChange/1 ...
```

The mechanism is **two failures compounding**:

```python
if self._characterLPBalanceByIssuerCorpID is None:              # 1. guard is `is None`
    lp = self._GetRemoteLPService().GetAllMyCharacterWalletLPBalances()
    self._characterLPBalanceByIssuerCorpID = {r[0]: r[1] for r in lp}   # 2. None throws HERE
```

`None` raises inside the comprehension, so **the assignment never runs**, so
the cache stays `None`, so the next read issues the call again — forever, at
UI refresh rate. Either failure alone is survivable: a cache that throws once
is a traceback, and a cache that never fills is merely wasteful. Together they
are a loop the player can see.

**The tell is in the log and takes one command.** A repeat count in the
*hundreds* for a single call, in a session that was idle, means a live lock
rather than a chatty UI:

```sh
grep -o 'UNANSWERED [a-zA-Z_:]*\.[A-Za-z_]*' logs/eve-server.log | sort | uniq -c | sort -rn
```

Compare against the `ProjectDiscovery.is_enabled` × 20 case in §4, which is
the same `is None` cache *without* the throw — wasteful, bounded, invisible.
The multiplier is what distinguishes them.

#### The worse variant: a cache that is poisoned rather than empty

`contractProxy.NumOutstandingContracts` is the same bug with **one more
failure stacked on**, and it is the one to recognise, because it leaves no
traceback at all. `createContractWnd.py(1545)`:

```python
n = 0
try:
    if getattr(self, 'numOutstandingContracts', None) is None:
        self.numOutstandingContracts = \
            sm.GetService('contracts').NumOutstandingContracts()
    ...
    n = self.numOutstandingContracts.myCharTotal
finally:
    return n, [maxNumContracts, cc.MAX_NUM_CONTRACTS][innerCorp]
```

Three things go wrong instead of two:

1. The guard is `is None` and our `None` is **successfully stored**. In the LP
   case the assignment threw and never happened; here it completes and sets
   the cache to the single value that fails its own guard. Same outcome,
   different mechanism — so "the assignment never runs" is not the thing to
   look for.
2. The attribute read one line later raises.
3. **`return` inside `finally` discards the in-flight exception.** `n` stays
   `0`, the wizard cheerfully prints "0 of N contracts", and nothing is
   logged anywhere.

So `--triage` on that capture lists **no** entry for it. The only evidence was
eleven calls in `--calls`, ten of them inside 1.8 s with the gaps shrinking to
~120 ms. A `return` in a `finally` swallows everything, and there is no
instrument on our side that can see past it.

**The practical rule:** when `--calls` shows a cluster and `--triage` shows
nothing, do not conclude the call is harmless. Conclude that something is
eating the exception, and go and read the caller.

The last two were found together and neither one throws, which is what makes
them worth naming separately.

#### The third variant: poisoned, loud, and too slow to count

`scanMgr.GetSystemScanMgr` is the `contractProxy` mechanism with the LP
visibility, and it is the one that **breaks the tell above**.

`scanSvc.GetScanMan` is the poisoned-cache shape exactly — the guard is
`is None`, and our `None` is successfully stored:

```python
def GetScanMan(self):
    if self.remoteObject is None:
        self.remoteObject = sm.RemoteSvc('scanMgr').GetSystemScanMgr()
    return self.remoteObject
```

But unlike `contractProxy` there is no `try`/`finally` around the
dereference, because the dereference is in a **different function**:
`sensorSuiteService.FullyUpdateSignalTrackerDebounced` writes
`self.scanSvc.GetScanMan().GetFullState()`. So the exception escapes and is
mailed to us, and `--alerts` names it in one pass:

```text
AttributeError: 'NoneType' object has no attribute 'GetFullState'
  /packages/uthread2/debounce.py(170) invoke
  /eve/client/script/parklife/sensorSuiteService.py(900) FullyUpdateSignalTrackerDebounced
```

**The part to carry forward is the cadence.** That method is
`@uthread2.debounce(1.0)`, so the loop is rate-limited by the decorator and
not by the UI: the observed run shows it roughly every **7 s**, a handful of
repeats per minute for the whole session. The rule two paragraphs up — "a
repeat count in the *hundreds* for a single call means a live lock rather than
a chatty UI" — therefore files this one as a chatty UI. It is not. It would
have run forever, and the sensor overlay was dead for the entire session.

So the multiplier distinguishes a live lock from a chatty UI only when the
retry sits behind a render or input handler. **When the retry sits behind a
debounce, a `BufferedCall` or an `AutoTimer`, the count is small by design and
the thing to look at is the repeat *interval*: a constant one, out to the end
of the capture, in a session that was idle.** `uthread2.debounce`,
`uthread2.BufferedCall` and `timerstuff.AutoTimer` are the three throttles to
recognise in a stack.

One more thing it has in common with the `dungeonExplorationMgr` case: **our
own answers are the clock.** `evedungeons/client/anomalyTracker.get_sites`
only reaches `if IsDocked(): sm.GetService('sensorSuite')
.FullyUpdateSignalTrackerDebounced()` *because* we answer its nine
`dungeonInstanceCacheMgr` listings. Fixing one call can start a loop on
another, and the loop is then driven at the Job Board's 3 s refresh — which
also means this one fires **while docked**, not only after an undock.

#### The fourth variant: a correct answer that is merely falsy

The three above all come down to what happens *at the store*. This one does
not, and it is why **the rule is to look at the guard, not at the store**:

| case | what goes wrong at the store |
| --- | --- |
| `LPSvc.GetAllMyCharacterWalletLPBalances` | the comprehension raises, so the assignment never runs |
| `contractProxy.NumOutstandingContracts` | the assignment runs and stores the one value that fails its own guard |
| `scanSvc.GetScanMan` | as above, but loud, and too slow to show up as a count |
| **`jumpCloneSvc.GetShipCloneState`** | the assignment runs and stores a **correct** value that is merely falsy |

```python
def GetShipClones(self):                                             # 98
    shipID = eveCfg.GetActiveShip()                                  # 101
    if not self.shipJumpClones or shipID != self.shipJustClonesShipID:  # 102
        ...
        self.shipJumpClones = lm.GetShipCloneState()                 # 105
    return self.shipJumpClones                                       # 106
```

**`class CRowset(list)`**, so a rowset with no rows is *falsy*. Our answer is
right -- the character has no jump clones -- and the guard is truthiness, so
the store at 105 succeeds, latches nothing, and the next read asks again.
Forever, with no traceback and no `UNANSWERED` line, because we *do* answer
it.

**And it cannot be made to latch honestly.** Python 2 truth is `__nonzero__`
then `__len__`, and every container these consumers accept derives truth from
length: `CRowset(list)`, `CIndexedRowset(dict)`, `CFilterRowset(dict)`, and
old-style `Rowset` defines its own `__len__`. There is **no truthy-and-empty
rowset**, so truthy means a row, and a row here means a jump clone that does
not exist and that the client would render by owner name and count in a status
bar. Raising is unavailable: nothing on that path has a handler.

So we keep the honest answer and accept the re-ask -- which is a legitimate
outcome, and the test for it is the one above: **how is the retry paced?**
All three consumers are user-paced (a tab select, a panel build, a menu
action), none behind a render handler or a throttle, and the clone tab has its
own empty-state text so the empty answer *displays correctly*. One extra call
per panel build is not a loop.

The lesson worth carrying: when a guard is truthiness rather than `is None`,
**the correct empty answer is itself the live lock**, and the question stops
being "what is the right value" and becomes "can any right value latch". When
the answer is no, document the re-ask and pin the emptiness in a test, so the
next person does not silence it by inventing a row. `service/character.rs`'s
`jumpCloneSvc` arm is the worked example.

**Python 2 orders `None` below every number.** `None <= -4.5` is `True`. So
`crimewatch.GetMySecurityStatus` answered `None` sailed through
`GetSecurityWhereAttacked`'s first comparison and told the client that a
brand-new character is a -4.5 outlaw attackable by faction police down to 0.5
space — in the character sheet's tooltip, in agent-dialogue route warnings,
and written into the dogma attribute `attributePilotSecurityStatus` every
time a pilot is put in a hull. For several runs there was no traceback and no
log line, and the absence read like evidence that `None` was fine. It is the
opposite: **a `None` that survives a comparison is a wrong answer with no
symptom.** Prefer the number even when nothing is visibly broken.

**An empty answer can latch.** `vgs.client.vgsClient.get_offers` is

```python
if not self.available:
    raise StoreUnavailableError()          # line 43
offers = sm.RemoteSvc('storeManager').get_offers(store_id)
if not offers:
    self.available = False                 # line 46  <- the latch
    raise StoreUnavailableError()          # line 47
```

so every falsy answer disables the whole store — `get_categories` and
`get_products` included — for the rest of the session, and every later call
raises at line 43 *without touching the wire*. That is why one capture held
two `StoreUnavailableError`s from the same window: one bug, two tracebacks,
and only the first reached us. **When a second traceback for the same
subsystem has a shorter stack than the first, look for a latch** rather than
counting it as a second fault.

The answer there is a list that is truthy and then filters itself empty — the
client's comprehension drops any offer without a non-empty `products`, so one
offer carrying no products is a real catalogue of nothing rather than an
invented one.

The fourth is the one that catches people twice: an empty container is not
empty *enough* if the client expects an object with methods. The third is the
reverse trap — the code tests `is not None` one line later, which makes `None`
look obviously safe right up until `UNPACK_SEQUENCE` runs first.

**The fifth is the exception to this file's own rule**, so it is worth
stating plainly: §1 says answering with a value always beats silence, and for
these calls that is false. `slash.SlashCmd('/')` is the known case —
`FetchRemoteCommandList` binds its local only inside the handler:

```python
try:
    sm.RemoteSvc('slash').SlashCmd('/')
except UserError as e:
    cmds = eval(e.args[1]['reason'].split(': ')[1])
...
self.remoteCommandList = cmds      # not assigned on the happy path
```

So a correct-looking `CallRsp` is an `UnboundLocalError`, and because
`_SlashCmd` calls this before matching anything, it breaks **every** slash
command — including the ones that never reach the server. `ServiceError::Raise`
exists for this: it sends a `MACHONETERR_WRAPPEDEXCEPTION` `ErrorResponse`
instead of a `CallRsp`. See `eve_proto::user_error` for the wire form, which
is tag `0x22` rather than the `0x17` our other instances use, because
`UserError` derives from `Exception` and is therefore new-style.

**The tell**, when hunting for others: a caller whose only use of the result
is inside an `except` block, or one that calls us purely for its side effect
on control flow.

A wrong-but-plausible answer gives you a traceback; `None` where a container
belongs often gives you a dead tasklet and silence. Prefer the shape the
caller touches.

One more category that no amount of shape-reading finds: **a value that is
structurally right and still wrong.** We sent `[]` where a dict was wanted;
the arity matched, the unpack succeeded, the session change completed, and it
failed later on a different tasklet with `AttributeError: 'list' object has no
attribute 'copy'`. So check what happens to the value *after* the immediate
call site. And note that Python 2's `hasattr` **swallows every exception**, so
a wrong shape behind one is invisible — that is how a bad `GetBoosters` answer
sat in the server log for a whole run with no traceback anywhere.

Two shapes come up constantly:

- `cfg.Recordset._Prime` accepts either a `(header, data)` **tuple** — tested
  with an exact `type(fetch) is TupleType`, so a list will not do — or an
  object with `__columns__`. The tuple form needs no whitelisted class and is
  what `eve_rowset::cfg` builds.
- Keys a prime reply *omits* are remembered in `knownLuzers` and never asked
  for again. So returning only the ids we can actually name is correct
  behaviour, not a gap to paper over.

### Tooling traps that have cost real time

- **`grep -r` silently misses matches in `.pyc` files** because they are
  binary. Always `grep -a` / `grep -ra`. This one has burned us twice.
- **uncompyle6's output line numbers are not the client's line numbers.** It
  emits its own reconstructed file, so `grep -n` or `sed -n` over its output
  gives you an *output* line. Three lanes in one round cited decompiler-output
  lines in source comments before one of them caught it; the citations then
  disagreed with every traceback the client had ever sent us, which is exactly
  the case where a line number is load-bearing.

  The three sources of **real** line numbers:

  | source | why it is right |
  | --- | --- |
  | `evecap --alerts` | the client prints its own `co_filename` and line |
  | `findings/client-calls.tsv`, the `call_site` column | built from `co_lnotab` |
  | `tools/pyc27.py` | decodes `co_lnotab` itself |

  Rule of thumb: **cite a line only if a traceback or `co_lnotab` gave it to
  you.** Decompiled output is for reading logic, not for locating it.
- **uncompyle6 sometimes prints const-table *indices* instead of values.**
  `godma.Prime` decompiles as

  ```python
  for att in [3, 4, 5, 6, 7]:
      sm.ScatterEvent('OnAttribute', att, char, getattr(char, att))
  ```

  and the real loop is over
  `['memory', 'perception', 'willpower', 'charisma', 'intelligence']` —
  const-table slots 3 to 7. We wrote the wrong version into two documents
  and a source comment before catching it.

  The tell was available the whole time: `getattr(x, 3)` raises
  `TypeError: attribute name must be string` unconditionally, so that loop
  could never have run. **Any list of small integers in decompiled output is
  suspect.** Four more instances have turned up since, all in the same shape
  — a class-body list of names rendered as indices:

  | decompiles as | really is |
  | --- | --- |
  | `cfg.eveowners.header = [1, 4, 5, 6, 7]` | `['ownerID', 'ownerName', 'typeID', 'gender', 'ownerNameID']` |
  | `CrimewatchService.__dependencies__ = [1,2,3,4,5,6]` | `['michelle','godma','war','facwar','corp','securitySvc']` |
  | `corp.__notifyevents__ = [1..11]` | the eleven event-name strings |
  | `godma.Prime`'s `[3,4,5,6,7]` | the five attribute names above |

  `__dependencies__`, `__notifyevents__` and `*.header` are the usual
  victims, because they are lists of string constants in a class body.
- **uncompyle6's line numbers are not source line numbers.** They count lines
  of its own output, which drifts from the original by however much the
  decompiler reflowed — about 50 lines by the middle of `uisvc.py`. Citing
  one in a note sends the next reader to the wrong place. The real line is
  the code object's `firstlineno`, which `tools/pyc27.py` exposes; quote that
  or quote the function name.

  Check it against the raw disassembly:

  ```sh
  tools/analysis.sh python3 -I -c "
  import sys; sys.path.insert(0,'/work/tools')
  from pyc27 import load_pyc, walk, dis
  co = load_pyc('/work/out/pyc/<path>.pyc')
  for c in walk(co):
      if c.name == b'<function>':
          print(c.consts)
          for op in dis(c): print(op)
  "
  ```
- **Never install anything on the host.** Decompiling, disassembling and
  anything Python goes through `tools/analysis.sh` (containerised, no
  network, repo at `/work`, client read-only at `/client`). Rust goes through
  `tools/dev.sh` (containerised, no network, repo at `/repo`).
- `marshalstrings.py` in the client tree is a **no-op** — it does not contain
  the marshal string table. The real 195-entry table is a native array in
  `blue.dll`; `tools/extract_string_table.py` reads it from there and refuses
  to emit a table that does not put `machoVersion` at index 135.

## 5. Known false alarms

These appear in the logs on a healthy run. Do not chase them.

| What you see | Why it is not ours |
| --- | --- |
| `ConnectionError: ... api.ipify.org ... getaddrinfo failed` | `monolithsentry/geoip2_tags.py` trying to geotag a Sentry report. The jail blocking it **is the feature working.** |
| `_GetByMessageID failed: ... 'min_value' ... for property with name ''` (×2) | CCP's own mock data in `freelanceprojects/client/contributionmethods/mock/` failing their own validator at import time. Present on the live client too. |
| `ValueError: 'proxy::writing' is not in list` | Our `PingRsp` was missing the proxy/server hop chain. **Fixed** — but if it returns, see `docs/reference/login-path.md`: the labels and their order are not a free choice. |
| `unable to connect to server` | Usually the jail was torn down under a live client, not a server fault. Check for a stranded client process. |
| `evecap --calls` printing `HANG ... <- NO ANSWER; the client is still waiting` for `slash.SlashCmd("/")` | **Was a bug in the tool**, now fixed. A raise comes back as an `ErrorResponse` (command 15), not a `CallRsp` (7), and `--calls` only paired the latter — so every deliberately-raised answer read as a hang. Raising is the *only* correct answer to `/`. Such calls now print as `RAISE`. |
| `Info: TypeError: 'NoneType' object is not subscriptable` under `settingsSvc.UpdateSettingsStatistics` → `placebo.py(22) verify` | `charMgr.GetSettingsInfo` answered `None` **on purpose**. A real answer is a signed, marshalled Python 2.7 **code object** the client installs as a function body and runs; the only consequence of declining is that settings statistics are never logged, which is telemetry we do not want to work. See the comment on `GetSettingsInfo` in `service.rs`. |
| `TypeError: 'NoneType' object is not iterable` under `achievement_system/.../repository.py(257) get_unclaimed_rewards_count` | The protobuf gateway, **despite having no `stacklesslib.util.receive` frame.** `_make_unclaimed_rewards_request` catches the `TimeoutException` itself and leaves its cache `None`, so the throw lands one stack later at `achievement_rewards, category_rewards = self.get_all_unclaimed_rewards()`. The way to tell is an earlier `ServiceUnavailableError` from the **same repository object** — in `1792431326-0002.evecap` they are tracebacks 4 and 5, 5.3 s apart, `0x32FC1CC0` owned by `0x32FC1C50`. §3c, not ours. |
| `RuntimeError: Could not fetch owned ship cosmetic licenses` and **a fitting window that takes tens of seconds to open** | The Personalization tab runs `logos.FetchLogoData` inside a `loadingContainer._LoadContentThread`, which blocks on the protobuf **license** gateway (`shipcosmetics/client/licensegateway/quasar.py`) until it times out. Section 3c: not machoNet. The spinner is the timeout, not our latency. |

The last of those is worth knowing before anyone goes looking for a slow server
path: a `loadingContainer` waiting out a gateway timeout looks exactly like a
server that is answering slowly, and `evecap --calls` will show every one of our
answers landing in well under a millisecond.

## 6. Where the client tries to phone home

Verified from the DNS log on a real run: the client actively attempts
`sentry.io`, `api.ipify.org`, `clientresources.eveonline.com`,
`dev-public-gateway.evetech.net` and launchdarkly. All resolve NXDOMAIN; only
`tranquility` resolves, to us.

The defence is layered and does not depend on the resolver: the namespace has
**no default route at all**, nftables defaults to deny, and a host rule at
priority -150 blocks forwarding. So a hardcoded IP has nowhere to go either.
File logging is a local sink; `prefs.ini` has no network-logging keys. See
`docs/guides/jail.md`.

**The client is the better witness, and it testifies in its own log.** The
DNS log and `jail-verify.log` are both ours, and `jail-verify.log` only
proves the *container* has no egress path. Reading the client's own log from
`throttle_and_warp` (2026-10-24) gives the same conclusion from the other
side of the wall — four endpoint families, each with the client's own words:

| endpoint | what the client logged |
|---|---|
| `api.ipify.org` | `getaddrinfo failed`, surfacing as `EXCEPTION #1: Failed to get public IP` |
| `sentry.io` | `Sentry send failed: ... getaddrinfo failed` — crash reporting cannot leave |
| launchdarkly | `sse-client: backing off ... due to Host not found [system:11001]`, 1→2→6→14→15 s; `flush-worker[0]: error posting 3 event(s) (some events were dropped)` |
| `dev-public-gateway.evetech.net` | every gRPC request raises `TimeoutException` at its 10 s local deadline (§3c) |

**Quote this table when the jail is claimed to work**, not our own verifier:
one is a measurement of the client's behaviour, the other is a measurement of
our configuration. `clientresources.eveonline.com` is the one endpoint from
the list above that this particular run did not log an attempt against.

## 7. Adding a service method

`eve-server/src/service.rs` is a flat `match` on service then method, and
every arm carries a comment saying **why that value** — ideally quoting the
client code that consumes it. Keep that; it is what makes the file reviewable
against the client rather than against itself.

Mark anything not confirmed from bytecode with **[I]** (inferred) in the docs,
so a later run can go and settle it. `[V]` means verified against the real
client.

`Registry::SERVICES` is a hand-written list beside the `match`, and it has
already drifted once — `userSvc` sat in it unimplemented for eleven commits,
advertising a service that any real call to would have hung. A test now fails
if a name in the list has no arm, so add the name and the arm together.
