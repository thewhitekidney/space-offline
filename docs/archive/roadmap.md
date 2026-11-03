# Roadmap

> **Demoted 2026-10-28** — its milestone list (M0-M4+) ends at "undock" with "M4+" as one undefined line; the server is long past that and playable (README.md, `docs/port-tracker.md`). Its own "The layer below this page" already says the spine is complete and the "why not system-by-system" argument has expired. The founding vision in "What this is for" still stands. Current truth: `docs/archive/plan-subsystems.md`, `docs/archive/plan-deep-queue.md`. Kept for history; do not brief a lane from this file.

Each milestone is a demoable state; gaps between them are roughly an order of
magnitude of work. Detail lives in the linked docs.

## What this is for

**A co-op EVE: playing with friends, or against each other, alongside bots
that act as players.** In the owner's words:

> kind of like an MMO-RTS where a single game potentially can run for years,
> with no true victory goal — that's entirely up to the player what they
> consider victorious.

That is the end state, and it is worth writing at the top because it is not
what the work looks like from inside any single milestone.

**"Runs for years" is the load-bearing phrase**, and it is an architectural
requirement rather than a feature. The design for it is in
[`docs/reference/world-state.md`](../reference/world-state.md).

**Both halves of that headline have since been built**, and this section said
otherwise for longer than it was true — recorded here rather than quietly
edited, because a roadmap that describes a server from two rounds ago is worse
than no roadmap:

- **The world is shared.** `server.rs` holds **one** `Arc<RwLock<Registry>>`
  for the process and hands every connection a clone of the handle. It used to
  build a `Registry::new(config.sandbox.clone())` per connection, so two
  clients were two worlds that merely agreed because they were cloned from the
  same `Default`. The locking discipline that came with the change is
  load-bearing and documented on `dispatch::GpsSession`'s `world` field: never
  hold the world lock across a socket write, and take world-then-wire
  everywhere so there is no cycle to deadlock on.
- **Persistence exists, and is opt-in.** `Config::world_file` selects a
  `store::FileStore` and defaults to `NoStore`, so a server started without one
  still keeps nothing. Items, balances, the journal, skills and industry jobs
  round-trip through `store::Snapshot`. Fittings still have nowhere to go,
  which is why the fitting *mutators* remain unimplemented (§2.6 of
  `docs/archive/entering-the-game.md`) — that reason is still live even though the
  "nothing is persisted at all" that used to precede it is not.

**"No true victory goal" does not mean "no content."** An earlier draft of
this section read it that way and was corrected: *we still want all the stuff
in the game.* Agents, missions, industry, the market, career goals, the job
board — all of it is wanted eventually. Real EVE has every one of those and
still has no end state; that is the point of the comparison, not an exclusion.

What the phrase actually rules out is a **terminal** state: no scenario that
completes, no scoreboard that closes, nothing that declares the game over.
Whether a player's victory is a titan, a wallet, a corporation or a quiet
mining operation is their business, and the server's job is to keep running
either way.

So the protobuf gateway is **deferred, not deleted** — a large slice of that
content lives behind it (`docs/guides/debugging-the-client.md` §3c), and
`EVE_PUBLIC_GATEWAY_ADDRESS` is already the patch-free way in when we want it.
Today's timeouts are a debt we have chosen not to pay yet, not a design
decision.

Two corrections it makes to assumptions this repo has been carrying:

- **The GM tools are a means, not the goal.** They buy a sandbox to build in.
  The owner did not have GM access on Singularity in the first place, so
  "recover the Singularity sandbox" is about the *feel* of a free sandbox and
  not about the toolset. Unlocking a dev menu is not a finish line.
- **A single-player docked client is a waypoint, not the target.** Everything
  M1 and M2 built is one session, one character, one station.

What that reorders, concretely. These stop being late-stage polish and become
foundations:

| why it moves up | what it is |
|---|---|
| nothing shared can be seen without it, **and now: nothing in space can either** | the server-initiated **`Notification`** (command 12) push path — broadcast address, `payload = (0, pickle((0, (1, args))))`. We have never sent one. No longer merely desirable: see below. |
| PvP and bots need somewhere to happen | **space**: the Destiny ballpark and its full-state blob (`docs/guides/debugging-the-client.md` §3c is about the *other* protocol; this one is ours) |
| two players is a different server from one | **concurrent sessions** against one world, rather than one `SandboxConfig` character |
| bots are most plausibly server-side | actors driving Destiny balls, not extra real clients |
| a world that outlives the process | **durable state** — today there is none; see above |
| years make time an axis | skill training, market history, industry, standings decay stop being cosmetic |

**Push is not optional, and we checked rather than assumed.** It was an open
question whether a client that only ever polls could see its own ship, which
would have let us defer the whole push path. It cannot, on four independent
counts:

