<!-- Provenance: produced by a read-only research lane on 2026-10-19, from
     this client's bytecode and this tree's code. Kept close to as written,
     rather than paraphrased, because the value is in the specific citations
     and a summary would lose them. The [V]/[I] marks are the house
     convention: [V] read from bytecode or code, [I] inferred and unsettled.
     Nothing here has been run against a client. -->

# Spine step D — the server-initiated push path

Design doc, delivered as text. Marks follow the house convention: **[V]** read from this
client's bytecode or from this tree's code, **[I]** inferred and not settled.

Read-only lane: nothing was written, edited or committed. Note that `server.rs` changed
on disk mid-investigation (coordinator editing `service.rs`/`server.rs`), so the line
numbers below are as of my read; the shapes they describe are the ones I read.

---

## 0. What this settles, and what it does not

**Settled by bytecode in this session, not inherited from the roadmap:**

- The exact `MachoAddress` BROADCAST state tuple, and that a broadcast address has **no
  `service` attribute at all** on the receiving side.
- The exact payload nesting `(0, pickle((0, (1, args))))`, by walking the client's own GPCS
  chain layer by layer and confirming the chain *order* from `machoNet.py`'s builder.
- That the destination is never consulted on the client — `_AssociateWithSession` takes the
  `mode == 'client'` branch with `nodeID = 0` **unconditionally**. The note on record is
  **confirmed**.
- Four separate ways a push can close the transport or hang the client, each with the exact
  line that does it. Three of them are one field each and none is in the roadmap.
- That a notification for an unhandled `broadcastID` is completely inert, and *why*.
- That `ExceptionWrapper.NotifyUp` wraps the entire receive chain in a bare
  `except Exception: log.LogException(...)`, so **no push we can send can kill the client's
  receive tasklet**.

**Not settled:** the Destiny state blob (step E, unchanged), and whether
`notifyAccountChange` defaults on — the first push works either way, via two independent
UI paths (§3.2).

---

## 1. The mechanism

### 1.1 Where the write half lives, and who owns it

`conn::serve` (`server/crates/eve-server/src/conn.rs:65`) owns the `TcpStream` by value and
borrows `Option<&mut Capture>`; `send` at `conn.rs:148` frames one payload, records it, and
`write_all`s it. The read loop blocks in `stream.read` at `conn.rs:131` for up to four
minutes at a time (`docs/archive/destiny-and-the-ballpark.md` §5.3), so **any design where the reader and
the writer share one handle is a design where nothing can ever be pushed.** That decides
`try_clone`.

**The proposal: one new module, `eve-server/src/push.rs`, holding three things.**

```rust
//! The server-initiated write path: pushing a frame to an idle client.
//!
//! `conn::serve` writes only in response to a read, and that cannot stay true.
//! Nothing on the client fetches Destiny state and the client falls silent for
//! minutes at a time (`docs/archive/destiny-and-the-ballpark.md` §5); and every
//! inventory, fitting, market and industry *mutator* discards its return value,
//! so the client learns what happened from `OnItemChange`, `OnGodmaItemChange`,
//! `OnOwnOrdersChanged` and friends -- or not at all. This is the gate on a
//! visible ship *and* on every act of doing something the player can watch work
//! (`docs/archive/roadmap.md`, the spine, step D).
//!
//! Three types, and keeping them apart is most of the design:
//!
//! - [`Wire`] owns **one connection's write half and that connection's
//!   capture**, behind one mutex.
//! - [`Writers`] is the process-wide registry of `Weak` handles the ticker fans
//!   out over. It is **not** a session table: it answers *where do I write*, not
//!   *who is online*. The session table is spine step C and a different thing
//!   with different semantics; conflating them now would make step C a rewrite.
//! - [`spawn_ticker`] is the one clock.
//!
//! **Lock order, process-wide: world > writers > wire.** Acquired in that order,
//! never the reverse, and nothing holds more than two. The only code that ever
//! holds two is the ticker, and its shape is six lines you can read in one go.
//! `docs/reference/world-state.md` §4 rule 4 -- never hold a lock across a socket write --
//! is enforced structurally rather than by discipline; see [`crate::dispatch::Reply`].
```

```rust
/// One connection's write side.
///
/// **The capture lives in here, under the same mutex as the socket, and that is
/// not tidiness.** A capture's whole value is its claim that frame *n* left the
/// socket before frame *n+1* (`capture.rs`): it is the instrument we use to
/// arbitrate a disagreement with the client. Behind a second lock, two writing
/// threads could record in one order and write in the other, and the instrument
/// would be the thing lying. One lock, both, always.
pub struct Wire {
    /// `TcpStream::try_clone`'s second handle. `conn::serve` keeps the original
    /// for reading.
    stream: TcpStream,
    capture: Option<Capture>,
    /// Frames to write later, each with the earliest instant it may go out.
    outbox: VecDeque<(Instant, Vec<u8>)>,
    /// Set when the connection has ended.
    ///
    /// **Not redundant with the `Arc` being dropped.** Two file descriptors
    /// refer to this socket; the kernel closes it when the second one goes, and
    /// the ticker can hold a transient strong handle across a write. Without
    /// this flag the window is small and real: an fd closed and its *number*
    /// reused by a later `accept` would take a pushed Destiny update into
    /// somebody else's session. A bool makes that unrepresentable rather than
    /// unlikely.
    closed: bool,
}

impl Wire {
    pub fn new(stream: TcpStream, capture: Option<Capture>) -> Self {
        Wire { stream, capture, outbox: VecDeque::new(), closed: false }
    }

    /// Write each payload as one frame, in order, with nothing interleaved.
    ///
    /// **The unit of exclusion is the frame, not the byte**, and the guarantee
    /// is structural rather than careful: `write_frame` prepends a length word,
    /// so the whole framed buffer is built *before* any I/O and handed to
    /// exactly one `write_all`, with this `Wire`'s mutex held by the caller
    /// across the call. Two writers interleaving at byte granularity would
    /// produce a stream whose declared lengths do not match its contents, and
    /// the failure would surface on the client as a framing error with nothing
    /// in it that names us.
    ///
    /// A slice rather than a single payload, because a `Reply` can be several
    /// frames whose order *relative to each other* is load-bearing --
    /// `handle_select_character` sends the session change before the answer
    /// (`dispatch.rs`) -- and a push must not be able to land between them.
    /// One call, one guard, one ordering.
    pub fn send_all(&mut self, payloads: &[Vec<u8>]) -> Result<(), ConnEnd> {
        for payload in payloads {
            if self.closed {
                return Err(ConnEnd::DispatchClosed);
            }
            let mut framed = Vec::with_capacity(payload.len() + 4);
            write_frame(payload, &mut framed).map_err(ConnEnd::Framing)?;
            if let Some(c) = self.capture.as_mut() {
                c.write(Direction::ServerToClient, payload).map_err(ConnEnd::Io)?;
            }
            self.stream.write_all(&framed).map_err(ConnEnd::Io)?;
        }
        Ok(())
    }

    /// Record a frame we *received*.
    ///
    /// The inbound direction goes through the same mutex so the capture has one
    /// serialisation point and its record order is the socket's order in both
    /// directions. It costs one uncontended lock per inbound frame.
    pub fn record_inbound(&mut self, payload: &[u8]) -> Result<(), ConnEnd> {
        match self.capture.as_mut() {
            Some(c) => c.write(Direction::ClientToServer, payload).map_err(ConnEnd::Io),
            None => Ok(()),
        }
    }

    /// Queue a frame for the ticker to write once `due` has passed.
    pub fn post(&mut self, due: Instant, payload: Vec<u8>) {
        self.outbox.push_back((due, payload));
    }

    /// Write everything due by `now`. Called only from the ticker.
    pub fn flush_due(&mut self, now: Instant) -> Result<(), ConnEnd> {
        let mut ready = Vec::new();
        while self.outbox.front().is_some_and(|(due, _)| *due <= now) {
            ready.push(self.outbox.pop_front().expect("front() just matched").1);
        }
        if ready.is_empty() {
            return Ok(());
        }
        self.send_all(&ready)
    }

    pub fn capture_count(&self) -> u64 {
        self.capture.as_ref().map(Capture::count).unwrap_or(0)
    }

    /// End the write side. Idempotent, and the only correct way to finish.
    ///
    /// `serve_one` calls this the moment `serve` returns, on every exit path
    /// including a refused payload and a framing error. Dropping the `Arc`
    /// would eventually do the same thing; doing it explicitly is what makes
    /// "the ticker cannot write to a dead connection" a statement about this
    /// function rather than about reference-count timing.
    pub fn close(&mut self) {
        if !self.closed {
            self.closed = true;
            self.outbox.clear();
            let _ = self.stream.shutdown(Shutdown::Write);
        }
    }
}
```

