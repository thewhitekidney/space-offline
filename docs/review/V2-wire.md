# V2: wire, protocol and session robustness

Lane V2 of `docs/review/README.md`. Read-only review at `3f2741d`. Every
claim is marked **[V]** verified by reading the cited line, **[I]**
inferred, or **unknown**. No proof tests were run for the resource
findings; the lane stopped short of that, so those are [I] and each
proposal names the guard test that should pin it.

## Summary

The framing (`eve-proto/src/frame.rs`) and the marshal decoder's
per-count guards (`eve-marshal/src/value.rs`) are careful: declared
lengths are capped, counts are checked against the bytes left, the
shared table is sized by `mapcount`, recursion stops at 1000 and inflate
stops at 64 MiB. The weak points are around the codec, not in it:

1. The payload is decoded **while the world write lock is held**.
2. Nothing bounds the **decoded** size of a payload. Only the frame and
   the inflated byte size are bounded.
3. A panic in a connection thread **skips the session cleanup**.
4. A single `accept` or `spawn` error **ends the accept loop for good**.
5. Some error paths leave a client `CallReq` **with no answer**, and the
   client then hangs.

Items 1 and 3 cost the most for the end goal: they let one bad client
(buggy, modded or hostile) freeze or corrupt the session for every
friend at once.

## Findings

### F1. Decode and inflate run under the world write lock. High. [V]

- `dispatch.rs:1621` takes `world.write()`.
- Then `handle_payload` runs `loads(payload)` (`dispatch.rs:1678`) or
  `observe` runs it (`dispatch.rs:1578`).
- `loads` (`wire.rs:41-49`) inflates up to `eve_inflate::MAX_OUTPUT` =
  64 MiB (`eve-inflate/src/lib.rs:40`) and then decodes it.
- This includes the **pre-auth** path. The handshake payloads go through
  the same `on_payload`.

**Cost:** any client, before login, can make every other connection and
the world ticker wait for one large inflate and decode. A legitimate
large payload has the same effect, only smaller.

**Change:** split `Dispatch::on_payload` into two steps. First
`loads(payload)` with no lock, then lock and handle the decoded
`PyValue`. `GpsSession::handle_payload` takes `&PyValue`. Do the same
for `CallPayload::from_payload` (`eve-proto/src/call.rs:364`, the inner
pickle decode at `call.rs:254`): hoist it before the lock too.

- Behaviour-neutral. Size S-M.
- **Test:** a loopback test whose `Dispatch` records whether the world
  lock is free while `loads` runs (a hook, or `try_write` from a
  sibling thread in a unit test of the split function).

### F2. No budget on decoded size. High. [I]

- `Decoder::load` pushes one `(usize, u8)` onto `trace` for **every
  tag** (`value.rs:280`), even in production.
- Each element becomes a `PyValue`.
- `SHARED_REF` deep-clones the referenced value (`value.rs:468`). The
  client only increments a refcount.

So the memory one payload costs is a multiple of its byte size, with
nothing holding it to a ceiling. The 64 MiB inflate cap bounds the
input bytes, not the decoded result. Each connection thread can hold
one such decode at a time, and connections are not capped (F4).

**Change:**

- Add a `Decoder` budget: a node count, plus a byte estimate charged on
  every `PyValue` created, **including clones made by `SHARED_REF`**.
  Return `WireError::Malformed { why: "decode budget" }` when it runs
  out.
- Default: a few times `MAX_PAYLOAD`. A real client blob is at most
  5 MB (`frame.rs:33`).
- Make `trace` opt-in (`Decoder::with_trace()`). Only `eve-probe` and
  the tests read it [I]; check with
  `grep -rn '\.trace' server/crates`.
- Lower the server-side inflate cap to match the 5 MB practical ceiling
  (`docs/reference/framing.md` "Size caps").
- Size M. Neutral for real traffic.
- **Tests (eve-marshal):**
  - A payload whose reference fan-out exceeds the budget fails with
    `decode budget`.
  - A real captured client blob still decodes under the default.

### F3. A panic in a connection thread leaves a ghost session. High. [V]

- `server.rs:449` calls `serve`. The cleanup at `server.rs:456-462`
  (`session_closed`, `forget_client`) runs only if `serve` **returns**.
- There is no `catch_unwind` anywhere (`grep -rn catch_unwind` finds
  nothing).
- `[profile.release]` does not set `panic = "abort"`
  (`server/Cargo.toml`), so the thread unwinds.