- **Nothing on the client fetches state.** Across all 12,774 parseable
  modules, `ReadFullStateFromStream` and `UpdateStateRequest` appear only in
  `michelle.pyc` and one dead, unimported module. There is no
  `beyonce.GetState`.
- **The one call that looks like a poll is not one.**
  `Park._UpdateStateRequestThread` calls `UpdateStateRequest()` as a
  *statement*, discarding the return, and its only live caller is the desync
  recovery branch. A client that never received a state never calls it.
- **There is nothing to piggyback on.** In the undock capture, after the
  `beyonce` bind at 43.4 s the client issues **no service calls at all** for
  the remaining 19 s except its own crash mail and a disconnect log. Its last
  `PingReq` was 48 s earlier; `clientKeepAliveTimerInterval` is 240 s.
  Destiny ticks at 1 Hz and the client reads at 1/240 Hz, so answering only
  when asked cannot work even in principle.
- **The receive side is already wired.** `michelle.__notifyevents__` carries
  `DoDestinyUpdate` and `DoDestinyUpdates`, and `CoreBroadcastStuff.NotifyUp`
  routes any `broadcastID` beginning `Do` through
  `sm.SendEventWithoutTheStars` inside a `try/except` — so a malformed blob
  is a traceback, not a hang.

The cost is smaller than the conclusion sounds: `TcpStream::try_clone()`, a
mutex around the write half, one ticker thread per connection. No new
dependency, and a better fit for a blocking thread-per-connection design than
for an async one. `eve-proto` already has `MachoNetMsg::Notification` and
`MachoAddress::Broadcast`.

The protobuf gateway is the one thing that moves *later* rather than lower:
it costs tasklet timeouts and a slow Personalization tab today, and nothing a
second player would notice — but the content behind it is wanted, so it is a
scheduling call, not a scope cut.

**The honest scoping note.** This is a full server, and it is meant to become
one — the owner's words. Everything M1 and M2 did, reading the wire format and
matching shapes to callers, is the *tractable* half, and it is nearly
mechanical now that the instruments exist (`evecap --alerts` turns a run into
a worklist with the shapes already attached).

The half that is not mechanical is the one this section describes:
persistence, concurrency, and a simulation that keeps running when nobody is
watching. Those are ordinary hard server problems rather than
reverse-engineering ones, and they are **not blocked on the client**. It would
be reasonable to start them before space works — and every shape we add
between now and then is one more thing that will need a home in whatever store
we pick, which is an argument for picking it sooner rather than later.

- **M0 — recon & scaffold. DONE.**
  - Client analysed (`docs/archive/recon-v24.01.md`): Stackless Python 2.7, unobfuscated,
    Carbon server code shipped, static data client-side, 175 remote services +
    5,786 protobuf messages recovered.
  - Override resolved: source `.py` import live, no guard, env-independent.
  - Jail built (`docs/guides/jail.md`): a network namespace with no route off it; premise
    validated unprivileged (full run needs root).
  - Toolchain: container-isolated Rust workspace, green (fmt/clippy/tests).
  - `eve-marshal`: byte primitives + confirmed wire constants + 210-entry
    string table. Numeric marshal opcodes deferred to M1 (native; see below).

- **M1 — character-select screen. DONE**, and the character selected: the
  screen draws, a character can be clicked, and the session change puts it in
  a station. What remains before the hangar *draws* is view-layer service
  surface, which is M2's business.
  - **Every wire-format gap is closed**, by disassembly rather than by the
    capture that was supposed to settle them: the marshal tag space
    (`docs/archive/marshal-tags.md`), TCP framing (`docs/reference/framing.md`), the DBRow packed layout
    (`docs/reference/dbrow.md`), and the object wrapper (`docs/reference/object-wrapper.md`). The last of
    those was the blocker, and `eve-proto::MachoPacket::to_payload` now emits a
    complete packet.
  - **The server completes the client's whole login** (`docs/reference/server.md`): all ten
    GPS handshake payloads, H1..H10. They turned out to contain no class
    instances at all, which is why this did not have to wait for the wrapper.
  - **The jail is wired end to end** (`docs/guides/jail.md`): stub resolver, one-command
    bring-up, and the one client setting that must change.
  - **The service layer answers the calls up to character select**
    (`docs/reference/server.md`), including `charUnboundMgr.GetCharacterSelectionData`, and
    the session carries a role mask with the GM bits set
    (`docs/reference/session-roles.md`). The row and rowset layer is built
    (`eve-rowset`), so M2's data shapes have somewhere to go.
  - **The client has now met it, repeatedly.** The wire format survived
    contact: the handshake, the session notification, the packet layout and
    the marshal tag space all read correctly off a real connection, and the
    corrections needed were additions (payload-level zlib, tags 0x11 and
    0x2F, substream call payloads, an optional callID) rather than
    rethinks. The login completes and the client asks for its characters,
    gets them, and primes their names.
  - **The protocol is now closed, including the parts that needed a real
    client to settle.** Bound objects work end to end — resolve, bind with a
    fused first call, the `0x19` object reference, and calls *on* a
    handed-out object. The shared-object table turned out to live at the end
    of a stream rather than after the header, which only one payload in a
    thousand reveals. Both are in `docs/archive/marshal-tags.md` and
    `docs/reference/bound-objects.md`.
  - **What is left is service surface, one hang at a time.** It is not
    protocol: an unanswered `CallReq` blocks a tasklet, so every method a
    screen touches has to exist before it draws. That loop, and the
    instruments for it, are in
    [docs/guides/debugging-the-client.md](../guides/debugging-the-client.md) — read it first, and
    [docs/archive/entering-the-game.md](entering-the-game.md) for the station path.
  - Still owed from M0/M1: the empirical override test (drop one loose `.py`,
    confirm it wins) and frame-level bit-29 compression, which no real
    client has yet sent.