**Answering "who owns it" exactly:** `serve_one` owns the single `Arc<Mutex<Wire>>` in its
own stack frame. `conn::serve` borrows it. `Writers` holds only a `Weak`. That is the whole
lifetime design, and it is why nothing has to be told that a connection ended.

**Answering "the capture must record pushed frames":** the capture moves *inside* `Wire`,
and `conn::serve`'s signature loses its `Option<&mut Capture>` parameter. Every frame in
both directions passes through one mutex, so the capture's `micros` ordering is the socket's
ordering by construction — which is also what makes the verification in §3.3 possible at all.

### 1.2 What happens when a push races a reply mid-write

It cannot, and the reason is two separate properties, both above:

1. **Frame granularity.** `send_all` builds the complete framed buffer (4-byte length +
   payload) before the caller's lock does any I/O, and issues exactly one `write_all` per
   frame. Two writers can therefore only ever interleave *between* frames, never within one.
2. **Reply granularity.** `conn::serve` takes the guard once for the whole `Reply`, so the
   ticker cannot land a frame between a `SessionChangeNotification` and the `CallRsp` it
   belongs to. That matters for more than tidiness: `machoNetTransport.LockSession` takes the
   session **write** lock for commands 16 and 18 and a **read** lock for every other even
   command **[V]**, so a notification landing mid-session-change parks its tasklet until the
   change completes.

`conn::serve`'s loop becomes:

```rust
            let reply = match dispatch.on_payload(&payload) {
                Ok(r) => r,
                Err(e) => return ConnEnd::Dispatch(e.to_string()),
            };
            // One guard for the whole reply, so nothing from the ticker can land
            // between a session change and the answer it belongs to.
            {
                let mut w = match guard(wire) {
                    Ok(w) => w,
                    Err(end) => return end,
                };
                let now = Instant::now();
                let mut frames = reply.payloads;
                let mut later = Vec::new();
                for (after, frame) in reply.pushes {
                    if after.is_zero() {
                        // After the answer, not before it. A `CallRsp` is what
                        // unblocks the tasklet that is waiting on it, and a
                        // notification is non-blocking at both ends.
                        frames.push(frame);
                    } else {
                        later.push((now + after, frame));
                    }
                }
                if let Err(end) = w.send_all(&frames) {
                    return end;
                }
                for (due, frame) in later {
                    w.post(due, frame);
                }
            }
            if reply.close {
                return ConnEnd::DispatchClosed;
            }
```

with

```rust
/// Lock a `Wire` from the connection's own thread.
///
/// **Deliberately does not recover from poisoning, unlike `Writers::guard`.**
/// There is an invariant here and there is none there: a panic inside
/// `send_all` can leave the stream mid-frame, and continuing to write would
/// turn a cosmetic bug into a corrupt stream. `docs/reference/world-state.md` §4 rule 5
/// asks for the poisoning policy to be a decision with a reason attached, so:
/// two mutexes, two policies, both stated.
fn guard(wire: &Arc<Mutex<Wire>>) -> Result<MutexGuard<'_, Wire>, ConnEnd> {
    wire.lock().map_err(|_| {
        ConnEnd::Dispatch("the write side is poisoned: a frame may be half-written".to_owned())
    })
}
```

### 1.3 Shutdown — and the SIGKILL case is not the hard one

The honest version of the lesson from the stranded `eve-dns` and the stranded netns
(`tools/up.sh:205-218`) is: *a previous run left something behind, and the next run met it as
a confusing failure a long way from the cause.* Both of those were resources **outside the
process**. A thread is inside the process, so SIGKILL is the *easy* case: every thread and
every fd goes with it, and there is nothing to reap.

**The hard case is the one that is not SIGKILL: the process keeps running and a connection
ends.** Then a stale write handle can write into a *reused* file descriptor number — a
Destiny update delivered to the wrong player. That is the bug to design against, and it is
designed against three ways, each sufficient alone:

| mechanism | what it rules out |
|---|---|
| `Writers` holds only `Weak`; the one `Arc` lives in `serve_one`'s frame | the registry keeping a connection alive, and a registry leak |
| `serve_one` calls `Wire::close()` on **every** exit path, before logging | a write after the connection ended, whatever the ticker is holding |
| `closed: bool` checked at the top of every `send_all` iteration | a write through a transient `Arc` the ticker upgraded a microsecond before close |

And the thing that makes all of this *simple* is that **there is no per-connection thread to
stop.** Nothing is signalled, nothing is joined, no flag is polled. A connection ending is
expressed by its `Arc` being dropped; the next tick sweeps the slot. That is the single
strongest argument for the next decision.

```rust
fn serve_one(
    stream: TcpStream,
    peer: SocketAddr,
    id: u64,
    config: &Config,
    writers: &Arc<Writers>,
) -> ConnEnd {
    let capture = /* unchanged from today */;

    let write_half = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            // Refuse rather than serve a connection with no push path. A
            // half-functional session where the wallet silently never updates
            // is the most expensive kind of bug this project can have
            // (`docs/reference/server.md` §3): it looks like a shape we got wrong.
            eprintln!("[conn {id}] refusing {peer}: no push path, try_clone failed: {e}");
            return ConnEnd::Io(e);
        }
    };
    let wire = Arc::new(Mutex::new(Wire::new(write_half, capture)));
    writers.register(&wire);

    let mut dispatch: Box<dyn Dispatch> = /* unchanged from today */;

    let end = serve(stream, &wire, dispatch.as_mut());

    // **Before anything else, on every exit path.** After this the ticker
    // cannot write here, whatever it is holding. Poisoning is recovered from
    // here and nowhere else: `close` is the one operation that is correct on a
    // `Wire` whose invariant is already broken.
    let frames = {
        let mut w = wire.lock().unwrap_or_else(|e| e.into_inner());
        w.close();
        w.capture_count()
    };
    println!("[conn {id}] state: {}", dispatch.state());
    println!("[conn {id}] captured {frames} frames");
    end
}
```

