# The server

> **Status note, 2026-10-28.** §§1-7 describe the server as of 2026-10-19 to 22, a handshake plus a first service layer; read them as history of how it was built. Verified stale: §2's "what is missing is the station view" (`map.GetStationInfo` and `stationSvc.GetStationItemBits` are answered in `server/crates/eve-server/src/service/world.rs`). What exists now: §8 here, the `DOMAINS` table in `server/crates/eve-server/src/service.rs` (each module's `SERVICES` feeds `Registry::services()`, which is the authoritative list of what is answered; §2's sentence naming `Registry::SERVICES` is out of date), and `docs/port-tracker.md`.

What `server/crates/eve-server` does today, what it deliberately does not do,
and the one step in it most likely to be wrong.

Contents: 1 What works, 2 What does not, 3 The handshake, 4 `signedFunc`,
5 Capture, 6 Running it, 7 What a first connection would tell us,
8 Services, flags and GM commands added by the 2026-10-27 lanes.

---

## 1. What works

A `std`-only TCP listener that accepts a connection from the real EVE client
and runs the **complete GPS low-level version check and authentication
handshake** — the ten payloads H1..H10 of `docs/reference/login-path.md` §4 — then logs
everything that follows.

No external crates — including the DEFLATE decompressor. The client compresses
payloads once they grow past a few hundred bytes, so inflating them is not
optional, and `eve-inflate` implements RFC 1950/1951 directly: a closed,
fully specified format with a published test surface, about three hundred
lines, no `unsafe`, pinned against vectors from the reference implementation
and against the first real client's own compressed frame.

`std::net` and one thread per connection. For one player,
or ten, that is not a compromise: an async runtime would pull in a large
dependency tree -- dozens of packages resolved from a registry, by authors
nobody here has read -- in exchange for concurrency we will never need. The
premise is **not** that we only ship our own code; it is that we keep the
supply-chain surface at zero. Vendored source we have read does not count
against it. `tools/check_deps.py` enforces
the empty allowlist.

The layers, bottom up, each already covered by its own document:

| Layer | Where | Established by |
|---|---|---|
| TCP framing | `eve-proto::frame` | disassembly — `docs/reference/framing.md` |
| payload decompression | `eve-inflate` | RFC 1950/1951, pinned to a real client frame |
| marshal codec | `eve-marshal` | disassembly — `docs/archive/marshal-tags.md` |
| one GPS payload | `eve-server::wire` | `blue.marshal.Save` = header + one object |
| handshake H1..H10 | `eve-server::handshake` | `docs/reference/login-path.md` §4 |
| connection loop | `eve-server::conn` | — |
| capture | `eve-server::capture` | — |

## 2. What does not work

Once the client is authenticated it speaks machoNet, and **every machoNet packet
is an old-style Python instance** — `__getstate__`'s 14-tuple wrapped in marshal
type code `0x17`. That wrapper is now recovered and implemented
(`docs/reference/object-wrapper.md`), so the server can both read a packet and build one:
`eve_proto::MachoPacket::to_payload` produces a complete wire payload.

The **service layer now exists** too, and has been driven by a real client
rather than by this document: `Registry::SERVICES` in
`eve-server/src/service.rs` is the current list, and a test now enforces that
it matches the `match` beside it, because it had already drifted once. The
design point is that **almost every answer is allowed to be empty** — CCP's
client is tolerant here because the real cluster is heterogeneous, so a server
that answers a dozen calls with near-nothing gets a long way.

That tolerance has a sharp edge, found the hard way: `get_active_campaign`
unpacks its result into four names *before* testing any of them, so "empty"
there is `(None, None, None, None)` and a plain `None` is a `TypeError`. Empty
is usually right; which empty is a question for the caller's bytecode. See
`docs/reference/login-path.md` §8.

**Selecting a character works too**, which is a different kind of thing from
answering a call: the point of `charUnboundMgr.SelectCharacterID` is not its
return value — that is discarded — but the `SessionChangeNotification` sent
with it. See `docs/archive/entering-the-game.md` for the field list, which comes from the
real server's own session-building code, and for the four ways to get that
notification wrong that all fail *silently*.

**Bound objects are implemented**, not declined: resolve, bind with a fused
first call, the `0x19` object reference, calls *on* a handed-out object, and
plain calls whose return value is itself an object. One function mints them
all (`dispatch::mint_object`) and `GpsSession::bound_objects` maps an id back
to the service that owns it — necessary because a bound-object call names the
object only by its id string and carries no service field at all.
`docs/reference/bound-objects.md` §3a has the pieces and the two simplifications we have
taken deliberately.

