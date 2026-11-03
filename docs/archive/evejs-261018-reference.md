# EvEJS 261018.1 — third-party reference implementation: what it answers, what not to touch

> **Demoted 2026-10-24** — 8 of its claims did not survive cross-check against the client. Current truth: [re/evejs-crosscheck.md](../re/evejs-crosscheck.md). Kept for history; do not brief a lane from this file.

**Audience: an agent working on eve-offline.** This is a briefing on a *third-party*
EVE server emulator, analysed statically on 2026-10-20. It is **not** our code and
nothing from it is in this repo.

Read `## 0` before anything else. The rest is a GAP→evidence map against
`docs/reference/login-path.md` §10 and `docs/archive/wire-format.md`, then the facts worth knowing.

---

> **Cross-checked 2026-10-20, and eight of its claims did not survive.**
> Read [`re/evejs-crosscheck.md`](../re/evejs-crosscheck.md) alongside this file;
> its §9 tables every correction. Of 31 claims checked against the client:
> **19 agree, 8 disagree, 3 partly, 1 unknown.** The three that matter most
> here:
>
> - **§1's "client build 3396210 ← SAME TARGET AS OURS" is wrong at the build
>   level.** Ours is **3619879**. Same version, different build, so framing
>   and opcodes carry over and shapes and service lists may not.
> - **GAP 2 is now closed, and better than reported.** `0x7D` is not an
>   alternate magic, it is the marshal **version**, and a non-zero version
>   means there is no mapcount word. That closed `docs/archive/wire-format.md` gap 4 and
>   `docs/archive/marshal-tags.md` gap 10 and exposed a latent bug in our own reader.
> - **GAP 3's "verification flag hardcoded `false` … verification off" is a
>   misreading with an accidentally-correct conclusion.** Placebo's `verify`
>   is `(signedData[0], signedData[1] == 0)` and the raise is *outside* the
>   try, so the second element is a **required zero-equal value**, not a
>   disabled check. `false` works only because `False == 0` in Python 2, and
>   sending `True` — the obvious move if you believe the check is off — kills
>   the handshake with no explanation.
>
> Also struck: the "Bonus — service advertisement" section.
> **`machoNet.GetInitVals` does not exist in V24.01** — zero hits across all
> 12,774 `.pyc` files — so its "~145 vs our 175" comparison is between
> non-comparable populations. The real call is `machoNet.GetServiceInfo`, it is
> **mandatory** (no `except` around it, so an exception aborts login), and it
> is missing from `findings/client-calls.tsv`, which is a genuine hole in the
> login-path inventory rather than a flaw in this document.
>
> §0 below is unaffected and still binds.

## 0. Operating constraints — both are hard

### 0.1 Never execute anything from it

The archive is unsigned, unverifiable, Windows-built, and distributed through a
chat server. Treat it as hostile data. No `npm install`, no `node`, no `cargo`, no
`docker build`, no `.bat`/`.ps1`/`.sh`, no linter that loads its config. Read it
with `cat`/`grep`/`sed` only; any Python you write runs `python3 -I` from outside
the tree with paths passed as arguments.

A full static audit found **no backdoor and no server-side RCE** (see `## 6`), so
this is not fear of that specific archive — it is that nothing is gained by
running it and the whole point of eve-offline's container/jail model is not to.

### 0.2 Never copy from it — this is a clean-room project

`README.md` promises: *"Our own code only. Nothing from CCP's client is
redistributed here — no assets, no scripts, no decompiled source."* Two separate
contamination risks, both of which that promise forbids:

| risk | detail |
|---|---|
| **Licence** | EvEJS is **AGPL-3.0-only**. Its marshal codec is itself ported from EVEmu (`server/src/network/tcp/utils/marshal.js:6-7` cites `EVEMarshalOpcodes.h`, `EVEMarshal.cpp`, `EVEUnmarshal.cpp`). Copying code — or transcribing a function closely enough to be a derivative work — pulls that chain into our tree. |
| **CCP data** | ~505 MB of the 586 MB archive is CCP-derived static data: dungeon geometry *"compiled from EVE client build 3475087"*, decoded FSD exports naming `res:/staticdata/nodegraphs.fsdbinary` and `missions.fsdbinary`, 2,878 missions with verbatim CCP mission prose, 10,941 NPC agent records, 162 faction-logo PNGs, two raw client blobs (`latest.achv`, `latest.chti`), and 9.3 MB harvested from CCP's ESI API. Copying **any** of it breaks the repo's stated position directly. |