### 1.4 One ticker per connection, or one global ticker? — **Global.**

The roadmap's sketch says "one ticker thread per connection". **Change it.** Four reasons, in
order of weight:

1. **The simulation clock is world state, not connection state.** A Destiny update carries a
   simulation time the client *rebases its own ballpark onto*
   (`docs/archive/destiny-and-the-ballpark.md` §8 step 4: `State history rebased at <t>`). Two tickers are
   two clocks; two clocks in one solar system is a desync generator, and the recovery
   machinery for it (`OnRecoverableDesync`, `RequestReset`) is step 6 of that document —
   i.e. we would be building the alarm for a fire we lit ourselves.
   `docs/reference/world-state.md` §5 Step 1 wants one shared world; one world has one clock.
2. **The thing that is genuinely per-connection is the write handle, not the clock**, and
   conflating them is exactly what makes the per-connection version look simpler than it is.
   Separating them costs one `Weak` vector.
3. **Shutdown is strictly easier** (§1.3). A per-connection ticker is a second lifetime that
   must be told to stop, on a path that includes a panicking connection thread. A
   process-lifetime daemon has no such path. This is the generalisation `tools/up.sh:207`
   says "should have been made twice already".
4. **Threads: 1 + N, not 2N.** Irrelevant at ten players, and worth one line of a decision
   rather than a shrug.

The counter-argument — that per-connection is simpler *today* because there is no session
table — is answered by noticing that the registry is **not** the session table. `Writers`
answers "where do I write"; `station.GetGuests`/`multiLoginBlocker`/`onlineStatus` answer
"who is online". The second needs identity, lifetime semantics and a disconnect hook
(`docs/reference/world-state.md` §2 item 2: *disconnect is a write*). The first needs fifteen lines:

```rust
/// Where to write, for every live connection.
///
/// `Weak`, not `Arc`, and that is the whole lifetime design: a connection's
/// strong handle lives in `serve_one`'s frame and nowhere else, so a connection
/// that has ended cannot be written to, and nothing has to be told that it
/// ended. The registry notices on its next pass.
///
/// **This is not the session table.** It answers *where do I write*; the session
/// table answers *who is online*, needs identity and a disconnect hook, and is
/// spine step C. Keeping them apart is what stops step C from being a rewrite.
#[derive(Default)]
pub struct Writers {
    inner: Mutex<Vec<Weak<Mutex<Wire>>>>,
}

impl Writers {
    pub fn register(&self, wire: &Arc<Mutex<Wire>>) {
        let mut v = self.guard();
        v.retain(|w| w.strong_count() > 0);
        v.push(Arc::downgrade(wire));
    }

    /// Live handles, sweeping dead slots on the way past.
    ///
    /// **Drop the result before sleeping.** Each element is a strong handle, and
    /// one held across a sleep keeps a finished connection's write fd open for a
    /// tick -- which is the one way this design could still strand something.
    pub fn live(&self) -> Vec<Arc<Mutex<Wire>>> {
        let mut v = self.guard();
        v.retain(|w| w.strong_count() > 0);
        v.iter().filter_map(Weak::upgrade).collect()
    }

    /// `lock`, recovering from poisoning -- the opposite policy from
    /// `conn::guard`, and for the opposite reason. There is no invariant here:
    /// the value is a list of weak handles and every operation on it is total.
    /// The alternative is that one panicking connection kills the push path for
    /// every other player.
    fn guard(&self) -> MutexGuard<'_, Vec<Weak<Mutex<Wire>>>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}
```

```rust
/// How often the clock wakes.
///
/// Destiny runs at 1 Hz (`docs/archive/destiny-and-the-ballpark.md` §4.2). A clock that
/// wakes at the rate it must emit at has no slack, so this is a divisor: a frame
/// due at *t* goes out within 250 ms of *t* rather than within a full second of
/// it.
pub const TICK: Duration = Duration::from_millis(250);

/// Start the one clock. The thread lives as long as the process.
pub fn spawn_ticker(writers: Arc<Writers>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("push-ticker".to_owned())
        .spawn(move || {
            loop {
                std::thread::sleep(TICK);
                let now = Instant::now();
                // `live()`'s strong handles are dropped at the end of this
                // iteration, which is before the next `sleep`.
                for wire in writers.live() {
                    // `try_lock`, not `lock`. One connection stuck in
                    // `write_all` against a client whose receive window is full
                    // must not stop every other connection's tick -- and a
                    // skipped tick is the cheapest failure available here,
                    // because the client's own ballpark extrapolates between
                    // Destiny updates by design. Anything that *must* be
                    // delivered does not come through this path at all: it goes
                    // out on the connection's own thread, in the reply that
                    // caused it. A poisoned `Wire` lands here as a permanent
                    // skip, which is the right outcome -- that stream may be
                    // mid-frame.
                    let Ok(mut w) = wire.try_lock() else { continue };
                    if let Err(e) = w.flush_due(now) {
                        // One connection's problem. Its own thread will meet
                        // the same error on its next read and end there, which
                        // is where ending belongs.
                        println!("[push] write failed: {e}");
                    }
                }
            }
        })
        .map(|_| ())
}
```

`Server` grows `writers: Arc<Writers>`, built in `bind` beside the other once-per-process
setup, with `spawn_ticker` called there — the same placement `docs/reference/world-state.md` §5 Step 1 picks
for `Arc<World>`. **And the comment at `server.rs:94-95` — *"there is no shared state for a
panicking thread to corrupt"* — becomes false in this commit and must change in it.**
`docs/reference/world-state.md` §0 already flags that comment for the sharing commit; this is the commit
that reaches it first.

### 1.5 The one lock-ordering rule, enforced by the type system

`docs/reference/world-state.md` §4 rule 4 forbids holding a lock across a socket write. Discipline will not
hold that line once `Registry` is behind an `RwLock`, because every dispatch arm will then be
running *with the world write lock held* — and a `push()` reachable from an arm would be
precisely the forbidden thing.

So make it unreachable: **a dispatch arm can only *ask* for a push.**

```rust
/// What to send after handling a payload.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Reply {
    /// Each element becomes one frame, in order.
    pub payloads: Vec<Vec<u8>>,
    /// Frames to **push**: ones the client did not ask for, each with how long
    /// to wait first. `Duration::ZERO` means "in the same flush as `payloads`".
    ///
    /// A dispatch arm can only *ask* for a push; it cannot perform one, and
    /// there is no handle in scope that would let it. That is the type system
    /// enforcing `docs/reference/world-state.md` §4 rule 4 in advance of the lock that
    /// makes it matter: once `Registry` sits behind an `RwLock`, an arm runs
    /// with that lock held, and the only code that can touch a socket is
    /// `conn::serve` (after the arm returned) and the ticker (which never takes
    /// the world lock while holding a `Wire`).
    pub pushes: Vec<(Duration, Vec<u8>)>,
    /// Close once these are flushed.
    pub close: bool,
}
```

And when step A lands, the ticker becomes **two-phase**, which is the only shape that obeys
the rule: phase 1 takes the world read lock and builds `Vec<(handle, Vec<u8>)>`; phase 2
holds no world lock and writes. Six lines, and the rule is readable off them.