- **M2 — docked in a station. The hangar draws.** The session change lands
  the character in a station, the station UI builds, and the hangar view
  loads and renders: `HangarView LoadView called on view hangar` followed by
  `view ViewInfo(view=HangarView(name=hangar)) opened` in the client's own
  log, with the Ammatar Consulate interior on screen. Every call in the
  blocking wave is answered, and so is every call the view needs.

  Three things got us the last stretch, and only one was a shape:

  1. **Never answering with silence.** Unknown calls now return `None` and
     log it (`dispatch::unhandled`). That turned one gap per run into a
     worklist — a single run surfaced twenty, ten of which came back as
     tracebacks naming the consuming frame.
  2. **`invbroker::inventory.GetSelfInvItem`.** `containerHangar` is one of
     nine magic ids that make `invCacheContainer` ask the server what it is,
     and `_SetItemData` dereferences the answer with no guard. The container
     is the station.
  3. **The art was missing.** 41% of the client's indexed resources were
     absent — EVE streams them, and a client that has never rendered a hangar
     has never downloaded one. `hangarView.LoadScene` unpacks
     `scene, _ = sceneManager.LoadScene(...)`, which returns `None` for a file
     it cannot open, so a missing texture looked exactly like a shape we got
     wrong. `tools/check-resources.sh` exists so that question can be
     answered in one command instead of a debugging round.

  ~~What is left in M2: the **ship**. The character has none~~ — **stale, and
  the ship has landed.** `Character::ship_id` is an `i64` rather than an
  `Option` (`service.rs:426`), an item is created for it (`:767`), there is a
  `capsule_dogma_row`, and `docs/archive/entering-the-game.md:89` says it plainly:
  "`shipid` is now sent, and the character is in a capsule. The hangar draws."
  Confirmed independently by two client runs on 2026-10-23, which read
  `session.shipid = 100000001` and `GetInvItem(100000001).charID = 90000001`
  out of the live client.

  What is actually left in M2 is **inventory and fitting proper** — and note
  that the fitting *mutators* are a storage increment, not a cheap win; see
  the struck list below for why answering them without persistence would be a
  lie. Minimal slash/role sandbox in here, not later — it doubles as the test
  harness.

- **M3 — undock.** Destiny: the server-authoritative space sim the client
  mirrors. Native in `_destiny.dll`, no source to port — the hard wall.
  Undocking itself now works, as far as a client in space with no ship in it.
  **The push path is a hard prerequisite for the ship, and that is now
  proven rather than suspected** — see `docs/archive/destiny-and-the-ballpark.md`.
  It is *not* a prerequisite for two players seeing each other in a station,
  which is why it is step D and not step A in the spine below.

- **M4+** — combat/dogma, skills, market, industry, multi-session for friends.

## Where the testing layer stands, 2026-10-24

Put first because it changes what the rest of this document costs: **most of
what used to need a human now does not.**

Two layers, and they answer different questions:

| layer | what it is the oracle for | state |
|---|---|---|
| `tools/client-ci.sh` + `tools/client-ci/scenarios/` | **the wire and the real client.** Only the client can say our bytes are right | 7 scenarios, 115 checks, all green. One is two-player (`--with-second`, prefix 8501) |
| `server/crates/eve-bot` | **behaviour, interaction, timing.** Many actors, cheap, no Wine | milestones 1-4 of `docs/archive/plan-bots.md` done |

**Neither replaces the other, and the reason matters.** `eve-bot` marshals
with `eve-marshal`, exactly as the server does, so a shared encoding mistake
passes every bot test and agrees with itself fluently. The client suite stays
the gate for wire correctness; the bot is for everything a Wine client per
player cannot reach.