Note also what the release deleted relative to its own earlier public snapshot:
`LEGAL.md`, `NOTICE.md`, `ACCEPTABLE_USE.md`, `THIRD_PARTY_NOTICES.md`,
`clean.bat` and `tools/ReleaseGuard/` are all gone, and the old `LICENSE`'s
carve-out (*"No EVE Online client, CCP static data, CCP binaries, CCP
artwork… are licensed by this file"*) was replaced by the plain AGPL text with
zero mentions of CCP. The documents that forbade shipping CCP data were removed
in the same release that started shipping it.

### 0.3 The permitted use

**Hypothesis generation and corroboration only.** EvEJS is a second, independent
reading of the same protocol that reportedly drives the real client. Where it
agrees with our reading of the client's own bytecode, confidence goes up. Where
it disagrees, the client is ground truth and EvEJS is probably carrying EVEmu's
Crucible-era legacy.

Workflow: read a claim here → confirm it in the client's own disassembly → write
our own implementation from the client, citing the client. EvEJS is never the
citation in our docs; at most a footnote that an independent implementation
concurs.

---

## 1. What it is, in one block

```
name            EvEJS / "EVE.js"      version 261018.1 (YYMMDD.build)
target          EVE 24.01, client build 3396210   ← SAME TARGET AS OURS
language        Node.js (CommonJS), ~1,785 .js / 156,640 lines
                + 4 Rust binaries (market daemon + 3 seeders)
tests shipped   0  (290 in the earlier public snapshot; stripped from the release)
posture         README: "localhost-only… not hardened for a LAN, the public
                Internet, port forwarding, shared hosting, or untrusted users"
archive         $DOWNLOADS/EveJS-261018.1.zip
sha256          2bd63d39b95308c7a383e39ec1d749ff82449b4591dcafa70e4e2414ba4aabeb
provenance      unsigned, no checksum shipped, all 2228 entries Windows-built,
                mtimes spread over 69 days ending 2026-10-18
```

**Re-extraction** (the prior extraction lived in an ephemeral session scratchpad):
unzip into a *new empty* directory outside this repo, never inside it. The archive
has no absolute or `..` paths. Do not add it to `.gitignore` as a way of keeping
it here — keep it out of the tree entirely.

Also harvested from *newer* clients than our target: build `3475087` (dungeon
content), `3487903` (AIR tutorial), `3436472` (abyssal capture). Treat
content-derived claims as possibly post-24.01.

---

## 1a. Scope — see `docs/evejs/evejs-subsystem-gap.md`

This document asks whether their *reading of the protocol* agrees with ours.
It never asked how much game they built. That question has its own file now:
**206 service entry points against our 84**, a `server/src/space/` tree we
have no counterpart to, and the three rows of it that change what we do next.
Built from the file listing only — same §0 constraints.

---

## 2. GAP→evidence map (the reason to care)

Against `docs/reference/login-path.md` §10 "Remaining GAPs" and `docs/archive/wire-format.md`.
Confidence labels: **[2ND-IMPL]** = an independent implementation's answer, not
client-verified; **[PARTIAL]** = narrows but does not close.

### GAP 1 — on-wire framing, length-prefix width/endianness — **[2ND-IMPL] answered**

`server/src/network/tcp/directGameSocketTransport.js:118-132`

```
frame  = [uint32 LE payloadLength][payload]
cap    = payloadLength > 0 && <= 10_000_000   (:3, :120-126)
        violation ⇒ destroy socket, code GAME_FRAME_LENGTH_INVALID
```

Reader pauses the socket per `data` event, appends to one accumulation buffer,
drains complete frames strictly in order (`await handleFrame(payload)` at `:134`
means frame N+1 cannot start before N's state transition resolves), geometric
compaction at `:88-108`.

So: **uint32, little-endian, no OOB channel in the framing itself.** This is the
single most useful thing in the archive for us, because it is `SocketGPS.pyc`'s
native behaviour observed from the other side.

### GAP 2 — marshal opcodes / DBRow packed layout / `0x7D` vs `0x7E` — **[2ND-IMPL] answered**

`server/src/network/tcp/utils/marshal.js`, 1,910 lines, the only wire codec on
the MachoNet path.

```
payload layout (:9-12)   [0x7E magic][uint32 LE mapCount][marshaled value]
                         [mapCount × uint32 LE shared-object ids]
opcodes      (:21-64)    PyNone 0x01 … PyVarInteger 0x2f, incl. PyPackedRow 0x2a,
                         PySubStream 0x2b, PyObjectEx1/2 0x22/0x23, cPicked 0x21,
                         PySavedStreamElement 0x1b
flag masks   (:66-69)    SAVE_MASK 0x40, OPCODE_MASK 0x3f, UNKNOWN_MASK 0x80
dict order   (:544-553)  VALUE written before KEY
packed row   (:898-951, :1156-1244)  real bit-packing + RLE
object share (:124-162, :391-432)    implemented but restricted to
                                     blue.DBRowDescriptor by identity
pickle       (:433-447, :1466-1504)  embedded pickle emitted, end found by
                                     walking protocol-0/1/2 opcodes
```

It uses **`0x7E`**, which bears directly on our `0x7D` vs `0x7E` question. Two
decode entry points: `marshalDecode` (lenient, ignores trailing bytes) and
`marshalDecodeExact` (`:1315-1317`). Note the live inbound path uses the
**lenient** one (`network/tcp/index.js:340`).

Cross-check candidate: our `eve-marshal` against this opcode table and the
value-before-key dict ordering. Compare behaviour, not text.

### GAP 3 — `GetHandshakeFunc` shape (not in the client tree) — **[2ND-IMPL] answered**

`server/src/network/tcp/handshake.js:163-167`

```js
const expr = 'eval(compile("' + pyCode + '", "<tidi>", "exec"))';
return buildMarshaledString(expr);
```

Sent in `CryptoServerHandshake` at `:734-735` as `[buildTidiSignedFunc(clientId), false]`
— the second element is the **verification flag, hardcoded `false`** (unsigned).

So the client accepts a **plain marshaled string containing a Python expression**,
with verification off. That confirms our §4/H8 note that only the *shape* matters,
and gives a concrete working shape. We do not need the payload to do anything —
EvEJS uses it for TiDi setup, a portrait-upload monkeypatch and a skill-extractor
token (`:90-162`); an empty-but-valid expression should satisfy the client.

**Design note for us:** this primitive is arbitrary unauthenticated Python
execution on every connecting client. Our server should emit the minimum the
client requires and nothing else — see `## 6` finding S3 for why.

### GAP 4 — `blue.crypto.GetSharedAsymmetricCipher()` / the 512-bit blob — **[PARTIAL]**

EvEJS implements **no asymmetric crypto at all**. `handshake.js:505-585`: on
`CryptoRequestPacket` the server replies with the literal string `"OK CC"`; the
**client then supplies** `crypting_sessionkey` and `crypting_sessioniv`
(`:517-574`), truncated to 32/16 bytes, and AES-256-CBC starts with the IV
chained from the previous ciphertext block (`:851-875`). The `VK` command's
vipKey is read at `:473` and **discarded, never validated**. Author comments:
`:280-283` *"made in attempt for CryptoAPI support… failed horribly"*; `:7`
*"only supports PLACEBO packets"*.

**What this tells us:** a PLACEBO-mode client never exercises the asymmetric
path, so our server can skip it for M1. It does **not** answer what CCP's real
server does, and the 512-bit blob question stays open.

### GAP 5 — real `session_init` contents — **[PARTIAL]**

EvEJS authors its own, same as we do. Its 6-step FSM and the `CryptoHandshakeAck`
it sends (`sessionID`, `user_clientid`, `session_init` dict) are at
`handshake.js:782-847`; immediately after, `network/tcp/index.js:283-309` builds
the session and sends `SessionInitialStateNotification` (type 18) which the
client blocks on (`:571-643`). Useful as a worked example of a dict the client
accepts; not authoritative.

### GAP 6 — bound-object interfaces — **[PARTIAL]**

Mechanism, not the specific `populationCap` interface:

```
OID format     N=${PROXY_NODE_ID}:${nextBoundId++}      (counter starts at 1)
registration   inferred from a handler's RETURN SHAPE — packetDispatcher.js
               :1128-1160 walks results for substruct(substream([oid, refID]))
               and auto-registers
lookup         serviceManager.lookup checks _services THEN _boundObjects
               (serviceManager.js:179-181) — one flat namespace
release        client sends ClientHasReleasedTheseObjects with OID- metadata
               (:838-875), guarded only by refID monotonicity
rebind         a call to a dead OID gets a synthetic UnMachoDestination so the
               client's Moniker rebinds and retries once (:779-790)
```

The `UnMachoDestination` rebind contract is worth knowing before M2.

### Bonus — service advertisement

`machoNet.GetInitVals` returns a **~145-entry `serviceInfo` dict** telling the
client which services exist and their binding class
(`server/src/services/machoNet/machoNetService.js:175-333`). Cross-check against
our `findings/remote-services.txt` (175 services). `GetServiceInfo` returning a
bare `{}` is consistent with our note that it is only a node-id hint.

### Also closes a worry, not a gap

`docs/archive/wire-format.md:345` notes everything is *"still our reading"* of the
client. EvEJS is a **second independent reading that reportedly works against the
real client**. That is meaningful corroboration for layouts we read off the
client's own reader/writer — but mind that its codec descends from EVEmu's
Crucible-era work, so a disagreement is as likely to be their legacy as our error.

---

## 3. Protocol facts worth knowing (beyond the gaps)

```
listener      net.createServer on gameServerBindHost:serverPort
              default 127.0.0.1:26000        (network/tcp/index.js:424-436, :536)
handshake     SEND_VERSION → WAIT_VERSION → WAIT_COMMAND → WAIT_CRYPTO
              → WAIT_AUTH → WAIT_FUNC_RESULT → DONE      (handshake.js:255-263)
step 1        server→client VersionExchangeServer on connect (:299-323):
              [eveBirthday, machoVersion, userCount, boot_version, boot_build,
               projectVersion, null]
step 3        QC (len-2) replies queue position 0 and RESTARTS start() (:464-466)
              VK (len-3) vipKey read and discarded (:473)
step 5        server writes PyInt(2) = "I want hashed passwords", then
              CryptoServerHandshake (config_vals, proxy_nodeid, boot meta, signedFunc)
pinned        machoVersion 496, clientBuild 3396210, eveBirthday 170472,
              clientVersion 24.01            (protocolProfile.js:6-14)
version gate  NONE — mismatch only logs a warning and proceeds (handshake.js:426-433)
inbound       decrypt → opportunistic zlib.inflateSync if data[0]===0x78
              → marshalDecode → dispatch     (network/tcp/index.js:331-368)
call routing  serviceName = pkt.service || pkt.dest.service; bound calls recover
              the OID from the body by scanning for an "N=" string to depth 6
              (packetDispatcher.js:542, :111-162)
method        Handle_<name> first, then bare this[<name>]  (baseService.js:106-128)
responses     CallRsp PyObject "carbon.common.script.net.machoNetPacket.CallRsp"
              with [7, src, dest, userID, [substream(result)], namedPayload, …]
              (:959-1060); undefined normalised to null so the client can't hang
exceptions    MachoWrappedException ⇒ ErrorResponse type 15; anything else ⇒ a
              plain null CallRsp, so a server crash looks like None on the wire
notifications 5 outbound lanes: sendNotification (type 12, broadcast address,
              body [0,[1,payload]], pre-marshaled inner buffer, 'sn' sequence),
              sendServiceNotification, sendObjectNotification,
              sendClientCallRequest (server→client RPC, callIDs from 610000),
              sendSessionChange (type 16)     (clientSession.js:872-1302)
fan-out       iterate sessionRegistry.getSessions() — no subscription model
session ids   clientID = counter × 1e10 + PROXY_NODE_ID(0xffaa) (handshake.js:179-185)
              sessionID = BigInt(Date.now()) * 15n + seed % 15n  (:187-190)
in-session    NO re-authentication anywhere; the session IS the socket
reconnect     no resumption; full handshake, new ids. Login closes the account's
              other sessions with GPSTransportClosed + 5s grace
              (accountLoginClose.js:32-87)
```

**`tickAlignedSends.js` is the most interesting thing in the archive and the
least obvious to derive.** Its header (`:3-23`) records measurements from 16
retail client logs — 2,381 of 2,682 `DoDestinyUpdate` packets arriving within
60 ms of the wall-clock second — and reproduces it: `DoDestinyUpdate`,
`OnMultiEvent`, `OnEnteringDungeonRoom`, `OnExitingDungeon` are held and flushed
by one process-wide `setTimeout(1000 - now % 1000)` (`:66-82`), while call
responses, session changes and `OnItemsChanged` go immediately.
`sendSessionChange` force-flushes first so a departed location's updates precede
the change. Simulation ticks at 100 ms; this is purely an egress shaper. **The
observation is a fact about retail behaviour we can verify ourselves from our own
captures — do that rather than taking their constant.**

Dead ends, so you don't chase them: `network/inboundMarshalDecoder.js`,
`network/tcp/utils/decode/index.js`, `network/tcp/utils/encode/index.js` are
unreferenced; the last has a wrong 9-byte header. `nativeTimeDilation.js` is a
hand-rolled bit-packed "BlueNet" encoder with **no decoder and no traced caller**
— its `0x10000000` frame marker exceeds the 10 MB inbound cap, confirming it is
write-only; reads as researched-but-parked.

**`protobufjs` is not the EVE wire format.** It serves the parallel modern HTTPS
`eve_public.*` gateway APIs that the 24.01 client also speaks; schemas are built
in code via `Root.fromJSON`, there are no `.proto` files. Relevant to our
`findings/proto-messages.txt` work, not to machoNet. `node-forge` is local TLS
cert generation only.

---

## 4. Architecture, if we ever want the shape

```
server/src/network/      framing, handshake, dispatch, session, marshal worker pool
server/src/services/     201 classes extending BaseService, 20+ families
server/src/space/destiny/ {authority,batching,bootstrap,commands,delivery,dispatch,
                          identity,native,presentation,projection,protocol,
                          simulation,stream,visibility}
server/src/edge/         separate worker processes: game, chat, image, redshift
server/src/gameStore/    SQLite (better-sqlite3) + persistenceWorker
server/src/contentPacks/ data-only pack loader (see ## 5)
externalservices/        Rust market daemon, rusqlite bundled
```

Outbound marshal is offloaded to a bounded `worker_threads` pool (≤2 workers,
queue 64, 5 s timeout) for call responses only, with inline fallback and ordering
preserved by a per-session promise chain (`clientSession.js:391-480`). **Inbound
decode is synchronous on the main thread** — their `inflateSync` and uncapped
recursion both run on the event loop. We should not copy that arrangement.

Three `space/destiny/{simulation,batching,visibility}/README.md` files claim the
warp-factor integration is adapted from `carbonengine/destiny` v3.1.1 under an
MIT licence attributed to CCP Games. **That claim is asserted by the archive and
was not corroborated.** Do not rely on it, and do not adopt anything downstream
of it.

---

## 5. One component worth independently reimplementing: the content-pack loader

Audited specifically for a plugin-as-root path; there isn't one, and the parser is
the best-hardened code in the tree. Worth copying the *ideas* (not the code) if we
ever load external content:

- closed capability allowlist, 46 literal strings, unknown capability refused at
  verify **and** install (`contentPacks/contentPackManager.js:26-73, :270-278`)
- `assertSafeJsonValue` (`:96-124`): depth cap 80, rejects non-finite numbers,
  rejects non-plain objects, rejects `__proto__`/`constructor`/`prototype` keys
- size cap 128 MiB checked **before** read; template cap 10,000 (`:16-17, :316-321`)
- no traversal: output path always `<root>/packs/<id>/<version>/pack.json`, with
  `id`/`version` re-validated *at the path-building boundary* (`:353-359`) against
  `/^[a-z0-9](?:[a-z0-9._-]{0,126}[a-z0-9])?$/`; root never derived from content
- atomic write with backup (`:410-431`)
- zero `eval`/`vm`/dynamic `require` of pack content; every `require` literal

Its honest gap: `payloadSha256` lives **inside** the file, so it is
integrity-against-corruption, not authenticity. Their own README says so and
suggests comparing against a digest published out of band. If we sign anything,
sign it properly.

Two other patterns worth lifting conceptually: the **egress proxy is fail-closed**
(blocked hosts refused, then *unhandled* hosts also refused; CONNECT target
derived from the server's own bind address, never the client's —
`_secondary/express/server.js:1026-1056`), and the **local CA** uses
`nameConstraints` with excluded subtrees plus an excluded-subject trick to stop
CN-only bypass on Windows, `0o600` atomic key writes, CurrentUser-scoped install
(`_secondary/express/localTlsCertificate.js:47-70, :382-416`). Both are better
than typical.

---

## 6. Security findings — and which of them are ours to care about (read §6.0 first)

Our server will face the same decisions. Every item verified by reading; `file:line`
given so it can be re-checked.

### 6.0 Threat model first — most of this list does not apply to us

**Decision, 2026-10-20, from the project owner.** EvEJS's "everyone is GM"
posture is **not a defect under our threat model either**. eve-offline is a
private sandbox: invite-only, jailed with no route off the host
(`docs/guides/jail.md`), never publicly hosted. And we already made the same choice
deliberately — `roles::SANDBOX_SAFE = 0x6168005200E41000` has both GM bits and
is handed to **every** session (`docs/reference/server.md` §2a).

So a finding that reads "any client that reaches the port becomes a GM"
describes our design. Their README says the same thing about theirs
(*"localhost-only… not hardened for… untrusted users"*). Read as a defect list
it would send a future agent hardening something we chose on purpose.

The table splits in two, and only the second half was ever a "do not repeat"
list:

**Does not apply — the same design choice we made.** S1 (auth off, auto-create
as GM), S2 (role model fails open), S5 (slash console ungated), S7 (gateway
caller defines identity), S14 (privileged machoNet handlers callable by
anyone), and S4 (password *is* the hash — moot when we have no auth at all; a
missing credential is more honest than a badly stored one). S3 and S9 are about
not pointing a client at someone else's server and not binding wide, which the
jail settles structurally rather than by policy.

One of these converts later. **S7 becomes a correctness problem the moment we
do the identity split** (`docs/archive/roadmap.md`, the three hardcoded `user_id: 1`):
"the caller may define its own `userid`" is harmless with one player and is two
players stomping each other's character with three. Not a privilege boundary —
an ownership check. Worth re-reading S7 at that point for the shape of the
mistake, not the severity.

**Still applies — robustness, not access control.** These bite a three-person
sandbox exactly as hard as a public server, because the attacker is a malformed
frame, not a person:

| id | the hazard | our state |
|---|---|---|
| S11 | uncapped `inflateSync` — a 10 MB frame demanding GBs | **closed.** `eve_inflate::MAX_OUTPUT = 64 MiB`, checked at every store and copy (`lib.rs:375, :478, :498`) |
| S12 | no depth cap in the recursive decoder | **closed.** `max_depth: 1000`, and the number is the *client's own* `ReadObject` limit, not a guess (`value.rs:205-213, :274`) |
| S12b | `Buffer.slice` clamps, so an oversized length silently truncates | **closed.** `WireError::UnexpectedEof { at, need }` — we reject short reads rather than return what we got |
| S13 | no idle timeout, nothing bounded per connection | **partly closed.** `conn::IDLE_TIMEOUT = 600s` with `set_read_timeout`. No connection *count* limit — fine at our scale, and recorded here rather than fixed |
| S6 | empty username → storage path `"/"` → full table overwrite | **not applicable in shape** (our `FileStore` has no name-derived paths) but **the lesson stands**: validate an identifier before it becomes a path. Data loss, not privilege — this one would hit an invite-only server identically |
| S10 | bare-name fallback exposing ~642 methods | **closed by construction.** `DOMAINS` → `SERVICES` → explicit `match`, with tests that the lists are disjoint and match their arms |

So: four of the six already closed, one closed by construction, one a lesson
with no current surface. **The security section is now mostly settled history.**
Its remaining value is the supply-chain finding and the two patterns worth
lifting conceptually (§5), not the defect table.

| id | finding | evidence | why it matters to us |
|---|---|---|---|
| S1 | **Auth off by default.** `devAutoCreateAccounts` and `devSkipPasswordValidation` both default `true` in the schema *and* the shipped config; an unknown username is auto-created with **`isGM: true`** | `config/schema/server.js:5-7, 18-20`; `config/server.json:3-4`; `handshake.js:652-666` | any client that reaches the port becomes a persistent GM. Our defaults must fail **closed** |
| S2 | **Role model fails open.** `const isGM = account.isGM !== false` — absent means GM, and the record is re-stamped on every login | `services/account/accountRoleProfiles.js:207`; `handshake.js:681-683` | default-deny, and never infer privilege from a missing field |
| S3 | **Unsigned Python push to the client** at handshake, verification flag `false` | `handshake.js:163-167, :734-735` | inherent to the signedFunc design; emit the minimum. A server owns every client that connects to it — this is why we must never invite anyone else to point a client at a server we don't control |
| S4 | **Password is the hash.** Client-supplied `user_password_hash` stored and compared verbatim, no salt, no KDF, non-constant-time | `handshake.js:609, :654, :694` | the account file is a file of password-equivalents. Note their own POS code does it right (`services/starbase/starbaseAccessRuntime.js:121-122, 216-222`) |
| S5 | **GM console completely ungated.** `Handle_SlashCmd` → ~144 commands with **zero** role checks (verified: `grep -c "isGM\|hasStaff\|ROLEMASK\|accountRole"` = 0), reachable by any session, advertised to clients | `services/admin/slashService.js:214-266`; `services/chat/chatCommands.js:1076`; advertised at `services/machoNet/machoNetService.js:312` | includes cross-character mutation with no ownership test (`services/chat/commands/characterTargets.js:27-115` accepts any characterID or name) |
| S6 | **Account-table wipe via empty username.** No username validation → write path `"/"` → `getSegments("/")` → `[]` → *"Full table overwrite"* (the code's own comment) | `handshake.js:608` → `services/login/accountStore.js:44` → `gameStore/index.js:837-839, :2220-2238` | **static chain airtight, not executed — high-confidence-unproven.** Validate identifiers before they become storage paths |
| S7 | **HTTP gateway lets the caller invent its identity.** Every caller-supplied session field copied in, only `userid` validated, no key allowlist; result spread into the live session. `slash.SlashCmd` is on the web-call allowlist | `_secondary/express/evejsWebGatewayRuntime.js:4127-4157, :4253-4255, :3676` | token unset by default ⇒ any loopback process is GM. Never let a request body define identity |
| S8 | **Wildcard-CORS drive-by.** Redshift monitor sets `ACAO: *`, answers preflight, no auth on `POST /api/override` and `/api/autoscaler` | `edge/redshift/redshiftEdgeProcess.worker.js:35-40, :81-87, :93-97` | any page the operator visits can reach it. No `ACAO: *` on anything stateful |
| S9 | **Listener binds all interfaces while logging loopback.** `app.listen(port, …)` with no host, printing `http://127.0.0.1:${port}`; unauthenticated config/store writes | `tools/NewEdenStoreEditor/editor.js:179-183, :113-118, :132-145` | always pass an explicit bind host; never let a log line assert a security property the code doesn't |
| S10 | **`BaseService` bare-name fallback** exposes ~642 non-`Handle_` instance methods as client-callable, plus `Object.prototype` members | `services/baseService.js:106-128` | allowlist RPC entry points explicitly |
| S11 | **Unbounded `zlib.inflateSync`** on the hot inbound path; `maxOutputLength` appears **0 times** in `server/src` | `network/tcp/index.js:337-339` | zlib bomb: a 10 MB frame can demand GBs, synchronously. Cap decompression output |
| S12 | **No depth cap in the recursive decoder**; `grep -n depth marshal.js` is empty. Also: `readBytes` uses `Buffer.slice`, which clamps, so an oversized length field silently truncates instead of erroring | `marshal.js:1506-1836, :1375-1379` | our codec needs an explicit depth limit and must reject short reads rather than truncate |
| S13 | **No connection limit, no handshake timeout, no idle timeout** (`socketIdleTimeoutMs: 0` shipped); uncapped audit arrays reachable from the wire | `network/tcp/index.js:424-436`; `config/server.json:31`; `services/sessionMgr/sessionMgrService.js:35-43`, `services/machoNet/tcpRawProxyService.js:25-33` | bound everything that grows per connection |
| S14 | Two privileged MachoNet handlers callable by any client: `SetGlobalConfigValue` (writes server-wide config, broadcasts to all sessions, into a map never pruned) and `ForwardCharacterNotification` (arbitrary payload into any character's session) | `services/machoNet/machoNetService.js:357-422, :450-483` | — |

**Supply chain: clean.** 86/86 and 118/118 lockfile entries with `resolved` +
`integrity`, every URL `registry.npmjs.org`; 4 Rust lockfiles all crates.io with
complete checksums, no `git =` deps; zero vendored or minified blobs tree-wide
(no `.node`/`.so`/`.dll`/`.wasm`/`.min.js`, no long base64); no committed keys,
certs, tokens or default accounts; no `curl|bash`, `iwr|iex` or remote
`Invoke-Expression` across all 48 shell/PS scripts. Exceptions: `better-sqlite3`
pulls `prebuild-install`, which fetches a prebuilt binary from GitHub Releases
**not covered by lockfile integrity** (`server/package-lock.json:131-135`); base
images pinned by tag not digest; the SDE download has no checksum; and
`tools/Updater/Update-EvEJS.ps1:668-684` has an unguarded zip-slip
(`ExtractToFile` with no `..` rejection) on an operator-supplied archive.

**Prototype pollution is structurally closed** on the wire path, deliberately:
the decoder represents dicts as `{type:"dict", entries:[[k,v],…]}` — pair arrays,
never JS objects — with linear-scan `dictGet` (`marshal.js:1707-1709, :1867-1874`).
Good pattern; our codec should be at least as safe.

---

## 7. Deployment facts (only if a comparison is ever wanted)

Not recommended for us — we have our own container/jail model and this would be a
sideways step. Recorded because it was established and is cheap to keep:

- **The game DB needs no EVE client and no Windows.** Built in-container by pure
  Node from bundled static tables + a downloaded SDE zip
  (`docker/entrypoint.sh:56-70`; `tools/DatabaseCreator/database-creator.js:4-11, :36`).
  The server **never** reads the client's shared cache — zero grep hits for
  `ResFiles|SharedCache|index_tranquility|CLIENT_PATH` in `server/src`.
- Container is non-root (`USER node`, uid 1000) under `tini`; no privileged
  container, no Docker socket, no host networking anywhere in `compose.yaml`.
- **Inside the container every listener binds `0.0.0.0`** (`Dockerfile:55-61`);
  the only thing keeping it loopback is the `127.0.0.1:` prefix on each published
  port. The Rust market daemon **forces** `0.0.0.0` when `/.dockerenv` exists
  (`externalservices/market-server/src/config.rs:155-162, 240-250`) and cannot be
  configured back. Combined with S1, anything sharing its bridge network is GM.
- Ports: 26000 game TCP, 26001 image, 26002 gateway, 26003 loopback-CDN HTTPS
  (published on host **443**), 5222 XMPP, 26400 redshift, 40110/40111 market.
- **Client and server must share a loopback.** Not one hardcoded IP: three
  endpoints are byte-patched into `exefile.exe` and
  `launchdarkly_stackless_client_sdk.pyd` at **fixed string length** (≤9 chars of
  host after `https://`) and gated on **exact SHA-256** of the patched file
  (`tools/ClientSETUP/*_recipe.json`;
  `tools/ClientSETUP/scripts/Set-EvEJSClientNetworkPatches.ps1:56-63, :105-106`);
  the server re-enforces a fixed TLS SAN allowlist + `127.0.0.1` **every boot**
  and silently rebuilds the cert otherwise
  (`_secondary/express/localTlsCertificate.js:43, :89-91`); and `Play.bat` re-checks
  `server = 127.0.0.1` on each launch. Satisfiable only with a loopback→LAN
  forwarder on the client machine.

---

## 8. Not determined — do not treat these as settled

1. Runtime exploitability of S6 (chain traced across all six hops, never executed;
   it was not established whether a cache or persistence-role assertion intervenes).
2. Whether XMPP authenticates — no SASL/PLAIN found, but `_secondary/chat/chatRuntime.js`
   (65 KB) was grepped, not read end to end.
3. Whether the CCP/MIT `carbonengine/destiny` licence claim in `## 4` is real.
4. Argument validation (amount caps, overflow) inside the ~144 ungated commands —
   only the *absence of role checks* was verified.
5. Three large PowerShell GUIs (`OpenServerConfigV2.ps1` 271 KB,
   `Update-EvEJS.ps1` 86 KB, `ClientSetup.ps1` 67 KB) were pattern-grepped, not read.
6. `tools/ClientCodeGrabber` and `tools/ClientSETUP/legacy-python/blue_dll_patch.py`
   were not reviewed; they patch the client's `blue.dll` on the host.
7. Built image size (~1–1.3 GB estimated from `du`, not measured).
8. No line-by-line comparison was made between their codec and our `eve-marshal`.
   **That comparison is the obvious next piece of work** and should be done by
   behaviour (same bytes in, same structure out), not by reading their source into
   ours.

---

## 9. Suggested next step for whoever picks this up

Highest value, lowest risk: take **GAP 1** and **GAP 2** above as hypotheses and
confirm them against the client's own reader/writer — `uint32 LE` framing with a
sane cap, `0x7E` payload magic, the opcode table, value-before-key dict ordering.
If our reading already agrees, two long-standing GAPs move from "our reading" to
"two independent readings agree", and `docs/archive/wire-format.md:345` can say so.

Then build the M1 listener from **our** reading, with the `## 6` list as the
things not to do. Cite the client, not EvEJS.