> **CORRECTION, 2026-10-22 — it happened, but not to this ticker.** The
> original ticker stayed **one-phase** and takes no lock but a wire's, and its
> `try_lock`-and-skip safety argument now depends on staying that way. The
> two-phase shape landed in a **second** thread,
> `push::spawn_world_ticker`, which drains the world's scheduled queue: it
> takes the world lock, resolves each audience to client ids while holding it,
> drops the guard, and only then encodes and posts. A reader following this
> paragraph would edit the wrong loop and weaken the one guarantee the first
> ticker rests on. `docs/reference/threads-and-locks.md` has both.


---

## 2. The packet

### 2.1 The broadcast address — **verified, and the roadmap is right**

`carbon/common/script/net/machoNetAddress.py`, `MachoAddress.__getstate__` **[V]**:

```python
if self.addressType == const.ADDRESS_TYPE_BROADCAST:
    return (const.ADDRESS_TYPE_BROADCAST, self.broadcastID, self.narrowcast, self.idtype)
```

and `__setstate__`:

```python
elif state[0] == const.ADDRESS_TYPE_BROADCAST:
    self.addressType, self.broadcastID, self.narrowcast, self.idtype = state
```

So `(4, broadcastID, narrowcast, idtype)`, `narrowcast` a list. That is exactly
`eve-proto/src/address.rs:33-38` and exactly `docs/archive/wire-format.md:273`. **Nothing to change.**

Two facts not in the roadmap, both load-bearing:

- **`idtype` arrives on the wire with a `*` already prepended.** It is
  `ScattercastWithoutTheStars` that adds it, not the transport **[V]**:
  ```python
  def ScattercastWithoutTheStars(self, idtype, ids, method, args):
      if not idtype.startswith('+'):
          idtype = '*%s' % idtype
      if ids:
          self.ForwardNotifyDown(machoNetPacket.Notification(
              destination=machoNetAddress.MachoAddress(
                  broadcastID=method, idtype=idtype, narrowcast=ids),
              payload=(1, args)))
  ```
  So the literal to send is `'*clientID'`, not `'clientID'`. (`'+nodeID'` is the proxy
  fan-out form and is not ours.)
- **A reconstructed broadcast address has no `service` attribute at all.** `__setstate__`
  sets four names and `service` is not among them, and `MachoAddress` has no class-level
  default. So `getattr(packet.destination, 'service', None)` is `None` **[V]**. This is read
  in two places and matters in both — see §2.3.

`broadcastID` and `idtype` go on the wire as **byte strings**, because that is what
`__getstate__` produces on the real server and `str.startswith` is what reads them back.
`address.rs:219-221` already emits `PyValue::Bytes`. **Nothing to change.**

### 2.2 The payload — **verified, and the roadmap is right**

Command **12** `macho.Notification`, `params = ['payload?']` **[V]**
(`machoNetPacket.py`; `cluster/__init__.py:86` gives the 12). So `body` is a 1-tuple.
`docs/archive/wire-format.md:259` already records both. **Nothing to change.**

The nesting was verified by walking the client's own chain rather than by reading the
roadmap. The chain order is **confirmed from the builder**, which matters because the list in
`get_gpcs_map()` does not say which end is the bottom:

```python
# machoNet.py:826
self.channelHandlersUp[each] = nodes[0]
```

and `nodes` is `['client'][None] = [ExceptionWrapper, ExceptionMappingGPCS, ObjectCall,
ServiceCall, BroadcastStuff]`, with `ForwardNotifyUp` wired by walking `.next` to the first
handler that *has* a `NotifyUp` (`machoNet.py:820-821`). `ExceptionMappingGPCS` has no
`NotifyUp`, so the live chain is:

```
ExceptionWrapper.NotifyUp  ->  ObjectCall.NotifyUp  ->  ServiceCall.NotifyUp  ->  BroadcastStuff.NotifyUp
```

Climbing it with `payload = (0, pickle)` **[V]**:

| layer | code | result |
|---|---|---|
| `ExceptionWrapper.NotifyUp` | `try: self.ForwardNotifyUp(packet)` / `except Exception: log.LogException('Exception during remote notification')` | passes through — **and swallows every exception above it** |
| `ObjectCall.NotifyUp` | `if packet.payload[0] == 1:` … `else: packet.payload = self.FromPickle(packet.payload[1], packet.source)` | `payload = (0, (1, args))` |
| `ServiceCall.NotifyUp` | `if packet.payload[0]:` … `else: packet.payload = packet.payload[1]` | `payload = (1, args)` |
| `BroadcastStuff.NotifyUp` | `if packet.payload[0]:` → dispatch `packet.destination.broadcastID` with `packet.payload[1]` | `args` reach the handler |

So **`payload = (0, pickle((0, (1, args))))` — confirmed [V]**, and the outer `0` is not
optional: `ObjectCall.NotifyUp` branches on `payload[0] == 1` and would try to read the
payload as an *object* notification (`objectID, method, args = tup[0], tup[1], tup[2]`,
then `session.machoObjectsByID[objectID]`) and raise `UnMachoDestination`.

**The pickle is a byte string, not a `SubStream`.** `ObjectCall.FromPickle` is
`blue.marshal.Load(thePickle, self.ParseObjectID)` **[V]** — same reason
`call.rs:121`'s `response_payload` is `PyValue::Bytes(to_pickle(v))` and not a substream: a
substream tag materialises into an object before `Load` ever sees it. `response_payload`
already builds exactly the right thing; I'd suggest splitting the one line out so the name
fits both callers:

```rust
/// A value marshalled into the byte string the client will hand to
/// `blue.marshal.Load` -- a response payload, or the inner pickle of a
/// `Notification`. Both land in `ObjectCall.FromPickle`.
pub fn pickled(v: &PyValue) -> PyValue {
    PyValue::Bytes(to_pickle(v))
}
```

with `response_payload` keeping its (excellent) doc comment and becoming `pickled(v)`.

**The constructor**, paste-ready, for `eve-proto/src/packet.rs`:

```rust
    /// A server-initiated `Notification` (command 12) with a **broadcast**
    /// destination -- the only way to reach a client-side notify event.
    ///
    /// Mirrors `CoreBroadcastStuff.ScattercastWithoutTheStars`, which is the
    /// method the real server calls:
    ///
    /// ```python
    /// def ScattercastWithoutTheStars(self, idtype, ids, method, args):
    ///     if not idtype.startswith('+'):
    ///         idtype = '*%s' % idtype
    ///     if ids:
    ///         self.ForwardNotifyDown(machoNetPacket.Notification(
    ///             destination=machoNetAddress.MachoAddress(
    ///                 broadcastID=method, idtype=idtype, narrowcast=ids),
    ///             payload=(1, args)))
    /// ```
    ///
    /// so `id_type` is passed in **with** its `*` -- `"*clientID"` -- and
    /// `narrowcast` is a list.
    ///
    /// The payload is the GPCS chain written out. On the client a packet enters
    /// at `channelHandlersUp[None]`, which `machoNet.py:826` sets to `nodes[0]`
    /// = `ExceptionWrapper`, and climbs:
    ///
    /// ```text
    /// ObjectCall.NotifyUp      payload[0] != 1 -> payload = FromPickle(payload[1])
    /// ServiceCall.NotifyUp     payload[0] == 0 -> payload = payload[1]
    /// BroadcastStuff.NotifyUp  payload[0]      -> dispatch(broadcastID, payload[1])
    /// ```
    ///
    /// The pickle is a byte string, not a `SubStream`: the client hands it to
    /// `blue.marshal.Load` via `ObjectCall.FromPickle`, and a substream tag
    /// materialises into an object before it gets there.
    ///
    /// **`oob` stays empty, and that is a hang if it does not.**
    /// `machoNetTransport.LockSession` calls
    /// `WaitForSequenceNumber(packet.source, packet.oob.get('sn', 0))` for every
    /// even-numbered command, and that function returns early only `if not seq`.
    /// A non-zero `sn` parks the notification's tasklet in the client's
    /// `Sequencer` waiting for numbers we will never send.
    ///
    /// **`source` must not itself be a broadcast or ANY address, and that is a
    /// disconnect if it is.** `machoNet.HandleMessage` closes the transport
    /// outright for any even command whose source is in `__aorb__`:
    /// *"A request must be sent from a specific address, not a forking one"*.
    /// A node address is what the real server sends and what this takes.
    pub fn notification(
        source: MachoAddress,
        broadcast_id: &str,
        id_type: &str,
        narrowcast: Vec<PyValue>,
        args: Vec<PyValue>,
    ) -> Self {
        let inner = PyValue::Tuple(vec![
            PyValue::Int(0),
            PyValue::Tuple(vec![PyValue::Int(1), PyValue::Tuple(args)]),
        ]);
        MachoPacket::new(
            MachoNetMsg::Notification,
            source,
            MachoAddress::Broadcast {
                broadcast_id: broadcast_id.to_owned(),
                narrowcast,
                id_type: id_type.to_owned(),
            },
            vec![PyValue::Tuple(vec![
                PyValue::Int(0),
                crate::call::pickled(&inner),
            ])],
        )
    }
```

and the `dispatch.rs` side:

```rust
    /// A client-bound `Notification`, addressed the way the real server
    /// addresses a per-client one: `Scattercast('clientID', [clientID], event,
    /// *args)`. The `*` on the idtype is added by `ScattercastWithoutTheStars`
    /// itself, which is why it is literal here.
    ///
    /// `userID` is left `None`, as on our session notifications: the client's
    /// only reader of it is the `installSession` branch of
    /// `_AssociateWithSession`, which an established session never takes.
    fn notify(&self, event: &str, args: Vec<PyValue>) -> Result<Vec<u8>, DispatchError> {
        MachoPacket::notification(
            MachoAddress::node(self.handshake.node_id()),
            event,
            "*clientID",
            vec![PyValue::Int(self.session.user_id)],
            args,
        )
        .to_payload()
        .map_err(DispatchError::Proto)
    }
```

`MachoPacket::new` already sets `oob: None` and `packet.rs:118-121` already carries the
warning about a stray `sessionVersion`. The whole envelope is additive: `MachoNetMsg::
Notification` and the class name are at `packet.rs:283`, and `MachoAddress::Broadcast` round-
trips in a test at `address.rs:328-332`.

### 2.3 How `NotifyUp` routes a `broadcastID`, and what it does with the args

`CoreBroadcastStuff.NotifyUp`, `carbon/common/script/net/BroadcastStuffGPCS.py` **[V]**:

```python
def NotifyUp(self, packet):
    if packet.payload[0]:
        self.machoNet.LogInfo('BroadcastStuff::Notify(', packet.destination.broadcastID, ',...)')
        if packet.destination.broadcastID.startswith('Process'):
            sm.ChainEventWithoutTheStars(packet.destination.broadcastID, packet.payload[1])
        elif packet.destination.broadcastID.startswith('Do'):
            sm.SendEventWithoutTheStars(packet.destination.broadcastID, packet.payload[1])
        elif packet.destination.broadcastID == '__MultiEvent':
            ...
        elif packet.destination.broadcastID == '__PacketBus':
            ...
        else:
            sm.ScatterEventWithoutTheStars(packet.destination.broadcastID, packet.payload[1])
        self.UpdateRecvNotificationStats(packet.destination.broadcastID, False, packet.source)
    else:
        packet.payload = packet.payload[1]
        self.ForwardNotifyUp(packet)