A panic in any dispatch arm (`docs/reference/threads-and-locks.md` §11 counts
about 100 that could panic, none audited) therefore:

- poisons the world, which is recovered (`dispatch.rs:1621`), possibly
  with a half-applied mutation;
- leaves the character in the session table, so other players see them
  online and `of_character` routes pushes to a dead wire.

**Change:** wrap the body of `serve_one` from `serve(...)` onward so that
cleanup runs on unwind. Either a drop guard
(`struct SessionGuard { world, client_id }` with
`impl Drop { session_closed }`), or
`std::panic::catch_unwind(AssertUnwindSafe(|| serve(..)))` mapped to a
new `ConnEnd::Panicked`. The drop guard is preferred: it also covers
early returns.

- Size S. Behaviour-neutral on the happy path.
- **Test:** a `Dispatch` test double that panics in `on_payload`, served
  over a loopback socket in a spawned thread. Assert that the session
  table no longer holds its `client_id`.

### F4. The accept loop dies on the first error. High. [V]

`Server::run` (`server.rs:285-301`) uses `accept()?` and
`spawn(...)?`. The binary returns that error (`bin/eve-server.rs:400`),
and the process exits. Two kinds of error are routine on a public
socket:

- a transient `accept` error, such as `ECONNABORTED` or `EMFILE`;
- a failed thread spawn.

On top of that, there is no limit on concurrent connections, and each
connection costs a thread plus up to `MAX_BUFFERED` (about 10.5 MiB,
`conn.rs:67`).

**Change:**

- Log `accept` and `spawn` errors and continue. Back off briefly on
  `EMFILE`/`ENFILE`.
- Add `Config::max_connections`, default about 64 (10 humans plus 50
  bots, with headroom). Track a live count in an `AtomicUsize`,
  decremented by a guard in the thread, and refuse above the cap by
  closing the stream at once.
- Size S. Neutral at normal load.
- **Test:** a unit test with an `accept` that fails once (factor the
  loop body into `fn accept_one(&self) -> io::Result<()>`) shows the
  loop continues. A cap test opens `cap+1` loopback connections and
  sees the last one closed.

### F5. A pre-auth connection may sit idle for 600 s. Medium. [V]

`IDLE_TIMEOUT` = 600 s (`conn.rs:36`) is correct after login (keepalive
240 s). Before the handshake completes, the client answers within
seconds.

**Change:** use a short read timeout, about 30 s, until
`Phase::Authenticated`, then raise it. `serve` would need a hook such as
`Dispatch::idle_timeout()`.

- Size S.
- **Test:** a loopback test where a silent peer is dropped with
  `ConnEnd::Idle` before the short timeout plus a margin. Use a
  config-injected duration, so the test does not wait 30 s.

### F6. Paths that hang the client. Medium. [V]