What the bot settled in one evening, none of which needed a person:

- **§30.3 promoted `[A]` → `[V]`.** A pilot binding mid-warp is handed the
  warping ship already parked at its destination: 11 m from the computed
  arrival point, 582 m from where it left, `mode = Stop`.
- **§30.3b, new.** `on_ballpark_bind` rewrites the actor's position from
  `undock_motion` on *every* bind, so a second bind is a server-side teleport
  home. Sound for the client (one bind per undock); an assumption for anything
  else, and `/tp` does not survive one.
- **The scheduled push queue, end to end over a socket.** The clock honours
  its schedule to 0.63 s against `warp_flight_seconds`.
- **A lock is deferred and timed**: `(1, [])` then `OnTarget` at 5.90 s, with
  the server-side lock recorded immediately so `GetTargets` is right during
  acquisition.
- **Human step 1 off the list** (two bots, 739.6 m apart), **steps 4, 5 and 8
  half off** — the commands and the timing are ours; the trajectory and the
  progress bar are still eyes.

What is left for a human is in `docs/guides/human-checks.md` §8 and is now short: one
visual baseline (does the warp tunnel draw), the real GPU versus lavapipe, and
"does it feel right". Everything else in that file is automatable and listed
as such.

**The observability lesson from the same evening, because it cost three
rounds:** a green instrument is not evidence until you have seen it move. The
tick-health line was added, reported nothing for a run, then reported only the
idle ten seconds before the client connected, and read `late 0.1 ms, over
budget 0` identically six times — healthy by construction. The real loaded
baseline is `world lock max 6.2 ms, late max 1.8 ms`.

## The layer below this page

**The spine below is complete** (A–E all landed), and the backlog beside it is
a doctrine rather than a queue. The packet-level plan that sits under both is
[`docs/archive/plan-subsystems.md`](../archive/plan-subsystems.md), written 2026-10-24 from four
survey passes over the tree. It carries the things this page deliberately does
not: one source of truth per subsystem, the demotion list for the 16 documents
that are stale or contradict themselves, the measured parallelism ceiling, and
the packets themselves.

It also records that **this page's own "why not system-by-system" argument has
expired** — both of its premises moved when the static corpus opened — which
is a correction to this file and belongs here by rule 3 of the lane
convention.

## The plan from here: a spine, and a backlog beside it

Written 2026-10-19, after a day of fixing the client one relaunch at a time.
That loop works and is not finished, but it is **not the thing that gets us to
the goal**, and this section exists to keep the two apart.

### Why not system-by-system

Everything fixed on 2026-10-19 — wallet, character sheet, contracts, map,
station services, slash console — is **docked-station UI**. All of it real,
all of it correct, and none of it moves toward co-op PvP in space with bots.

Breadth-first has a structural problem here too: each window that opens
reveals its own next layer of calls, because a window that cannot draw never
gets to ask for its contents. That is an unbounded tree. The client
references 175 remote services and we answer roughly 40. We could run that
loop for weeks, feel productive every round, and still have one character
docked in one station with nothing persisted.

The most valuable thing found that whole day was not a shape. It was **"a
visible ship requires a server-initiated push, proven four independent
ways"** — architecture, found by one agent going deep instead of wide.

### The spine

Each link is a hard prerequisite for the end goal and each is small. Order
matters, and it is not the order it looks like.

**A. Share the world. — LANDED, 2026-10-20.** `Arc<RwLock<Registry>>`, built
once in `bind`. `serve_one` used to call `Registry::new(config.sandbox.clone())`
per connection, so two clients were two unrelated universes and a reconnect
discarded everything.

**The lock is acquired exactly once per payload, in `on_payload`, and passed
down as `&mut Registry`** through nine handlers. That is not tidiness. The
`session-and-binding` lane established that the client gives us *no ordering
guarantee* between a session change and the answer to the call that caused it
-- two mechanisms that do not interlock, and the one field that could have
correlated them is unreachable from the wire. So there is no protocol excuse
for computing them against different worlds, and `handle_bind` does exactly
that on the undock path if the lock is taken per-arm. The hazard
`docs/reference/world-state.md` §5 warned about is therefore closed by construction rather
than deferred.

And **the world now survives the process** (`docs/reference/world-state.md` §5 step 2): a
`Store` trait, a line-oriented snapshot behind `--world FILE`, and a test that
drops the entire `Server` and rebuilds from the path on disk. That was the
clause making three of tonight's other steps quietly useless -- `tools/up.sh`
restarts the server, so every grant died with it.

**B. Split identity. — LANDED.** Audited 2026-10-23 by the `identity-spec`
lane, which was briefed to plan this step and wrote an audit instead because
the work was already done: `Session.client_id`/`character_id`,
`Request.actor: Option<i64>`, `Registry::character(id)`, `session_table.rs`,
a process-global `NEXT_OBJECT_ID`, the journal owner filter, and per-character
bio and mission state — all in, all with tests.