```

`payload[1]` is `args` — **the whole tuple, passed positionally to the handler**, because
`SendEventWithoutTheStars(eventid, args)` ends in `getattr(srv, eventid)(*args)` via
`ClockThisWithoutTheStars(logname, func, args)` **[V]**. So `args` must be a tuple whose
arity matches the handler's signature exactly.

**Five prefixes, three dispatch disciplines, and the difference decides which events are
safe:**

| prefix | dispatcher | handlers run | can a handler block? |
|---|---|---|---|
| `Do…` | `SendEventWithoutTheStars` | **synchronously, inside `NotifyUp`**, with `stackless.getcurrent().block_trap = 1` | **No.** A remote call from a `Do*` handler raises. |
| `On…` (default branch) | `ScatterEventWithoutTheStars` | each in its own `uthread.worker`, via `MollycoddledUthread` | Yes. |
| `Process…` | `ChainEventWithoutTheStars` | synchronously, chained | — |

That table is **[V]** from `serviceManager.py:577-636` and `:699-745`, and it is the single
most useful thing I found that is not already written down anywhere: it says `Do*` pushes
(Destiny) run under the session read lock with blocking forbidden, and `On*` pushes (every
mutator notification) do not. **Prefer `On*` until Destiny forces `Do*`**, and when step E
arrives, know that a `DoDestinyUpdate` handler cannot call back to us.

Also **[V]**: `ScatterEventWithoutTheStars` logs two `LogError`s plus a traceback if the
eventid does not start with `On` — cosmetic, but a reason to name any diagnostic event `On…`.

### 2.4 Is the destination validated? — **Confirmed. It is not.**

The note on record is correct, and here is the whole of it, in four steps **[V]**:

1. `machoNet.HandleMessage` → `if not skipSanityCheck: if not self.IsMessageSane(...)`.
   `IsMessageSane` has exactly two branches: `if srcTransport.transportName ==
   'tcp:packet:client'` (the *server's* name for a client wire) and
   `elif macho.mode == 'proxy' or macho.mode == 'server'`. On the client neither applies and
   it returns `True` unconditionally. Empirically corroborated: our existing
   `SessionInitialStateNotification` has a **CLIENT** destination, which that first branch
   would reject as *"Client is trying to send C2C"* (`__borc__ = (BROADCAST, CLIENT)`), and
   it works today.
2. `routesTo = theMessage.RoutesTo(...)`, then `if macho.mode == 'client' or routesTo:` —
   short-circuits on the client. Routing is never consulted.
3. Command 12 is in `__notificationtypes__`, so the non-proxy branch is a plain
   `srcTransport.SessionNotification(theMessage)`.
4. `SessionNotification` → `_SessionAndChannelAndIDFromPacket` → `_AssociateWithSession`:
   ```python
   elif macho.mode == 'client':
       nodeID = 0
       serviceSession = False
   ```
   **unconditional, and the packet is not read at all.** `sess` is then the global `session`.

**So `clientID`, `idtype` and `narrowcast` are cosmetic for us.** Send the real server's
shape anyway — it costs one tuple, and the day a second player exists the narrowcast is the
field a bug would hide in.

What *is* validated, and these are the three things that bite:

- **The source address type.** `HandleMessage`: for an even command,
  `elif theMessage.source.addressType in self.__aorb__: srcTransport.Close('A request must
  be sent from a specific address, not a forking one')`, with `__aorb__ = (ANY, BROADCAST)`
  **[V]**. A `Notification` sourced from a broadcast address is an instant "Connection Lost".
  `MachoAddress::node(...)` is correct.
  **The rule is "not forking", not "must be Node"** — `NODE` (1) and `CLIENT` (2)
  both pass the check, and the odd-command branch tests the *destination* the
  same way. We keep `node` for three other reasons: it is what the real proxy
  sends, it is the only address kind the sequencer consults for `oob['sn']`, and
  it is the prefix the object-id parser expects. Worth the distinction because
  `a_notification_source_is_not_a_forking_address` asserts `Node` — which is an
  assertion about *our choice*, not about the client's requirement, and a future
  reader should not infer a constraint from it. Found by the
  `session-and-binding` lane.
- **`oob['sn']`, and the rule extends to a server-initiated *call* too.** A
  `CallReq` is even-numbered, so it reaches `WaitForSequenceNumber` through
  `LockSession` exactly as a notification does -- so "absent `oob`" is not a
  notification-specific rule, it is a rule about every even command we send.
  Worth saying here because the next thing built on this path is a call, and
  the send half of a call **is** `push.rs`: the same `Wire` mutex, the same
  frame, with a `callID` in the source node address and a pending table
  keyed by it. See `re/client-callable.md` §4.
- **`oob['sn']`.** `LockSession` → `WaitForSequenceNumber(packet.source, packet.oob.get('sn',
  0))`, which is `if not seq or macho.mode != 'client': return` **[V]**. Absent `oob` is
  safe; a non-zero `sn` is a hang in the `Sequencer`.
- **The channel.** `macho.packetTypeChannelMap` maps **only** commands 16 and 18 to
  `'sessionchange'` (`machobase.py:19-20`) **[V]**, so command 12 falls to
  `channelHandlersUp[None]` — the main chain, as the roadmap says. And since the broadcast
  destination has no `service` attribute, `SessionNotification` skips its
  `msgSession.ConnectToService(packet.destination.service, remote=1)` block entirely **[V]**
  — worth knowing, because that call would otherwise fire a *bind* on the client's side as a
  side effect of our push.

---

## 3. The first push to implement, and its test

### 3.1 The choice: `OnAccountChange`, driven by `/giveisk`

Weighed against the alternatives:

| candidate | why not first |
|---|---|
| `OnItemChange` | needs an item store (`docs/reference/world-state.md` §2 item 3) to say anything true, and the client caches items aggressively — a wrong `OnItemChange` hands a live client a stale object. Blocked on persistence, not on push. |
| `OnSessionReset` | `walletSvc.ProcessSessionReset` *clears* `_wealth`, so the effect is "the next pull is fresh" — i.e. it proves the push by making the client do a pull. The weakest possible evidence. |
| a chat / notification message | needs `LSC`/`notificationSvc` surface we do not answer, so a failure is ambiguous between the push and the shape. |
| `DoDestinyUpdate` | step E. Also `Do*`, i.e. the one discipline where handlers run synchronously with `block_trap = 1` under the session read lock (§2.3) — the hardest case, not the cheapest. |
| **`OnAccountChange`** | **three scalar args, no DBRow, no item store, no Destiny, no new service surface, and the server-side state already exists and already moves.** |

And it closes a gap the code itself complains about. `service.rs`'s `/giveisk` arm carries
this comment today:

> **The wallet does not refresh by itself.** A real server sends `OnAccountChange`, and we
> have no server-initiated push path at all — `conn::serve` only ever writes in response to a
> read. So the new balance is live on the wire the instant this returns, and invisible until
> something asks again.

`/giveisk` already mutates live server state (`c.balance += amount`), there is a test pinning
that `GetCashBalance` follows it
(`service.rs:2861 giveisk_changes_what_the_wallet_asks_for`), and the console line it returns
is literally an apology for the missing push. **The first push deletes that apology.**

### 3.2 What the player sees, and the two independent paths that produce it

`eve/client/script/ui/shared/neocom/wallet/walletSvc.py` **[V]** — `WalletSvc.
__notifyevents__` contains `'OnAccountChange'`:

```python
def OnAccountChange(self, accountKey, ownerID, balance):
    LogInfo('Wallet::OnAccountChange', accountKey, ownerID, balance)
    if balance is not None:
        if accountKey == 'cash' and ownerID == session.charid:
            self._OnCharAccountChange(balance)
        ...
    sm.ScatterEvent('OnAccountChange_Local', accountKey, ownerID, balance)

def _OnCharAccountChange(self, newBalance):
    if GetSettingValue('notifyAccountChange'):
        sm.GetService('neocom').Blink('wallet', GetByLabel('UI/Neocom/Blink/ISKBalanceChanged'))
        self.BlinkPersonalWallet(walletConst.PANEL_TRANSACTIONS)
    difference = newBalance - self.wealth
    sm.ScatterEvent('OnPersonalAccountChangedClient', newBalance, difference)
    self._wealth = newBalance
    walletSignals.on_personal_isk_balance_changed(newBalance, difference)
```

**Three things happen on screen, and the setting gates only the first:**

1. the neocom wallet button **blinks** with "ISK balance changed" (gated on
   `notifyAccountChange`, default **[I]**);
2. `OnPersonalAccountChangedClient` reaches `NeocomContainer.OnPersonalAccountChangedClient`
   → `walletPopupManager` → **a floating ISK-delta popup by the neocom** **[V]**, ungated;
3. `_wealth` is **replaced**, so the wallet window and the market's `WalletBalance` (which
   listens to `OnAccountChange_Local`) read the new number immediately. `WalletBalance` even
   has `animate_balance_to`, which colours the morph green for an increase.

That answers the exact gap the roadmap names: `walletSvc.Run` latches
`self._wealth = sm.RemoteSvc('account').GetCashBalance(0)` at startup **[V]**, and
`ProcessSessionReset` is the only other thing that clears it — so the push is the *only* way
the number moves without a reconnect.

**Three details that are silent failures if wrong, all [V]:**

- **`accountKey` is the string `'cash'`, not the integer `appConst.accountingKeyCash`.** The
  guard is `accountKey == 'cash'`. An integer there passes every type check, reaches
  `ScatterEvent('OnAccountChange_Local', ...)`, and updates nothing visible — the worst kind
  of near-miss. Send `PyValue::Bytes(b"cash")`.
- **`ownerID` must be `session.charid`**, i.e. `90_000_001` (`service.rs:353`), **not**
  `session.userid` (1) and not the corp id. A mismatch takes the `elif` and then falls out.
- **`balance` must not be `None`**, and it is a `Real` — `GetCashBalance` already answers
  `PyValue::Real` (`service.rs:2873`), so use the same type.

`OnAccountChange` starts with `On`, so it lands on `ScatterEventWithoutTheStars`: each
handler runs in its own `uthread.worker` and **may block** (§2.3). That matters here —
`_OnCharAccountChange` reads `self.wealth`, which would make a remote `GetCashBalance` call
if `_wealth` were `None`. Under `Do*` that would raise; under `On*` it is just a `CallReq`
that `conn::serve`'s read loop answers on its next iteration, from the other thread, while
the push thread is already done. **The push path's first user exercises the "client calls us
back because of something we pushed" case for free**, which is the property Destiny will
depend on and which is otherwise hard to arrange a test for.