`dispatch.rs:559` (`note_unanswered`'s doc) states it: an unanswered
`CallReq` is a hang. These paths still answer nothing:

- `undecodable call payload` (`dispatch.rs:393-398`). The packet header
  parsed, so we know the call id.
- `unhandled command` (`dispatch.rs:370-372`) when the command is a
  request type.
- `CallReq has no payload` (`dispatch.rs:384-389`). This one is worse:
  it disconnects.

**Change:** when `MachoPacket::from_value` succeeded and the command is
`CallReq`, every failure answers with the same
`MACHONETERR_WRAPPEDEXCEPTION` `ErrorResponse` that `raised`
(`dispatch.rs:566`) builds, carrying a generic exception. The client
then raises and does not hang, and the evidence lands in its log.

- Size S.
- **Test:** a loopback test sends a `CallReq` with a garbage inner
  pickle and asserts that an `ErrorResponse` with the same call id
  arrives.

### F7. Per-connection maps grow without bound. Low. [V]

- `unanswered: BTreeMap<String, u64>` (`dispatch.rs:223`) is keyed by
  formatted error text. That text includes byte offsets and client
  strings, so the keys hardly repeat.
- `bound_objects` (`dispatch.rs:238`) is freed only by the client's
  release notification (`dispatch.rs:657-668`).

Each of these grows by one entry per packet, for the life of the
connection.

**Change:**

- Cap `unanswered` at about 256 keys and fold the rest into an
  `"(other)"` counter.
- Cap `bound_objects` at about 4096 and log when the cap is hit. Do not
  evict: eviction would break live references.
- Size S.
- **Test:** a unit test on `note_unanswered` with 10k distinct keys.

### F8. Recursion depth versus thread stack. unknown.

`max_depth` = 1000 (`value.rs:213`) matches the client. Connection
threads use the default 2 MiB stack (`server.rs:290`, no `stack_size`).
`load` → `child` → `load_body` frames in a debug build may not fit 1000
levels.

Overflowing the stack aborts the **whole process**, not just one
thread.

**Not measured.** The proof is one eve-marshal test that decodes
1000-deep `LIST1` nesting on a `std::thread::Builder` thread with
`stack_size(2 << 20)`, in debug and release.

**Change if it fails:** either set `stack_size(8 << 20)` on `conn-N`, or
lower `max_depth` to whatever the measurement shows is safe. Size S.

### F9. Mostly sound. [V]

- **Framing:** `frame.rs:130-175` refuses unknown flags, oversize and
  bad OOB. `conn.rs:189` caps buffering.
- **Handshake:** checks tuple lengths before indexing
  (`handshake.rs:288`, `:364`).
- **Proto:** `address.rs:257-299` and `packet.rs:447-450` check lengths
  before indexing.
- **Write side:** `WRITE_TIMEOUT` bounds stalls on the write side, as
  `docs/reference/threads-and-locks.md` documents. That doc's F2 and F3 remain
  open and belong to V3.
- **Minor:** `buf.drain(..consumed)` (`conn.rs:156`) is quadratic over
  many small frames in one 10 MB buffer. Low cost. Swap it for a cursor
  if F1's rework touches `serve`.

## Proposals

1. Decode outside the world lock (F1).
2. A decode budget, opt-in trace, and a 5 MB inflate cap (F2).
3. Session cleanup on unwind (F3).
4. A resilient accept loop and a connection cap (F4).
5. A short pre-auth idle timeout (F5).
6. Every parsed `CallReq` gets an answer (F6).
7. Caps on the per-connection maps (F7).
8. Measure the depth-versus-stack question, then fix it if needed (F8).
9. A `tests/hostile.rs` loopback suite, one test per item above, so that
   wire robustness has its own file to grow in. It also answers
   `docs/reference/threads-and-locks.md` §11's open "can an arm panic" question:
   add a dispatch fuzz test that calls every `(service, method)` with
   wrong-typed args under `catch_unwind` and fails on any panic.

## Packets

| Packet | Files | Tests | Size | Neutral |
|---|---|---|---|---|
| P1: lock-free decode (1) | `dispatch.rs`, `conn.rs` (Dispatch trait), `eve-proto/src/call.rs` callers | lock-free-decode unit test | M | yes |
| P2: decode budget (2) | `eve-marshal/src/value.rs`, `eve-inflate/src/lib.rs` (cap const), `wire.rs` | budget and real-blob tests | M | yes for real traffic |
| P3: unwind cleanup (3) | `server.rs` | panicking-dispatch loopback test | S | yes |
| P4: accept loop and cap (4, 5) | `server.rs`, `conn.rs`, `bin/eve-server.rs` (flag) | accept-error, cap and idle tests | S | yes |
| P5: no silent CallReq (6, 7) | `dispatch.rs` | ErrorResponse loopback test, map-cap unit test | S | changes behaviour: errors where it was silent |
| P6: stack measurement (8) | `eve-marshal` test only, then maybe `server.rs` | depth-on-2-MiB test | S | yes |
| P7: hostile suite and arm fuzz (9) | new `eve-server/tests/hostile.rs` | itself | M | yes |

P1 and P5 both touch `dispatch.rs`, so land them in sequence. The other
packets do not collide.

## Do not do

- **Async or a reactor rewrite.** A thread per connection is right at
  60 connections (`conn.rs:4-9`). The problems above are missing
  limits, not the threading model.
- **Server-side authentication or rate-limit machinery in this lane.**
  It is an open owner decision and belongs to V6. A connection cap plus
  a pre-auth timeout is enough here.
- **Raising `max_depth` or stripping the client-mirroring checks.**
  Rejecting exactly what the client rejects is worth keeping.
- **A third-party fuzzing crate.** The rule is std-only. A
  deterministic in-tree generator is enough.

## Unknown

- Whether any dispatch arm can actually be made to panic from client
  input (P7 answers this).
- F8: stack headroom at depth 1000.
- How long the worst-case F1 lock hold really lasts. It needs a
  measurement in an exported copy, which V3 can fold into its lock
  hold-time work.
- How the real client reacts to the F6 `ErrorResponse` for a call it
  considers routine. It is a human check: one line in
  `docs/guides/human-checks.md`.