What is missing is the station **view**: `map.GetStationInfo`,
`stationSvc.GetStationItemBits`, and the hangar inventory's contents. Those
do not run in the blocking session-change wave, so the symptom is a loading
screen rather than a black one — a distinction worth knowing, because it
tells you which half of `docs/archive/entering-the-game.md` to read.

An unanswered call does not disconnect us — but **it is fatal to the client**,
and an earlier version of this paragraph said otherwise. The correction is
worth keeping visible, because believing it cost three debugging rounds:

> `RemoteServiceCallWithoutTheStars` blocks its tasklet until a response
> arrives. A call we do not answer is a **hang**, not a no-op: the client
> shows a black screen, logs nothing, and keeps the connection open — which
> is exactly why the old end-of-session summary never printed.

So the server now says `[svc] UNANSWERED <service>.<method>/<argc>` the moment
it happens, as well as keeping the list in `GpsSession`. Treat a silent client
as a missing answer first and a malformed packet second.

A type code we cannot read is still merely recorded, and that *is* narrow now:
with the object wrapper handled, payload-level zlib inflated and tags `0x11`
and `0x2F` implemented, nothing unread has turned up in several full logins.

## 2a. The session, and the one value that matters most

After H10 the server sends a `SessionInitialStateNotification` (command 18,
`params = ('sid', 'sessionType', 'initialstate')`) in the same flush. The
`initialstate` map is flat and applied **verbatim** —
`CoreSession.ApplyRemoteAttributeChanges` does no filtering and no type
coercion — so what we put there is what the session has.

Two things about it are load-bearing:

- **`charid` is absent.** `gameui`'s session handler routes on
  `session.charid is None`, and that is what sends the client to the
  character-select screen rather than into the game.
- **`role` is the Singularity lever.** Omitting it is not neutral: the default
  is bare `ROLE_LOGIN`, with no player rights. The default we send is
  `roles::SANDBOX_SAFE` = `0x6168005200E41000`, which has both GM bits.

**The trap, because two of our own documents fell into it.** `ROLE_ANY` is not
"every role": it is all-ones minus `ROLE_IGB` and the `ROLE_EXT_GM*_PLUS`
masks, and `ROLE_EXT_GM4_PLUS` contains `ROLE_GML | ROLE_GMH`. So
`ROLE_ANY & ~ROLE_SERVICE` — the obvious "maximally permissive" mask, and the
one both documents recommended — **switches off the GM tooling this project
exists for**. `roles::ROLE_ANY_EXCLUDES_THE_GM_BITS` asserts the arithmetic at
compile time. Full table: `docs/reference/session-roles.md`.

Never set `ROLE_SERVICE`. It poisons the session rather than degrading it: the
notification carrying it succeeds, and then every *later* session change —
including selecting a character — raises.

## 3. The handshake, as implemented

Ten payloads, each its own frame. `E` marks the ones the client sends through
`EncryptedWrite`/`EncryptedRead`, which under the Placebo crypto pack
(`docs/reference/login-path.md` §3) are the identity — so everything here is plaintext.

| # | dir | what |
|---|---|---|
| H1 | S→C | 7-tuple `(170472, 496, usercount, 24.01, 3619879, 'V24.01@ccp', None)` |
| H2 | C→S | the same, 6 elements, no `update_info` — checked field by field |
| H3 | C→S | `(None, 'VK', hash)` continues; `(None, 'QC')` is a status probe |
| H4 | C→S | `('placebo', {})` — element 0 must be our crypto pack |
| H5 | S→C | the literal `'OK CC'` |
| H6 | C→S | `(clientChallenge[64], requestDict)` — length is enforced, as `Step3` does |
| H7 | S→C | `2`, the password version |
| H8 | S→C | `(serverChallenge[64], signedFunc, {}, responseDict)` |
| H9 | C→S | `(CryptoHash(serverChallenge), funcOutput, funcResult)` — verified |
| H10 | S→C | the authentication result |

Three details are worth stating because each one is a silent failure if wrong:

- **`boot_version` must marshal as a real.** `24.01` as an integer or a string
  is `HANDSHAKE_INCOMPATIBLEVERSION` and the client says nothing more useful
  than that. There is a test asserting the tag, not just the value.
- **`CryptoHash` is varargs**, so it hashes the *tuple* of its arguments.
  Hashing a 64-byte challenge means hashing the 1-tuple containing it. Under
  Placebo it reduces to `str(crc_hqx(marshal.Save(args), 0))` — a 16-bit
  checksum rendered as decimal digits. Getting this wrong produces "Server
  response signature incorrect", which does not say which side miscomputed.