**Every claim this paragraph used to make was stale.** Kept, struck, with what
measurement replaced it, because the stale version misled the overnight plan of
2026-10-23 into making step C its headline:

- ~~"three independent hardcoded `user_id: 1`s"~~ — the count was right and the
  diagnosis wrong: one source (`handshake.user_id`) with two derivations
  (`server.rs:91,95`) and three emissions. The *layer* was never confused, only
  the arity.
- ~~"with no test tying them together"~~ — `server.rs:552-560` already asserts
  all three are 7.
- ~~"twelve `characters.first()` sites"~~ — 29 when that was written, **0 live
  today**; the two survivors in `service/reprocess.rs` are inside
  `#[cfg(test)]` from line 514.
- ~~"`docs/reference/world-state.md` §7 enumerates every constant"~~ — **§7 does not exist.**
  That file ends at §6 plus "Risks worth naming". A lane following this pointer
  burns a round. `docs/lanes/identity-spec.md` is the enumeration now.

`ship_id` **is** still the hangar row's `itemID`, and that is now deliberate
and asserted (`service.rs:745-756`, with a `#[should_panic(expected = "share
ship_id")]`): TQ's own session builder defines `shipid` as a hangar item's id,
so this must not be "fixed". The residual risk is the *allocator*, not the
roster — whether `Store::next_item_id` can collide with `base.ship_id + i` is
**unknown** and worth one hour.

**What is actually left, and nobody had named it as the gap: the account
layer.** `SandboxConfig.user_id` is per-**process**, `Character` has no account
field at all, and `character_selection_data` (`service.rs:2041+`) hands **every
character in the world to every connection with no filter** — verified. Two
clients today are two connections, two sessions and two characters, but one
account, and either may pick either character. Also `SessionEntry.user_id` is
written (`server.rs:387`) and read nowhere, which makes the account layer look
present when it is not.

**This needs a decision before an edit**, which is why the lane parked it: there
is no login to authenticate a character selection against, so "which characters
may this connection see" has no answer yet that is not invented. Owner's call.

**And the sharper reason this paragraph used to give for sequencing B before C
is not a reason.** `machoNet.GetClientSessionID()` is
`(clientSessionID, session.userid)` — `[V]`, and the other half is
`machoNet.clientSessionID`, a client-**local** counter of `ConnectToServer`
calls (`machoNet.py:557` init, `:1862` increment, `:3634` return). The token is
stamped as `clisid` (`ObjectCallGPCS.py:644`), carried at `:730` and compared at
`:753` (`COMPARE_OP 3`, raising `RuntimeError('This object was acquired by a
different session')` at `:754`). **The comparison never crosses a process** —
`clisid` is not in the wire payload, and across all 12,774 `.pyc` the symbol
appears in exactly one module. So bound-handle invalidation does not constrain
the identity split in either direction. What it really guards is a reconnect
inside one client process, which our per-connection `bound_objects` already
handles.

**C. A session table, and `station.GetGuests` off it. — LANDED, and the
observable is PROVEN IN THE CLIENT as of 2026-10-23.** Two clients, two
characters, one station: each sees the other in the guest list, the panel's own
row transform builds a `Guest` for both, and the row's field order matches what
`guests.py` unpacks. 9/9 from both sides —
`tools/client-ci.sh run lobby_guests --with-second -- --character Alice --character Bob`.
"You log in, a friend logs in, you open the station lobby and see them" is no
longer a plan; it is a test that passes.

**The detail below remains accurate.**

Audited 2026-10-23 by the `session-and-binding` lane, independently of
`identity-spec` reaching the same conclusion about step B. `session_table.rs`
exists and is wired (`Registry::sessions`, `service.rs:693`); `GetGuests` is
off it (`service/world.rs:226` via `characters_at_station`); `GetOnlineStatus`
is honest (`service/social.rs:228`); `client_id` and `session_id` are per
connection (`server.rs:292`, `:321-325`); the guest-arrival push and
cross-connection routing are built.

Two corrections to what this paragraph used to say. `multiLoginBlocker.Login`
returning `true` is **correct where it sits**, not a hardcoded lie. And the
first observable is no longer the thing to build — it is the thing to *fix*,
because what remains after C is a short ordered list rather than an increment.
Full evidence in `docs/lanes/session-and-binding.md`; the order is what breaks
first:

1. **A departure was invisible until somebody spoke. — FIXED 2026-10-23.**
   `pending` is drained by `dispatch.rs` after a payload and a disconnect is
   not one, so the announcement waited on any connection's next call; and the
   client cannot rescue it, because `StationSvc.GetGuests` caches per station
   behind `guestListReceived` (`station/base.py:135`), so reopening the lobby
   makes no call at all. Now on `Registry::schedule`, drained by the world
   ticker on the clock. Same for the `BallsRemoved` of someone who quits while
   undocked. Test fails without the fix.
2. **Two connections can still be the same character and nothing says no** —
   `handle_select_character` (`dispatch.rs:987-1043`) checks only that the id
   is real. Related: `stamp_logoff` runs unconditionally (`service.rs:1561`)
   *before* the `is_online` guard at `:1573`, so the first window to close
   back-dates a character who is still playing — and that field orders the
   select screen.
3. **A non-zero `rolesAtAll` freezes the character-select click**
   (`invCache.py:162`'s blocking `officeManager` call inside the handler
   chain). We are safe because the value is zero, not because the key is
   absent.
4. **A handle stamped with a *falsy* session value dies locally, before the
   wire** — `ObjectCallGPCS.py:674` is `if current != expected or not
   expected: raise`, so a falsy `expected` always fails. The moniker's own
   check at `moniker.py:87` has an extra `and current` and does not.
5. **Every session change we push arms the client's own 10-second limiter** —
   `__ChangeAttribute`'s first statement (`basesession.py:1422`) sets
   `nextSessionChange = now + SESSIONCHANGEDELAY`, gated at
   `sessions.py:484-489`. Bites the first flow that chains "select character,
   then board".

**D. The push path. — LANDED, 2026-10-19.** `TcpStream::try_clone()`, a mutex
on the write half, and **one global ticker** — not one per connection.
Designed in full in `docs/reference/push-path.md`; increments 1 to 3 of that document are in,
and increment 4 (fan-out, the two-phase ticker) lands with step A because it
needs the world lock to have something to be two-phase about.

What is true now: `/giveisk` answers the console and tells the client in the
same flush, and `/pushtest [seconds]` queues a notification for the ticker to
write seconds later, **with the client having sent nothing in between**. Both
are integration-tested over a real socket, and the second fails by timing out
rather than passing vacuously if the ticker never writes. `/pushtest` stays
forever as the one-line answer to "is the push path alive".

The rest of this entry is kept as written, because it corrects two things that
were on this page and the corrections are the useful part:

- *One ticker per connection* was wrong. The clock is world state; only the
  write handle is per-connection. Step A's shared world is the argument, and
  the stranded-process lesson in `tools/up.sh` is the second.
- *Prove the plumbing by sending nothing* was wrong. A handler-less `On*`
  broadcastID is provably inert on the client and exercises the whole path —
  frame, mutex, capture, client decode, four GPCS layers, dispatch. A ticker
  that sends nothing only tests the sleep.

The first push was **`OnAccountChange`, driven by `/giveisk`**: three scalar
arguments, no DBRow, no item store, no Destiny, no new service surface, and
the state already existed and already moved. It deleted an apology, and the
apology was worse than it looked: the arm's comment said to reopen the wallet,
which could not have worked, because `WalletSvc.Run` caches
`GetCashBalance(0)` once and a reconnect builds a fresh `Registry`. The grant
was unobservable by every route. It is now observable by the route the client
itself uses: `WalletSvc.OnAccountChange` updates its own `_wealth` from the
pushed balance and does not re-ask us at all.

Four ways a push can hang or close the client were found, each one field,
and **three of them were not written down anywhere before**: a non-zero
`oob['sn']` parks the tasklet forever; a forking source address closes the
transport with no explanation; command 12 takes the session read lock, so a
push landing mid-session-change stalls. The fourth is a write to a
closed-and-reused fd.

And one property worth relying on: `ExceptionWrapper.NotifyUp` wraps the
whole receive chain in a bare `except Exception: log.LogException(...)`, so
**no push we can send can kill the client's receive tasklet.** The worst
case for a malformed push is one traceback in the client log — the same "a
wrong answer is evidence, silence is not" property §1 of
`docs/guides/debugging-the-client.md` relies on, now extended to the push direction.

**E. The Destiny state blob**, and a ship visible in space.