### 3.3 How we verify it from a capture

The verification is specifically designed so that **the absence of a client frame is the
evidence**, which is the only thing that distinguishes a push from a reply.

```sh
tools/dev.sh cargo run -q -p eve-server --bin evecap -- captures/<file>.evecap | tail -40
```

For increment 3 (`/pushtest 3`), the capture must show:

```text
[NNN]  <t>ms       C->S   ... slash.SlashCmd("/pushtest 3")
[NNN+1] <t+1>ms    S->C   ... CallRsp
                             <-- NOTHING here, in either direction, for ~3000 ms
[NNN+2] <t+3001>ms S->C   ... Notification(Address::Node(...), Address::BroadCast(
                                 broadcastID="OnAccountChange", narrowcast=[1],
                                 idtype="*clientID"), ...)
```

Four assertions, each independently falsifiable:

1. **A `S->C` frame with no `C->S` frame before it.** This is the whole claim. `evecap`'s
   `micros` column makes the ~3 s gap visible directly, and the gap exists only because the
   capture lives inside the `Wire` and therefore timestamps the *push*, not a reply.
2. **`evecap` decodes it as a `Notification` with a `BROADCAST` destination.** It does this
   today with no new code: `MachoPacket::from_value` already handles command 12
   (`packet.rs:283`) and `print_record` renders any decodable payload. A pushed frame lands
   in `--calls`'s "other" bucket, which is fine — the default per-frame dump is the view that
   matters here.
3. **The client's own log shows `Wallet::OnAccountChange cash 90000001 <balance>`** — a
   `LogInfo` on the first line of the handler, so it proves the args arrived with the right
   arity *and* the right types.
4. **No "Connection Lost", no `Exception during remote notification` in the client log, and
   the session continues.** The second of those is the specific string
   `ExceptionWrapper.NotifyUp` logs, so a malformed payload names itself.

And the human verification, which is the one the owner will actually do: type
`/giveisk 5000000`, read the console line, and watch the neocom wallet flash and the ISK
number change with nothing else touched.

---

## 4. The increments

Every one leaves `tools/gate.sh` green. The gate is `fmt --check`, `clippy -D warnings`,
`cargo test --locked --workspace`, `check_deps.py` (empty allowlist — **this design adds no
dependency**), and a clean working tree.

### Increment 1 — the plumbing, deliberately inert

- New `eve-server/src/push.rs`: `Wire`, `Writers`, `spawn_ticker`, `TICK` (§1).
- `conn::serve` signature: `(stream, &Arc<Mutex<Wire>>, &mut dyn Dispatch) -> ConnEnd`; the
  `Option<&mut Capture>` parameter goes, the capture moves into `Wire`, `conn::send` becomes
  `Wire::send_all`.
- `server.rs`: `Server` grows `writers: Arc<Writers>`, built and ticked in `bind`;
  `serve_one` does the `try_clone`/register/`close` dance; **the "no shared state to corrupt"
  comment at `server.rs:94` is corrected in this commit.**
- `Reply.pushes` added; every existing construction keeps working because `Reply` derives
  `Default` and the field is a `Vec`.
- `eve-proto`: `call::pickled`, `MachoPacket::notification`.
- `eve-server --push-probe N`: a dev-only flag, **off by default**, that posts `N` frames at
  1 s intervals carrying `broadcastID = "OnEveOfflinePushProbe"` with no args, and prints a
  loud startup line saying it is on. Per §2.3 this is **provably inert on the client**:
  `ScatterEventWithoutTheStars` indexes a `defaultdict(list)`, finds nothing, and returns.
  It is the maximum-information, zero-risk first contact.

*Visible on screen:* **nothing, by design.** Only a capture can tell you this worked.

*Tests (the one that matters is the first):*

- `a_push_cannot_interleave_a_reply` — a `Wire` over a loopback pair; one thread loops
  `send_all` on a 100 KB payload, another pushes a small one; read the stream back through
  `try_split_frame` and assert every frame is byte-for-byte one of the two. **This is the
  test that fails if the lock is byte-granular or if the framed buffer is built under I/O**,
  and it is the only one of these a falsification pass can meaningfully target.
- `the_capture_order_is_the_socket_order` — same setup, assert `capture::read_all`'s sequence
  equals the stream's.
- `the_capture_records_pushed_frames` — one push, assert one `ServerToClient` record.
- `a_closed_wire_refuses_a_push` — `close()` then `send_all` → `DispatchClosed`, and the peer
  received nothing.
- `a_dead_connection_leaves_the_registry` — register, drop the `Arc`, `live()` is empty.
- `a_notification_climbs_the_gpcs_chain` — build one, `loads` it back, walk
  ObjectCall/ServiceCall/BroadcastStuff by hand in the test, assert you recover
  `(broadcastID, args)`. The test *is* the §2.2 table.
- `a_notification_carries_no_sequence_number` — assert `oob` is `None` in the state tuple.
- `a_notification_source_is_not_a_forking_address` — assert the source is `Node`, naming
  `__aorb__` in the message.

### Increment 2 — the first visible push, with no thread in the path

`Registry` grows `pending: Vec<(Duration, Notify)>` and a `take_pending`:

```rust
/// Something that happened in the world and that the client has to be *told*,
/// because the call that caused it does not return it.
///
/// Five subsystems' worth of mutators discard their return value entirely
/// (`docs/archive/roadmap.md`), so this enum is where almost every future one lands:
/// `OnItemChange`, `OnGodmaItemChange`, `OnOwnOrdersChanged`, `OnIndustryJob`,
/// `OnAgentMissionChange`. It names the event in **world** terms; turning it
/// into a `Notification` packet is `dispatch.rs`'s job, because `Registry`
/// knows nothing about machoNet and should keep not knowing.
#[derive(Debug, Clone, PartialEq)]
pub enum Notify {
    /// `OnAccountChange(accountKey, ownerID, balance)`.
    AccountChange { owner_id: i64, balance: f64 },
}
```

`/giveisk` pushes `(Duration::ZERO, Notify::AccountChange { .. })` and loses its apology.
`dispatch.rs` drains it after every `registry.dispatch`:

```rust
    /// Drain the world's pending notifications into frames.
    fn pending_pushes(&mut self) -> Result<Vec<(Duration, Vec<u8>)>, DispatchError> {
        let mut out = Vec::new();
        for (after, n) in self.registry.take_pending() {
            let (event, args) = match n {
                Notify::AccountChange { owner_id, balance } => (
                    "OnAccountChange",
                    vec![
                        // The account key is the **name**, not the integer
                        // `appConst.accountingKeyCash`: `walletSvc.OnAccountChange`
                        // tests `accountKey == 'cash'`, and an integer there passes
                        // every guard and updates nothing.
                        PyValue::Bytes(b"cash".to_vec()),
                        // `session.charid`, not `session.userid`. The other
                        // branch is the corp wallet.
                        PyValue::Int(owner_id),
                        PyValue::Real(balance),
                    ],
                ),
            };
            out.push((after, self.notify(event, args)?));
        }
        Ok(out)
    }
```