- **`user_logonqueueposition` must stay below 2.** At 2 or more the client
  raises `AutLogonFailureTotalUsers` and stops. Asserted at compile time.

The state machine refuses anything out of order rather than tolerating it. A
tolerant server here would swallow a marshal or framing bug and surface it much
later as an unexplained disconnect, which is the most expensive kind of bug
this project can have.

### The challenge is not random, on purpose

The 64-byte server challenge comes from SplitMix64 seeded from the clock and
the connection id, not from an OS random source. Under Placebo the client's
only use for it is to hand back a 16-bit checksum of it. What the value must be
is *different per connection*, so two sessions are distinguishable in a
capture; it does not need to be unpredictable, and a dependency bought to feed
a 16-bit check would be a dependency bought for nothing.

## 4. `signedFunc` — the step most likely to fail

H8 carries a `signedFunc` that the client runs through `Crypto.Verify`, then
`marshal.loads`, then `eval`. That `marshal` is **CPython's own code-object
marshal**, a different format from everything else here, which is why
`eve-server::pycode` shares no code with `wire`.

A trivial function suffices. `GPS.__Execute` captures stdout and stderr into a
`WriteBuffer`, and if the `eval` raises it swallows the exception, prints the
traceback into the buffer, and returns `funcResult = {}` — which the client
never inspects. So the client needs *something* that unmarshals to a code
object and is evaluable. We send `return None`: 90 bytes, `argcount=0`,
`stacksize=1`, `flags=CO_NOFREE`, `co_consts=(None,)`, `co_code = 64 00 00 53`
(`LOAD_CONST 0`, `RETURN_VALUE`).

This matters because `carbon/server/script/net/handShake.py` — `GetHandshakeFunc`
and `Verify` — is **server-only and absent from the client tree**, so the real
expression is not recoverable from what we have.

**Confirmed:** the bytes parse back cleanly through our own Python 2.7 marshal
parser (`tools/pyc27.py`), consuming the stream exactly, with every field
recovered. The golden bytes are pinned in a test.

**Not confirmed, and not confirmable from here:** that CPython 2.7's real
`marshal.loads` accepts them; that `eval` of the result succeeds; and that
`Crypto.Verify` under Placebo is the identity `docs/reference/login-path.md` infers.
There is no Python 2.7 on this machine and no live client yet. **Expect H8 to
be the first thing that breaks**, and note that a failure there will look like
a disconnect after H7, not like an error message.

## 5. Capture

Every frame, both directions, is written to `<dir>/<unix-ts>-<id>.evecap`
before anything tries to interpret it, and flushed per record.

```text
magic    14 bytes  "EVEOFFLINECAP\n"
version   1 byte   1
repeated:
  dir     1 byte   0 = client->server, 1 = server->client
  micros  8 bytes  u64 LE, since the capture opened
  len     4 bytes  u32 LE
  bytes   len
```

Three deliberate choices:

- **Not a pcap.** A pcap records TCP segments; the thing we want to reason
  about is *frames*, the boundaries our splitter chose. Recording our own
  notion of a frame means a disagreement with the client localises the bug to
  the splitter instead of leaving it somewhere in the stream.
- **Bytes, not interpretation.** An interpretation we have wrong is exactly
  what a capture is meant to catch.
- **`create_new`, never truncate.** A capture is evidence, and the first client
  session is not cheap to retake.

`Capture::create` refuses to overwrite; `capture::parse` treats a short final
record as an error rather than returning what it managed to read, because a
truncated capture means the writer died mid-frame and that is worth knowing.

### Reading one back

`evecap` decodes a capture using the server's own decoder — not a Python
reimplementation, which would be a second thing to keep correct and would drift
from the one that actually has to talk to the client.

```sh
tools/dev.sh cargo run -q -p eve-server --bin evecap -- FILE --tags
```

Against a complete handshake (this is real output, from the loopback test's
capture):

```text
10 frames
[  0]  S->C   43 bytes  (170472, 496, 1, 24.01, 3619879, "V24.01@ccp", None)
[  1]  C->S   42 bytes  (170472, 496, 0, 24.01, 3619879, "V24.01@ccp")
[  2]  C->S   23 bytes  (None, "VK", "name-hash")
[  3]  C->S   17 bytes  ("placebo", {})
[  4]  S->C   12 bytes  "OK CC"
[  5]  C->S  325 bytes  ("ZZZZ...", {"macho_version": 496, "boot_version": 24.01, ...})
[  6]  S->C    7 bytes  2
[  7]  S->C  377 bytes  (<64 bytes b60e...>, (<90 bytes 6300...>, 0), {},
                         {"challenge_responsehash": "64948", ...})
[  8]  C->S   17 bytes  ("39749", "", {})
[  9]  S->C  117 bytes  {"session_init": {"userid": 1}, "proxy_nodeid": 1, ...}
```