> **CORRECTION, 2026-10-23, later the same morning: the red test below does
> NOT prove the gap it was presented as proving, and this section overclaimed.**
> The two clients' lifetimes barely overlap. A scenario writes its verdict and
> then quits, so in the `park_census` run the first client undocked, took its
> census and exited before the second had even finished booting — measured on
> the `lobby_guests` run that found this, where the second client's process
> exited **96 seconds before** the first's. Neither was in space while the
> other was, so "0 other ship ball(s)" is what a correct server would also
> answer.
>
> **And the claim about `BallsAdded` was simply false — both times I made it.**
> `BallsAdded` exists in full: the struct at `destiny.rs:2281`, `new` at
> `:2290`, `impl crate::notify::Push` at `:2325`, and it is **sent to
> observers** at `:1779-1790` — `BallsAdded::new(observers, stamp, Park{..},
> vec![(slim, DamageState::FULL)])` followed by `w.push(added)`, carrying the
> park, the slim item and a full damage state. Arrivals *are* broadcast.
>
> The error was mine and it was mechanical: the round-4 grep ended in
> `head -8`, and the first eight matches were `roster.arrive` test lines and
> two comments, so the definition never printed. **A truncated grep
> manufactures false negatives**, and I then wrote the false negative into
> this file as a measurement, and repeated it in the correction that was
> supposed to fix it.
> The gate is invalid until the harness makes the two clients' presence
> overlap — see `docs/lanes/ci-harness.md` §11. This is the same
> incompletely-verified-claim failure the lanes have twice been burned by, and
> the coordinator committed it this time.

~~**Now gated by a red test, 2026-10-23**~~ — this stopped being an assumption and became a
reproducible failure the moment two clients could be run at once:

```
tools/client-ci.sh run park_census --with-second -- --character Alice --character Bob

  FAIL the second character's ship is in the park
       expected a ball whose slimItem.charID is 90000002, found [].
       0 other ship ball(s): []
  ... and symmetrically, from the other client, for 90000001
```

Both clients undocked in solar system 30000001; each sees 19 balls — the
celestials, a structure, and **only its own ship**. The gap is precise:
**`BallsRemoved` exists as a `notify::Push` and there is no `BallsAdded`.**
A departure is broadcast to observers (`service.rs:session_closed`, and since
2026-10-23 on the clock rather than on a turn), and an *arrival* is broadcast
to nobody — so an observer never learns about a ship that enters space. It is
the exact mirror of the departure bug fixed earlier the same night.

`roster.arrive` already exists and the roster already answers
`others_in_system`, which is what the departure path uses, so the missing
piece is the push type and its state payload rather than the bookkeeping.

**Second correction, 2026-10-19, and this one moves D to the front.** An
enumeration pass over the six remaining docked systems found a fact that
changes the ordering: **every inventory, fitting, market and industry
*mutator* discards its return value.** `tryFit.FitModule` is
`shipInv.moniker.MultiAdd(...)` followed by `POP_TOP`, and the same is true
of `TrashItems`, `StackAll`, `SplitStack`, `InstallJob`, `CancelJob`,
`PlaceBuyOrder` and the rest. The client learns what happened from a
**server push** -- `OnItemChange`, `OnGodmaItemChange`,
`OnOwnOrdersChanged`, `OnIndustryJob`, `OnAgentMissionChange`.

So answering the mutators costs nothing and buys nothing. Across five of the
six systems the gate on "a flow completes" is the push path, **not shapes
and not persistence** — one conclusion rather than five.

That makes push the single cheapest thing with the widest blast radius: it
gates the ship *and* every act of doing something that the player can see
work. It is also small — `try_clone`, a mutex on the write half, one ticker
thread. So the order is now **D, A, B, C, E**, and the spine below is
written in its original order only because the dependency argument for each
link is unchanged.

~~Three exceptions where a return value alone completes a flow, because the
client updates its own cache from data it built itself, are therefore the
cheap wins to take *between* spine increments:
`charFittingMgr.SaveFitting`/`UpdateFitting`, `corpRegistry.AddCorporation`,
and `agentMgr.DoAction`.~~

**Checked one by one on 2026-10-23, and this reserve is empty.** One is done
and the other two are not cheap — and taking them *as* cheap wins would mean
shipping a lie, which is worse than leaving them unanswered:

- **`agentMgr.DoAction` — already implemented**, `service/character.rs:1470`,
  with tests at `:1769` and `:1877`.
- **`charFittingMgr.SaveFitting`/`UpdateFitting` — not a return value.**
  `service/inventory.rs:866-874` already says why, and the reasoning holds:
  there is nowhere to persist a fitting, and the client's `PersistFitting`
  only acts `if fittingID > 0` — so an **unanswered** save is read as "the
  save failed", which is true, while **an id we did not store is a lie the
  client believes**. The cheap part was never the return; it was the
  persistence, and that is a storage increment.
- **`corpRegistry.AddCorporation` — not a return value either.** There is no
  corporation state in the server at all: `service/corp.rs` answers only
  `Get*` methods and the corp data behind them is CCP's read-only
  `npccorporations.fsdbinary`. Creating a corporation means an id allocator, a
  store, and the founder's own membership changing — after which the client
  immediately asks `GetCorporation` about it and would be told something
  different.

**The general lesson, since it is what produced this list:** "the client
discards the return value" does not imply "the server can answer cheaply". It
implies the client learns the outcome *some other way* — from a push, or from
its next query. If that other way would contradict us, the honest answer is
still no answer. `docs/lanes/unanswered-calls.md` is the right home for calls
in that state.