**This is the first increment where something is visible on screen** — the neocom blink and
the ISK popup. It is also, honestly, **not yet the push path**: with `Duration::ZERO` the
frame goes out in the reply's own flush, on the connection's own thread. That is the point.
It isolates *is the packet right* from *is the thread right*, so a failure in a real client
run is attributable to one of the two — the same reason `docs/reference/server.md` §5 records frames rather
than interpretations.

### Increment 3 — the push path proper

Add `/pushtest [seconds]` (default 3) to `SLASH_COMMANDS` and the `slash` match: it queues
`(Duration::from_secs(n), Notify::AccountChange { balance: <current> })`, so the *only*
difference from increment 2 is which thread writes the frame and when. The player types it,
the console answers immediately, and `n` seconds later — with the client having sent nothing
at all in between, which is the hard part and §5.3's whole argument — the wallet reacts.

**This is step D, done**, and `/pushtest` stays forever as the one-line answer to "is the
push path alive", in the spirit of `tools/check-resources.sh`.

*Signal:* exactly the capture in §3.3, plus a full login/undock/quit with the ticker running
and `evecap --calls` showing the same answered/asked count as the previous run, no framing
errors, and no new entries in `--alerts`.

### Increment 4 — fan-out and the lock discipline

Lands with or just after spine step A. The ticker becomes two-phase (build under the world
read lock, write holding nothing), and `Writers` fans one world event to every connection.
**As shipped this is a second thread rather than a change to the first — see the correction
in §1.5 — and `Writers` is keyed by `client_id`, not a bare `Vec<Weak<…>>`:
`wire_for(client_id)` is what makes a notification deliverable to somebody other than the
caller, and it does not appear anywhere in this document.**
Nothing new is visible; this is the thing `OnItemChange`, `OnGodmaItemChange` and every
Destiny update then sit on, and it is the increment that makes `docs/reference/world-state.md` §4 rule 4
true by construction rather than by comment.

---

## 5. The risks, named

1. **Deadlock between the push mutex and the future world lock.** Stated once as
   **world > writers > wire**, and enforced two ways rather than by discipline:
   `Reply::pushes` means a dispatch arm *cannot* reach a socket (§1.5), and the ticker's
   two-phase shape means it never holds the world lock while holding a `Wire`. The
   consequence of getting it wrong is the symptom `docs/reference/world-state.md` §4 rule 2 describes — a
   black screen with nothing in either log — so it is worth making unrepresentable.
2. **A broadcastID the client has no handler for.** **Verified harmless** and worth writing
   down because it is the thing that makes `--push-probe` safe. `sm.notify` and
   `sm.notifyObs` are `defaultdict(list)` (`serviceManager.py:45-46`), so an unknown event
   yields an empty iteration, not a `KeyError`. `SendEvent` (`Do*`) additionally logs
   `"Orphan'd event. <id> doesn't have any listeners"` at `LogInfo` and returns `()`;
   `ScatterEvent` (`On*`) does not even log that. And above all of it,
   `ExceptionWrapper.NotifyUp` is `try: self.ForwardNotifyUp(packet) / except Exception:
   log.LogException('Exception during remote notification')` **[V]** — so the worst case for
   *any* malformed push is one traceback in `client-*.log`, not a dead receive tasklet. That
   is the same "a wrong answer is evidence, silence is not" property
   `docs/guides/debugging-the-client.md` §1 relies on, now extended to the push direction.
3. **A stuck `write_all` starving every other connection.** One global ticker means one
   thread for N sockets, so a client whose receive window is full would block all of them.
   Handled by `try_lock` + skip, which is sound because a skipped 1 Hz Destiny tick is
   exactly what the client's local extrapolation exists for, and anything that must be
   delivered goes out on the connection's own thread. **Do not "fix" this with
   `set_write_timeout`:** a timed-out `write_all` can fail *after a partial write*, leaving
   the stream mid-frame, which is precisely the corruption the frame-granular lock exists to
   prevent. Taking the timeout would trade a stall for a corrupt stream.
4. **A write to a closed-and-reused fd.** The `closed` flag, the `Weak` registry, and
   `close()` on every exit path (§1.3). This is the real stranded-resource analogue, and it
   is *not* the SIGKILL case — SIGKILL is the easy one, because a thread cannot outlive its
   process.
5. **Poisoning.** Two mutexes, two policies, each with its reason attached, as
   `docs/reference/world-state.md` §4 rule 5 asks: `Writers` recovers (no invariant; the alternative is one
   panicking connection killing everyone's push path), `Wire` does not (the stream may be
   mid-frame), and `serve_one`'s `close()` recovers because `close` is the one operation
   that is correct on a broken `Wire`.
6. **`oob['sn']` set non-zero** → the notification's tasklet parks in the client's
   `Sequencer` forever (§2.4). Guarded by a test asserting `oob` is `None`.
7. **A forking source address** → `srcTransport.Close('A request must be sent from a specific
   address, not a forking one')`, i.e. "Connection Lost" with no explanation (§2.4). Guarded
   by a test.
8. **A push landing mid-session-change.** Command 12 takes the session **read** lock, so a
   notification arriving while `SelectCharacterID`'s or `Undock`'s change holds the write
   lock sleeps until it completes — not a corruption, but a stall in a place where
   `dispatch.rs` already warns about exactly this class of mistake. Mitigated by the
   one-guard-per-`Reply` rule, and the rule to be added: never post a *delayed* push from a
   reply that also carries a session change.
9. **SIGPIPE.** Rust's runtime sets `SIGPIPE` to `SIG_IGN`, so a write to a vanished client
   returns `EPIPE` rather than killing the process. The ticker logs and moves on; the
   connection's own thread meets the same truth on its next read and ends there. Worth
   stating because the opposite behaviour would mean one departing client takes the server
   with it.
10. **The capture's `flush()` now runs under the socket mutex.** `Capture::write` flushes per
    record (`capture.rs:102`), deliberately, so the mutex now covers a filesystem write and a
    full or slow disk stalls both the reply path and the ticker. `try_lock` keeps it from
    being a server-wide stall; nothing keeps it from being a per-connection one, and that is
    the right trade for an instrument we have twice been glad to have.
11. **`--push-probe` left on by accident.** Off by default, loud startup line, and it emits
    `OnEveOfflinePushProbe` — a name no real client code will ever match, so finding it in a
    capture tells you immediately what you are looking at.
12. **Thread count and fds.** `1 + N` threads and two fds per connection, where the second
    closes on `Wire::close`. Named so that nobody later discovers `try_clone` as a surprise.

---

## 6. Two corrections the roadmap should take from this

1. **"One ticker thread per connection" → one global ticker.** The clock is world state; only
   the write handle is per-connection. The spine's own step A (one shared world) is the
   argument, and the stranded-process lesson in `tools/up.sh` is the second one. §1.4.
2. **"Prove the plumbing does not corrupt a reply" by sending *something*, not nothing**
   (`docs/archive/destiny-and-the-ballpark.md` §8 step 3 says "send nothing"). A handler-less `On*`
   broadcastID is provably inert on the client and exercises the entire path — frame, mutex,
   capture, client decode, four GPCS layers, dispatch. A ticker that sends nothing tests the
   sleep.

Both are small, and both are the kind of thing that is cheap now and expensive after the
first Destiny blob exists.