Two things are worth noticing in that dump. H7 is the bare integer `2`. And
H8's `signedFunc` is `(<90 bytes 6300...>, 0)` — `0x63` is `'c'`, CPython's
code-object type byte, so the thing the client is about to hand to
`marshal.loads` is at least the right shape.

`--tags` prints a histogram of the type codes that failed to decode, annotated
with what each one is. On a capture of real post-authentication traffic that
histogram is the shortest path to knowing which object-wrapper codes the client
actually uses, as opposed to which ones exist.

## 6. Running it

```sh
tools/dev.sh cargo test -p eve-server       # 47 tests
cargo run -p eve-server -- --help

eve-server --bind 127.0.0.1:26000 --capture captures/ --mode gps
eve-server --mode capture --capture captures/   # answer nothing, just record
```

The default bind is `127.0.0.1:26000`. In a project whose requirement is "no
phoning home whatsoever", a server defaulting to a public interface would be
the wrong default to have shipped even once. The jail (`docs/guides/jail.md`) puts the
client in its own network namespace and points it here.

### Stopping it

**Never by signal if the world matters.** std cannot catch SIGTERM (no
signal handling, no `unsafe`, no libc crate), so a killed server loses up
to `SAVE_EVERY` (5 s) of play. The clean stop (`crate::shutdown`) writes
everything:

- **`touch <world>.stop`** (or `--stop-file PATH`): the clock polls it;
  the server stops accepting, hangs up every session, writes the world
  through the saver, releases the `--world` lock and exits 0. A stale one
  is removed at startup. `tools/stop-server.sh` is the scripts' helper
  (touch, wait, SIGTERM after a timeout).
- **`/shutdown [seconds] [message]`** in game (GM only): a countdown,
  announced with the client's own `OnClusterShutdownInitiated` notice
  [V] (`machoNet.__BroadcastShutdownInformation`), at CCP's marks (an hour,
  15 and 5 minutes before; the client counts the last five minutes down by
  itself). The message goes as `OnRemoteMessage('CustomNotify')` [A].
  `/shutdown cancel` aborts (`OnClusterShutdownCancelled`), `/shutdown
  status` says how long is left; `/shutdown 0` stops at once.

`--mode capture` answers nothing and logs what the client sends unprompted. It
is the right mode when the handshake itself is the suspect, because it removes
our replies from the picture.

### Tests

47, of which 9 drive a mock client over a real loopback socket through all ten
payloads, with the framing layer, the marshal codec, the capture log and the
connection loop all in the path. They also cover a payload split across two
writes, two payloads in one write, a frame declaring an impossible length, the
`QC` status probe, and an unreadable post-handshake packet not killing the
connection.

Five deliberate falsifications were run against them, each verified to have
actually applied to the source before the suite ran:

| Mutation | Caught by |
|---|---|
| `boot_version` emitted as an int | 2 tests |
| dict entries stored key-first instead of value-first | 6 tests |
| `CryptoHash` hashing the bare argument, not the 1-tuple | 3 tests |
| the marshal stream header omitted | 3 tests |
| capture recording only one direction | 2 tests |

What the loopback test does **not** prove is that any of this agrees with CCP's
code. It proves the pieces agree with each other. The payloads are built to
match `docs/reference/login-path.md` §4, and where that document is wrong, the test is
wrong in the same direction.

## 7. What a first client connection would tell us

In rough order of value:

1. **Whether H8's `signedFunc` is accepted** — the one step with an unverified
   premise (§4).
2. **The object wrapper's tag and offset**, reported by `GpsSession` as the
   first thing it cannot decode. That is the observation that closes the last
   wire gap.
3. **Whether the handshake field types are right**, since the client's version
   checks are type-strict and currently only tested against our own encoder.
4. A capture of the real post-authentication traffic, against which the whole
   marshal table can be validated by example rather than by disassembly.

All four come from one connection, which needs the jail up (root) and a
display — the two things still blocked on the owner being at the machine.

## A build note that cost time twice

`tools/dev.sh` mounts the repo **read-only** by design, so anything that
rewrites a source file fails with `Read-only file system` -- `cargo fmt --all`
most often. The writable form is `tools/dev.sh --lock`, which is also what
`cargo` needs when it wants to update `Cargo.lock`.

`cargo test` works without it because `server/target` is the one writable
mount.

---

## 8. Services, flags and GM commands added by the 2026-10-27 lanes (Q-DOCS)