**The earlier correction, still true and still worth reading: D is not
*structurally* required before C.** The obvious reading is that nothing shared can be seen without a push
path, so push must come first. `station.GetGuests` disproves it — the Guests
tab is **pulled when the player opens it** (`docs/archive/entering-the-game.md`), so the
first multiplayer thing this server can do needs no push path at all. Push is
required for *space*, not for the lobby. Getting that backwards would have
put the hardest architectural change ahead of the cheapest observable.

**And the honest cost of this order:** A and B change **nothing a player can
see**, by design. Expect two or three rounds where relaunching looks
identical. That is the trade for landing a topology change while the client
still draws, where a regression is still attributable — `docs/reference/world-state.md` §5
argues it at length. It is worth saying out loud before choosing, not after.

### The backlog beside it

Shape-fixing continues, but **batched, not discovered one relaunch at a
time.** The pattern that works is enumeration: rather than waiting for a
failure, disassemble a subsystem's whole entry path, scan every module in
that subtree for remote calls, and chase each hit to its gate. Done for the
map on 2026-10-19, which concluded "two calls are missing, and after those,
nothing" — and was right.

So: one enumeration pass per system, producing a complete worklist that can
be fixed in bulk between spine increments. The cost of a pass is one agent;
the cost of *not* doing it is one relaunch per call, paid in the owner's
evening.

### What 2026-10-20 changed about all of this

The "why not system-by-system" argument above still holds, but one of its
premises has moved and it is worth saying where.

**The static corpus is open**, which was listed as an open question in
`docs/archive/recon-v24.01.md` and is now `re/static-corpus.md`,
`re/native-loaders.md` and `re/dogma.md`. 139 of the 149 native tables have a
named, typed, offset layout extracted from the loaders' own generated code;
all five dogma tables decode, including **5,178 modifier rules -- the rule
table of EVE's entire bonus system.** The consequence for this page: *"the
client references 175 remote services and we answer roughly 40"* was the right
way to measure progress when every answer had to be invented. A large share of
those answers are now **lookups**, and the enumeration passes above get
cheaper accordingly.

**And the content question is answered.** It was not on this page because
nobody had asked it: whether EVE's PvE content -- missions, sites, NPCs --
could be recreated at all. It largely does not need recreating. 2,892 missions
join by `dungeonID` into 5,516 dungeons that carry **room layouts and
per-room object lists**, and the whole of CCP's written content is in a
readable pickle keyed by a plain global integer. The server sends a message
id; the client renders CCP's prose.

What is *not* shipped is the part this page would have called the game design
anyway: NPC aggression and target selection, warp-in behaviour, respawn
timing, loot rolls, anomaly generation. That is the same engine as the
player-like bots in the end goal, which makes it one piece of work rather than
two.

**The practical effect on the spine is that the first observable moved.** Step
C -- two clients seeing each other in a station lobby -- is still the cheapest
*multiplayer* observable. But a single player talking to a real agent and
being offered a real mission in CCP's own words needs **no Destiny, no
ballpark, no dungeon in space and no standings**, and most of it is already
answered. `re/agents-missions.md` §1 has the slice; two of its rounds landed
tonight.

### Our own code, as opposed to the client's

Everything above is about what the client needs. The things *we* should
change about how the server is built live in
[`docs/archive/architecture-sweep.md`](../archive/architecture-sweep.md): a bounded `unanswered`
report, a read timeout on the connection, a clock seam, capture replay, the
world split, and the `PyValue::Long` trap that `FILETIME_2100` is currently
papering over. Three items have landed from it so far, and the two that
mattered most were not features but **shared files removed** — the eleven
service modules, and the derived service list — because a shared file is a
cap on how many lanes can work at once.

[`lanes/README.md`](../lanes/README.md) is the convention for those lanes,
adapted from another project: one coordinator, lanes that own *files*
rather than subsystems, one round per commit-base, no lane commits or runs
the gate, and every lane parks a note in `docs/lanes/<name>.md` so its
findings outlive the conversation that produced them.

### Where parallelism pays, and where it does not

- **Research and enumeration: maximum.** Independent subtrees, read-only, no
  merge conflicts. This is where the day's best results came from.
- **The long-lead spike: immediately, in parallel with everything.** The
  Destiny reference blob (`docs/archive/destiny-and-the-ballpark.md`) is the item with the
  longest lead time, and the client ships the *writer*, so a small override
  yields a byte-exact reference from the very code that will parse ours.
- **The spine: strictly serial.** A, B and D all touch `server.rs`,
  `dispatch.rs` and `service.rs`. Three agents there produce three
  conflicting diffs and the merge costs more than the writing.