Sections 1 to 7 describe the first connection. Since then the server grew
the subsystems below. Each row names the lane note (`docs/lanes/<lane>.md`),
which holds the client citations, the `[A]`s and the human check; this table
only says where to look.

**Flags** (`bin/eve-server.rs`):

| flag | what | lane |
|---|---|---|
| `--concord` / `--no-concord` | CONCORD in high-sec: the squad, the refusals and the checkpoints. On by default; off turns all three off | `concord-1a`, `concord-1b` |

**GM slash commands** (`slash.rs`, GM characters only):

| command | what | lane |
|---|---|---|
| `/jumpfatigue 0 [characterID]` | clear a pilot's jump fatigue and activation timer (and push `OnJumpTimersUpdated`) | `cap-f` |
| `/dock-capitals [on\|off]` | the capital docking switch: with it on, a Capital hull docks at a null-sec NPC station. Not persisted, default on | `cap-k` |
| `/settle`, `/settle park <career>`, `/settle <game hours> [name]`, `/settle sweep` | lazy settlement of logged-off bot careers: totals, park, settle as if away, sweep | `e1-settle` |
| `/secstatus [characterID] <value>` | set security status (the tag office and the CONCORD checks read it) | `sec-1`, `sec-1b` |

**Services and calls**, by subsystem:

| subsystem | calls | where | lane |
|---|---|---|---|
| CONCORD | none of its own: the crime seam (`crimewatch::ON_CRIME`) opens a response after the security band's delay; refusals ride the warp, dock, gate and wormhole arms | `concord.rs`, `movement_guard.rs` | `concord-1a`, `concord-1b` |
| CONCORD checkpoints | none: idle groups at every high-sec gate answer a nearby crime with no delay | `concord.rs` | `concord-gate` |
| implants | `dogmaIM.InjectImplant`, `DestroyImplant`, `skillMgr2.GetImplants`, push `OnServerImplantsChanged`; per-clone sets in the jump-clone calls | `implants.rs`, `clones.rs` | `implants`, `k-pod` |
| insurance | the payout hook and an expiry sweep with a notification; bots insure through the real calls | `service/insurance.rs` | `x-insure` |
| corp offices | `officeManager` rent and unrent, the deliveries and hangar divisions in the inventory arms | `service/corp_office.rs` | `corp-o` |
| capitals | `beyonce.CmdJumpToFleetModuleBeacon`, `CmdBridgeToFleetModuleBeacon`, `CmdJumpThroughFleet`, `CmdGroupJumpToFleetModuleBeacon`, `jumpTimers.GetTimers`, the fit and hold rules, `ship.GetShipConfiguration` and `ship.ConfigureShip` | `cyno.rs`, `jump_drive.rs`, `jump_portal.rs`, `jump_fatigue.rs`, `dock_policy.rs`, `ship_config.rs` | `cap-b`, `cap-d`, `cap-e`, `cap-f`, `cap-g`, `cap-h`, `cap-k` |
| fighters | `fighterMgr` calls: load, launch, recall, unload; the data table | `fighters_data.rs` and the fighter module | `ft-a`, `ft-b` |
| moons and reactions | reactions as industry activity 9; moon belts as rock fields | `reactions.rs`, `moon.rs`, `moon_belt.rs` | `mn-a`, `mn-b`, `mn-fg` |
| electronic warfare, remote reps | module effects for scram, web, painter, damp, ECM and the repairers | `remote_rep.rs` and the module engine | `k-ewar`, `k-logi` |
| T3 | relic invention, decryptors, subsystem extraction in the industry service | `industry.rs` | `t3-a` |
| sov nodes | command nodes and the node fight | `command_nodes.rs` | `sov-d3b` |
| hardening | caps and checks on the alliance, wallet, fleet-broadcast, killmail and contact calls; the preauth frame limit | several | `sec-fix`, `q-fuzz` |
| saving | the save renders off the lock on the saver thread | `store.rs`, `store/perf.rs` | `perf-save` |
| the map | system and constellation centres for ranges; the d-scan range | `celestial.rs` | `map` |

**Bots** that ride these services (`crates/eve-bot`): the reactor and the moon
preference (`mn-fg`), insurance (`x-insure`), the skill plan (`x-skill`), the
ship pool (`ship-pool`), corp operations (`bot-corp`), the co-founding pool
(`co-found`, `q-flake2`), ammo and priors (`bot-tune`, `gb-fix`), the logi bot
(`k-logi`), the capital ladder (`cap-c`) and the tag hand-in (`sec-1b`).
Every test that drives one over a socket lives in `crates/eve-bot/tests/bots/`.
