//! The server-initiated write path: pushing a frame to an idle client.
//!
//! `conn::serve` writes only in response to a read, and that cannot stay true.
//! Nothing on the client fetches Destiny state and the client falls silent for
//! minutes at a time (`docs/archive/destiny-and-the-ballpark.md` §5); and every
//! inventory, fitting, market and industry *mutator* discards its return value,
//! so the client learns what happened from `OnItemChange`, `OnGodmaItemChange`,
//! `OnOwnOrdersChanged` and friends -- or not at all. This is the gate on a
//! visible ship *and* on every act of doing something the player can watch work
//! (`docs/archive/roadmap.md`, the spine, step D; the design is `docs/reference/push-path.md`).
//!
//! Three types, and keeping them apart is most of the design:
//!
//! - [`Wire`] owns **one connection's write half and that connection's
//!   capture**, behind one mutex.
//! - [`Writers`] is the process-wide registry of `Weak` handles the ticker fans
//!   out over. It is **not** a session table: it answers *where do I write*, not
//!   *who is online*. The session table is spine step C and a different thing
//!   with different semantics; conflating them now would make step C a rewrite.
//! - [`spawn_writers`] is the one clock -- one for the process, not one per
//!   connection, because the simulation clock is world state (§1.4).
//!
//! **Lock order, process-wide: world > writers > wire.** Acquired in that
//! order, never the reverse, and nothing holds more than two.
//!
//! **An earlier version of this paragraph named the wrong function**, and it
//! is corrected here rather than quietly rewritten because it is exactly the
//! sentence a future reader would have used to convince themselves the
//! remaining risk was already handled. It said "the only code that ever holds
//! two is the ticker, and its shape is six lines you can read in one go".
//! Neither half is true. **Neither** ticker ever holds two: [`Writers::live`]
//! and [`Writers::wire_for`] release the writers mutex *before* handing back
//! an `Arc`, so the `writers -> wire` edge carries no live traffic. The code
//! that does hold two is `dispatch::deliver_elsewhere` and
//! `dispatch::post_session_change`, which take a wire mutex under the world
//! write lock -- and `conn::serve` holds a wire mutex across `write_all`, so
//! one peer with a closed TCP receive window can freeze the world for
//! everybody for up to `conn::WRITE_TIMEOUT`. That is
//! `docs/port-tracker.md` §26's known inversion, and it now has **two** call
//! sites rather than the one §26.3 audited.
//!
//! `docs/reference/world-state.md` §4 rule 4 -- never hold a lock across a socket write
//! -- is enforced structurally for a *dispatch arm*, which can only ask for a
//! push via [`crate::dispatch::Reply`]. It is not enforced for the two sites
//! above. `docs/reference/threads-and-locks.md` has the whole inventory.

use crate::capture::{Capture, Direction};
use crate::conn::ConnEnd;
use eve_marshal::PyValue;
use eve_proto::write_frame;
use std::borrow::Cow;

/// Payloads at or above this many bytes are sent zlib-compressed, the way
/// the real server does it (the client inflates any stream whose first byte
/// is not `0x7D`/`0x7E`, `eve_marshal::constants::compression` **[V]**). The
/// client's own floor is 200 bytes (`MIN_BYTES_TO_COMPRESS`); small replies
/// stay plain because the CPU costs more than the bytes save **[A]**.
pub const COMPRESS_MIN_BYTES: usize = 8 * 1024;

/// `payload` as it goes on the wire: deflated when it is large and the
/// deflate is actually smaller, untouched otherwise.
pub fn compress_payload(payload: &[u8]) -> Cow<'_, [u8]> {
    if payload.len() < COMPRESS_MIN_BYTES {
        return Cow::Borrowed(payload);
    }
    let z = eve_inflate::deflate_zlib(payload);
    if z.len() < payload.len() {
        Cow::Owned(z)
    } else {
        Cow::Borrowed(payload)
    }
}

/// One payload on its way to a socket (PA-9).
///
/// `Raw` is not yet decided: the writer runs [`compress_payload`], as it always
/// did (`Writers::post`, `Wire::post` and connection replies). `Ready` was
/// decided in the encode phase, on the tick's worker threads, so the writer
/// only frames it. Either way the capture sees the uncompressed payload and
/// the outbox caps count its uncompressed length.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Frame {
    Raw(Vec<u8>),
    Ready {
        /// What goes in the frame: the deflate when it was smaller, else the
        /// payload itself (`compress_payload`'s answer).
        wire: Vec<u8>,
        /// The payload, for the capture, when `wire` is a deflate of it.
        /// `Writers::post_frame` drops it for a connection with no capture.
        raw: Option<Vec<u8>>,
        /// The payload's length: what the outbox caps count.
        raw_len: usize,
    },
    /// A mergeable `DoDestinyUpdate` (PA-10): not yet encoded. The outbox may
    /// append the next same-stamp part to it; the writer encodes it when it is
    /// taken. See [`Part`].
    Part(Part),
}

impl Frame {
    /// Decide the compression now. Time spent deflating is added to
    /// `prof::Sec::Deflate` (from whichever thread ran it; the adds are
    /// relaxed atomics, so the sum is order-free).
    pub(crate) fn encoded(payload: Vec<u8>) -> Frame {
        let raw_len = payload.len();
        let deflating = Instant::now();
        let z = match compress_payload(&payload) {
            Cow::Borrowed(_) => None,
            Cow::Owned(z) => Some(z),
        };
        match z {
            None => Frame::Ready {
                wire: payload,
                raw: None,
                raw_len,
            },
            Some(z) => {
                crate::live_status::prof::add(
                    crate::live_status::prof::Sec::Deflate,
                    deflating.elapsed(),
                );
                Frame::Ready {
                    wire: z,
                    raw: Some(payload),
                    raw_len,
                }
            }
        }
    }

    /// The payload length the outbox caps count.
    pub(crate) fn cap_len(&self) -> usize {
        match self {
            Frame::Raw(p) => p.len(),
            Frame::Ready { raw_len, .. } => *raw_len,
            Frame::Part(p) => p.est,
        }
    }

    /// The uncompressed payload (tests, benches).
    #[cfg(test)]
    pub(crate) fn payload(&self) -> &[u8] {
        match self {
            Frame::Raw(p) => p,
            Frame::Ready { wire, raw, .. } => raw.as_deref().unwrap_or(wire),
            Frame::Part(_) => panic!("a Part has no payload until encode_part"),
        }
    }

    /// The bytes that go in the frame on the wire (tests, benches).
    #[cfg(test)]
    pub(crate) fn wire_bytes(&self) -> Vec<u8> {
        match self {
            Frame::Raw(p) => compress_payload(p).into_owned(),
            Frame::Ready { wire, .. } => wire.clone(),
            Frame::Part(_) => panic!("a Part has no wire bytes until encode_part"),
        }
    }
}

/// Most entries one merged part may hold. **[A]** Keeps a frame reasonable
/// for the client's `DoPreTick` and the writer's deflate. Falsified by a
/// client hitch as long as the batch, or deflate time in `tick writers`
/// going up rather than down.
pub(crate) const MAX_MERGED_ENTRIES: usize = 256;

/// How much later than the back entry a part may be due and still merge into
/// it. **[A]** Payload-turn posts take `Instant::now()` each, microseconds
/// apart, so an exact rule would never merge them; a part posted with an
/// `after` goes out at most this much early. Falsified by a client that
/// reacts to an `after`-delayed Destiny entry arriving 5 ms early.
pub(crate) const MERGE_DUE_SLACK: Duration = Duration::from_millis(5);

/// The envelope a part's byte estimate adds to its pickled body. **[A]**
/// Falsified by an outbox high-water line more than 2x off the bytes written.
const PART_ENVELOPE: usize = 256;

/// `--coalesce-destiny` (PA-10). Read with `Relaxed` at the post sites.
static COALESCE: AtomicBool = AtomicBool::new(false);

/// Turn Destiny coalescing on or off (`--coalesce-destiny`). Process-wide.
pub fn set_coalesce_destiny(on: bool) {
    COALESCE.store(on, Ordering::Relaxed);
}

/// Whether mergeable Destiny pushes are posted as [`Part`]s.
pub fn coalesce_destiny() -> bool {
    #[cfg(test)]
    if FORCE.with(|f| f.get()) {
        return true;
    }
    COALESCE.load(Ordering::Relaxed)
}

#[cfg(test)]
thread_local! {
    /// Coalescing on for **this test thread only**: the real flag is
    /// process-wide, and a test must not change what its neighbours run.
    static FORCE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test seam: coalescing on for the calling thread.
#[cfg(test)]
pub(crate) fn force_coalesce_for_test(on: bool) {
    FORCE.with(|f| f.set(on));
}

/// What [`destiny_part`] takes from a push.
pub(crate) type PartEntries = (i64, usize, Arc<[PyValue]>);

/// **The classifier (PA-10 §5.1)**, on what the client would receive. `Some`
/// `(stamp, entry count, entries)` only for a `DoDestinyUpdate` of exactly two
/// arguments, `waitForBubble` exactly `Int(0)`, and a non-empty list of
/// `(Int stamp, (Bytes name, args))` entries that share one stamp and name no
/// `SetState` or `PackagedAction` anywhere. Anything else is a frame today's
/// way. A three-argument push (`dogmaMessages`) is a barrier: the client keeps
/// only the last event per key when it merges dogma messages.
pub(crate) fn destiny_part(event: &str, args: &[PyValue]) -> Option<PartEntries> {
    if event != "DoDestinyUpdate" || args.len() != 2 || args[1] != PyValue::Int(0) {
        return None;
    }
    let PyValue::List(entries) = &args[0] else {
        return None;
    };
    let mut stamp = None;
    for e in entries {
        let PyValue::Tuple(t) = e else { return None };
        let [PyValue::Int(s), PyValue::Tuple(named)] = t.as_slice() else {
            return None;
        };
        let Some(PyValue::Bytes(name)) = named.first() else {
            return None;
        };
        if name == b"SetState" || name == b"PackagedAction" || named.len() != 2 {
            return None;
        }
        if *stamp.get_or_insert(*s) != *s {
            return None;
        }
    }
    Some((stamp?, entries.len(), entries.as_slice().into()))
}

/// One push's entries, shared by every recipient (an `Arc`), with the
/// numbers a [`Part`] needs. Built once per push.
#[derive(Debug, Clone)]
pub(crate) struct PartSrc {
    stamp: i64,
    entries: usize,
    segs: Arc<[PyValue]>,
    est: usize,
}

impl PartSrc {
    /// `body_len` is the push's pickled body (`notification_body`), once.
    pub(crate) fn new(src: PartEntries, body_len: usize) -> PartSrc {
        PartSrc {
            stamp: src.0,
            entries: src.1,
            segs: src.2,
            est: body_len.saturating_add(PART_ENVELOPE),
        }
    }

    /// Classify a push's `to_push()` output. The body is pickled here for its
    /// length only; the entries are encoded by the writer.
    pub(crate) fn of(event: &str, args: &[PyValue]) -> Option<PartSrc> {
        let src = destiny_part(event, args)?;
        let body = eve_proto::MachoPacket::notification_body(args.to_vec());
        Some(PartSrc::with_body(src, &body))
    }

    /// [`Self::new`] for a body already pickled (`notification_body`).
    pub(crate) fn with_body(src: PartEntries, body: &PyValue) -> PartSrc {
        let len = match body {
            PyValue::Bytes(b) => b.len(),
            _ => 0,
        };
        PartSrc::new(src, len)
    }

    /// The byte estimate each recipient's part counts.
    pub(crate) fn est(&self) -> usize {
        self.est
    }

    pub(crate) fn for_client(&self, node_id: i64, client_id: i64) -> Part {
        Part {
            node_id,
            client_id,
            stamp: self.stamp,
            segs: vec![Arc::clone(&self.segs)],
            entries: self.entries,
            est: self.est,
        }
    }
}

/// A mergeable `DoDestinyUpdate`, queued unencoded (PA-10 §5.2). Its
/// `segs` are the entry lists of the pushes merged into it, in post order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Part {
    node_id: i64,
    client_id: i64,
    stamp: i64,
    segs: Vec<Arc<[PyValue]>>,
    entries: usize,
    /// Pickled bodies plus an envelope each: what the outbox byte cap counts.
    est: usize,
}

/// The payload a [`Part`] goes out as (PA-10 §5.4): one `DoDestinyUpdate`
/// whose list is every merged entry in order and whose `waitForBubble` is 0.
/// A one-part entry is byte for byte the frame the clock encoded before.
pub(crate) fn encode_part(p: &Part) -> Result<Vec<u8>, eve_proto::ProtoError> {
    let entries: Vec<PyValue> = p.segs.iter().flat_map(|s| s.iter().cloned()).collect();
    eve_proto::MachoPacket::notification(
        eve_proto::MachoAddress::node(p.node_id),
        "DoDestinyUpdate",
        "*clientID",
        vec![PyValue::Int(p.client_id)],
        vec![PyValue::List(entries), PyValue::Int(0)],
    )
    .to_payload()
}
use std::collections::{HashMap, VecDeque};
use std::io::{self, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

/// Frames waiting for their due time, behind a lock of their **own**.
///
/// **Why this is not a field of [`Wire`] any more.** It was, and that made
/// every pusher take the same mutex the ticker holds while it is inside
/// `write_all`. A peer whose TCP receive window is full blocks that write for
/// up to `conn::WRITE_TIMEOUT` -- thirty seconds -- and any thread wanting to
/// *queue* a frame for anybody waited behind it. Two of those threads held the
/// world write lock at the time (`dispatch::deliver_elsewhere` and
/// `dispatch::post_session_change`), so one slow client froze the world for
/// every player. `docs/port-tracker.md` §26.2 designed the fix and §26.3
/// found the second site; `docs/reference/threads-and-locks.md` F2 is the audit.
///
/// Splitting it makes the rule structural rather than careful: **no path that
/// holds the world lock can now wait on a socket**, because the only lock a
/// pusher takes is this one, and nothing does I/O under it.
///
/// `closed` is not redundant with the connection's `Arc` dying. [`Writers`]
/// holds a strong handle to the outbox so it can post without touching the
/// wire, so the queue can outlive the socket by one registry pass; refusing a
/// post after `close` is what stops a frame being queued for a connection that
/// has gone.
///
/// **It is bounded** (IP30, V3 F5 and R6). A peer that keeps its socket open
/// and stops reading used to grow this queue until its connection ended. Now
/// a post that would take it past [`OUTBOX_FRAMES`] or [`OUTBOX_BYTES`] is
/// refused, the outbox is closed and emptied, and the socket is shut down in
/// both directions, so the connection's own thread wakes from its read (or
/// its blocked write) and ends the session through `server::SessionGuard`
/// like any other disconnect.
#[derive(Default)]
pub struct Outbox {
    queue: VecDeque<(Instant, Frame)>,
    /// The payload bytes in `queue`, kept beside it so the cap is O(1).
    bytes: usize,
    closed: bool,
    /// A second handle on the connection's socket, used **only** to shut it
    /// down when the cap is crossed. A pusher holds the world lock and must
    /// not take the wire (that is F2), so the outbox carries its own way to
    /// end the connection. `shutdown` does not block. It is taken out under
    /// the lock and used after the lock is released ([`Refused::end`]), so
    /// nothing does I/O under this lock. `close` drops it, so a finished
    /// connection does not keep an fd open through the registry's strong
    /// handle.
    kill: Option<TcpStream>,
    /// NET-TAKEOVER ([`Writers::evict`]): once this instant passes, or the
    /// close frame has left, the wire hangs up. `None` for every ordinary
    /// connection.
    evict_after: Option<Instant>,
}

/// The most frames one connection may have waiting. **[A]** At 1 Hz Destiny
/// plus notifications, a client that reads normally keeps a handful queued
/// (they drain every [`TICK`]). This allows minutes of a stalled reader before
/// the server gives up on it. Falsified by a `[push] outbox peak` line from a
/// healthy client or a bot soak within a quarter of it.
pub const OUTBOX_FRAMES: usize = 4096;

/// The most payload bytes one connection may have waiting. **[A]** Sized so
/// that 16 stalled connections (the `REMOTE_CAP` listener cap) cost at most
/// 256 MiB. Falsified the same way as [`OUTBOX_FRAMES`].
pub const OUTBOX_BYTES: usize = 16 << 20;

/// The deepest any outbox has been since the process started: frames and
/// bytes, each its own maximum. The push ticker prints a line when either
/// rises, so a soak or client run shows how far below the caps it stayed.
static PEAK_FRAMES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

/// **Bench counters** (lane bench-ramp, `tools/bench-ramp.sh`): what left the
/// world through [`Writers::post`] since the last `tick health` line, and the
/// `DoDestinyUpdate` share of it (frames are per recipient). Taken (reset)
/// by [`TickHealth::report`]. Cheap relaxed adds, outside every lock.
static POST_FRAMES: AtomicUsize = AtomicUsize::new(0);
static POST_BYTES: AtomicUsize = AtomicUsize::new(0);
static DESTINY_FRAMES: AtomicUsize = AtomicUsize::new(0);
static DESTINY_BYTES: AtomicUsize = AtomicUsize::new(0);
/// PA-10: parts posted, and how many of them were appended to a part already
/// queued (so `merged / parts` is the reduction). Taken with the rest.
static DESTINY_PARTS: AtomicUsize = AtomicUsize::new(0);
static DESTINY_MERGED: AtomicUsize = AtomicUsize::new(0);

/// Why [`Outbox::post`] did not take a frame.
#[derive(Debug)]
enum Refused {
    /// The connection had already ended. A normal answer.
    Closed,
    /// The post would have crossed a cap. The outbox is now closed, and the
    /// socket handle in here must be shut down once the lock is released.
    Overflow {
        frames: usize,
        bytes: usize,
        kill: Option<TcpStream>,
    },
}

impl Refused {
    /// End the connection if the cap was crossed. Call it with **no outbox
    /// lock held**; `shutdown` does not wait on the peer.
    fn end(self, who: impl core::fmt::Display) {
        if let Refused::Overflow {
            frames,
            bytes,
            kill,
        } = self
        {
            println!(
                "[push] {who}: outbox over cap at {frames} frames, \
{bytes} bytes (cap {OUTBOX_FRAMES} frames, {OUTBOX_BYTES} bytes); ending the connection"
            );
            if let Some(s) = kill {
                let _ = s.shutdown(Shutdown::Both);
            }
        }
    }
}

impl Outbox {
    /// Queue a frame, unless the connection has ended or the frame would take
    /// the queue past [`OUTBOX_FRAMES`] or [`OUTBOX_BYTES`].
    ///
    /// Crossing a cap closes the outbox for good: a peer that has stopped
    /// reading is not caught up by dropping one frame, and a stream with a
    /// hole in it is worse than none.
    fn post(&mut self, due: Instant, frame: Frame) -> Result<(), Refused> {
        if self.closed {
            return Err(Refused::Closed);
        }
        let bytes = self.bytes.saturating_add(frame.cap_len());
        if self.queue.len() >= OUTBOX_FRAMES || bytes > OUTBOX_BYTES {
            let refused = Refused::Overflow {
                frames: self.queue.len(),
                bytes: self.bytes,
                kill: self.kill.take(),
            };
            self.close();
            return Err(refused);
        }
        self.queue.push_back((due, frame));
        self.bytes = bytes;
        PEAK_FRAMES.fetch_max(self.queue.len(), Ordering::Relaxed);
        PEAK_BYTES.fetch_max(self.bytes, Ordering::Relaxed);
        Ok(())
    }

    /// Queue a [`Part`] (PA-10 §5.2): appended to the **back** entry when that
    /// is a part with the same stamp, due no later than [`MERGE_DUE_SLACK`]
    /// after the back entry, and room for the entries; otherwise a new
    /// entry. Never across a frame, never reordered. Tighter than the
    /// research spec's §5.2 (`back.due <= due`, any gap): merged into a back
    /// entry due earlier, a part posted with an `after` would go out before
    /// it; within the slack it goes out at most that much early, and a part
    /// due earlier than the back entry waits only as long as the FIFO would
    /// hold it anyway. `Ok(true)` when it merged.
    ///
    /// Caps: a new entry is one frame slot; a merge adds none. Either way its
    /// byte estimate counts, and crossing a cap closes the outbox as
    /// [`Self::post`] does.
    fn post_part(&mut self, due: Instant, part: Part) -> Result<bool, Refused> {
        if self.closed {
            return Err(Refused::Closed);
        }
        let bytes = self.bytes.saturating_add(part.est);
        // A part due by now goes out at the next drain whether it merges or
        // not, so it may join a back entry posted any time before it
        // (destiny-batch: every pilot's command is its own payload turn, so
        // `due` is that turn's `Instant::now()`, tens of ms after the back
        // entry's, and the slack alone split one stamp into a frame per
        // pilot -- 1300 one-entry `DoDestinyUpdate`s for one tick in the
        // owner's client log of 2026-11-03).
        let latest = (*self.queue.back().map_or(&due, |(d, _)| d)).max(Instant::now());
        let merge = matches!(
            self.queue.back(),
            Some((_, Frame::Part(b)))
                if b.stamp == part.stamp
                    && due <= latest + MERGE_DUE_SLACK
                    && b.entries + part.entries <= MAX_MERGED_ENTRIES
        );
        if bytes > OUTBOX_BYTES || (!merge && self.queue.len() >= OUTBOX_FRAMES) {
            let refused = Refused::Overflow {
                frames: self.queue.len(),
                bytes: self.bytes,
                kill: self.kill.take(),
            };
            self.close();
            return Err(refused);
        }
        if merge && let Some((_, Frame::Part(b))) = self.queue.back_mut() {
            b.entries += part.entries;
            b.est += part.est;
            b.segs.extend(part.segs);
        } else {
            self.queue.push_back((due, Frame::Part(part)));
        }
        self.bytes = bytes;
        PEAK_FRAMES.fetch_max(self.queue.len(), Ordering::Relaxed);
        PEAK_BYTES.fetch_max(self.bytes, Ordering::Relaxed);
        Ok(merge)
    }

    /// Everything due by `now`, removed. Called with the small lock held and
    /// **nothing written under it** -- the caller writes after releasing.
    fn take_due(&mut self, now: Instant) -> Vec<Frame> {
        let mut ready = Vec::new();
        while self.queue.front().is_some_and(|(due, _)| *due <= now) {
            let frame = self.queue.pop_front().expect("front() just matched").1;
            self.bytes -= frame.cap_len();
            ready.push(frame);
        }
        ready
    }

    fn close(&mut self) {
        self.closed = true;
        self.queue.clear();
        self.bytes = 0;
        self.kill = None;
    }

    fn len(&self) -> usize {
        self.queue.len()
    }

    /// Each queued entry's Destiny entries in order: a [`Part`]'s merged
    /// list, `None` for any other frame. Tests.
    #[cfg(test)]
    pub(crate) fn queued_parts(&self) -> Vec<Option<Vec<PyValue>>> {
        self.queue
            .iter()
            .map(|(_, f)| match f {
                Frame::Part(p) => Some(p.segs.iter().flat_map(|s| s.iter().cloned()).collect()),
                _ => None,
            })
            .collect()
    }
}

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
    /// for reading, because the read loop blocks for up to four minutes at a
    /// time and any design sharing one handle is a design where nothing can
    /// ever be pushed.
    stream: TcpStream,
    capture: Option<Capture>,
    /// Frames to write later. Shared with [`Writers`] so a pusher never has
    /// to take this mutex -- see [`Outbox`].
    outbox: Arc<Mutex<Outbox>>,
    /// Framed bytes taken from the outbox that the socket has not yet
    /// accepted (`pending[pending_off..]`). A writer thread writes for at most
    /// [`WRITE_BUDGET`] per pass; what the kernel did not take waits here and
    /// goes first next pass, before anything newer is taken from the outbox, so
    /// the byte stream is exactly what one blocking `write_all` would have
    /// produced.
    pending: Vec<u8>,
    pending_off: usize,
    /// Since when the socket has taken no byte of `pending`; `None` while it
    /// drains. Past [`STALL_LIMIT`] the connection is hung up.
    stalled_since: Option<Instant>,
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
        // Without the second handle a capped outbox still refuses and closes;
        // it just cannot wake the connection's thread, which then ends at its
        // next read or write instead.
        let outbox = Outbox {
            kill: stream.try_clone().ok(),
            ..Outbox::default()
        };
        Wire {
            stream,
            capture,
            outbox: Arc::new(Mutex::new(outbox)),
            pending: Vec::new(),
            pending_off: 0,
            stalled_since: None,
            closed: false,
        }
    }

    /// Take the outbox's verdict: one closed by crossing its cap
    /// ([`Outbox::post`]) closes this wire too, so nothing more is written.
    fn adopt_outbox_close(&mut self) {
        if !self.closed && self.outbox.lock().is_ok_and(|o| o.closed) {
            self.closed = true;
        }
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
    /// `handle_select_character` sends the session change before the answer --
    /// and a push must not be able to land between them. One call, one guard,
    /// one ordering.
    pub fn send_all(&mut self, payloads: &[Vec<u8>]) -> Result<(), ConnEnd> {
        self.send_all_inner(payloads, false)
    }

    /// [`Self::send_all`]; `prof` is the push-ticker's call (`flush_due`),
    /// whose frames, bytes, deflate and write times go to the PA-0 writer
    /// line (`live_status::prof`). Replies written by connection threads are
    /// not counted there.
    fn send_all_inner(&mut self, payloads: &[Vec<u8>], prof: bool) -> Result<(), ConnEnd> {
        use crate::live_status::prof as p;
        self.adopt_outbox_close();
        // Bytes a writer thread could not finish go out first: this reply was
        // produced after them and must follow them on the wire.
        if self.pending_off < self.pending.len() && !self.closed {
            let tail = std::mem::take(&mut self.pending);
            let off = std::mem::take(&mut self.pending_off);
            self.stalled_since = None;
            self.stream.write_all(&tail[off..]).map_err(ConnEnd::Io)?;
        }
        for payload in payloads {
            if self.closed {
                return Err(ConnEnd::DispatchClosed);
            }
            let deflating = Instant::now();
            let wire = compress_payload(payload);
            if prof && matches!(wire, Cow::Owned(_)) {
                p::add(p::Sec::Deflate, deflating.elapsed());
            }
            let mut framed = Vec::with_capacity(wire.len() + 4);
            write_frame(&wire, &mut framed).map_err(ConnEnd::Framing)?;
            if let Some(c) = self.capture.as_mut() {
                c.write(Direction::ServerToClient, payload)
                    .map_err(ConnEnd::Io)?;
            }
            let writing = Instant::now();
            self.stream.write_all(&framed).map_err(ConnEnd::Io)?;
            if prof {
                p::add(p::Sec::WriteAll, writing.elapsed());
                p::add_n(p::Sec::Bytes, framed.len() as u64);
            }
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
            Some(c) => c
                .write(Direction::ClientToServer, payload)
                .map_err(ConnEnd::Io),
            None => Ok(()),
        }
    }

    /// The shared queue, for [`Writers::register`] to keep a handle on.
    pub fn outbox(&self) -> Arc<Mutex<Outbox>> {
        Arc::clone(&self.outbox)
    }

    /// Queue a frame for the ticker to write once `due` has passed.
    ///
    /// Still here because `conn::serve` already holds this wire to call
    /// `send_all` in the same breath, so for *that* caller the small lock
    /// costs an extra acquisition and buys nothing. Everyone else should use
    /// [`Writers::post`], which never touches the wire at all.
    ///
    /// Past the cap this closes the wire and ends the connection, exactly as
    /// [`Writers::post`] does.
    pub fn post(&mut self, due: Instant, payload: Vec<u8>) {
        let refused = match self.outbox.lock() {
            Ok(mut o) => o.post(due, Frame::Raw(payload)).err(),
            Err(_) => None,
        };
        if let Some(r @ Refused::Overflow { .. }) = refused {
            self.closed = true;
            r.end("own connection");
            let _ = self.stream.shutdown(Shutdown::Both);
        }
    }

    /// Write everything due by `now`. Called only from the ticker.
    ///
    /// **The drain and the write are deliberately not under one lock.** The
    /// queue is emptied, the small lock released, and only then is the socket
    /// touched -- so a blocked `write_all` holds the wire mutex but never the
    /// outbox, which is the whole point of the split.
    pub fn flush_due(&mut self, now: Instant) -> Result<(), ConnEnd> {
        let flushed = self.flush_pass(now);
        // NET-TAKEOVER: an evicted connection hangs up once its close frame
        // has gone out, or at the deadline however stalled the reader is. The
        // connection's thread then reads end-of-stream and ends its session
        // through `SessionGuard` like any disconnect.
        if flushed.is_ok() && !self.closed {
            let hang = self.outbox.lock().is_ok_and(|o| {
                o.evict_after
                    .is_some_and(|at| now >= at || (o.queue.is_empty() && self.unsent() == 0))
            });
            if hang {
                self.pending.clear();
                self.pending_off = 0;
                self.hang_up();
            }
        }
        flushed
    }

    fn flush_pass(&mut self, now: Instant) -> Result<(), ConnEnd> {
        self.adopt_outbox_close();
        if self.closed {
            return Ok(());
        }
        // A tail from an earlier pass goes first, and nothing newer is taken
        // from the outbox until it is gone: that is the per-client order.
        if self.pending_off >= self.pending.len() {
            let ready = match self.outbox.lock() {
                Ok(mut o) => o.take_due(now),
                Err(_) => return Ok(()),
            };
            if ready.is_empty() {
                return Ok(());
            }
            self.pending.clear();
            self.pending_off = 0;
            for frame in &ready {
                // `Raw`: decide the compression here, as ever. `Ready`: the
                // encode phase did; frame it. `Part` (PA-10): encode it now,
                // merged entries and all, then as `Raw`. The capture gets the
                // payload either way, so it records the frame that left.
                let owned: Vec<u8>;
                let (wire, payload): (Cow<'_, [u8]>, &[u8]) = match frame {
                    Frame::Raw(payload) => (Self::deflate(payload), payload),
                    Frame::Ready { wire, raw, .. } => {
                        (Cow::Borrowed(wire), raw.as_deref().unwrap_or(wire))
                    }
                    Frame::Part(part) => {
                        let Ok(bytes) = encode_part(part) else {
                            println!("[push] DoDestinyUpdate would not encode; dropped");
                            continue;
                        };
                        DESTINY_FRAMES.fetch_add(1, Ordering::Relaxed);
                        DESTINY_BYTES.fetch_add(bytes.len(), Ordering::Relaxed);
                        owned = bytes;
                        (Self::deflate(&owned), &owned[..])
                    }
                };
                let before = self.pending.len();
                write_frame(&wire, &mut self.pending).map_err(ConnEnd::Framing)?;
                crate::live_status::prof::add_n(
                    crate::live_status::prof::Sec::Bytes,
                    (self.pending.len() - before) as u64,
                );
                if let Some(c) = self.capture.as_mut() {
                    c.write(Direction::ServerToClient, payload)
                        .map_err(ConnEnd::Io)?;
                }
            }
        }
        self.write_pending(now, WRITE_BUDGET)
    }

    /// [`compress_payload`], with the deflate time counted when it deflated.
    fn deflate(payload: &[u8]) -> Cow<'_, [u8]> {
        let deflating = Instant::now();
        let wire = compress_payload(payload);
        if matches!(wire, Cow::Owned(_)) {
            crate::live_status::prof::add(
                crate::live_status::prof::Sec::Deflate,
                deflating.elapsed(),
            );
        }
        wire
    }

    /// Write `pending` for at most `budget`, keeping what the socket did not
    /// take. The socket's write timeout is shared with the connection thread's
    /// clone (one file description) and is put back before returning; this
    /// runs under the wire mutex, so nobody else is writing meanwhile.
    fn write_pending(&mut self, now: Instant, budget: Duration) -> Result<(), ConnEnd> {
        let start = Instant::now();
        let mut progressed = false;
        let mut failed = None;
        while self.pending_off < self.pending.len() {
            let left = budget.saturating_sub(start.elapsed());
            if left.is_zero() {
                break;
            }
            let _ = self
                .stream
                .set_write_timeout(Some(left.max(Duration::from_millis(1))));
            let writing = Instant::now();
            let wrote = self.stream.write(&self.pending[self.pending_off..]);
            crate::live_status::prof::add(
                crate::live_status::prof::Sec::WriteAll,
                writing.elapsed(),
            );
            match wrote {
                Ok(0) => {
                    failed = Some(io::Error::from(io::ErrorKind::WriteZero));
                    break;
                }
                Ok(n) => {
                    self.pending_off += n;
                    progressed = true;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        let _ = self
            .stream
            .set_write_timeout(Some(crate::conn::WRITE_TIMEOUT));
        if let Some(e) = failed {
            self.pending.clear();
            self.pending_off = 0;
            return Err(ConnEnd::Io(e));
        }
        if self.pending_off >= self.pending.len() {
            self.pending.clear();
            self.pending_off = 0;
            self.stalled_since = None;
            return Ok(());
        }
        self.pending.drain(..self.pending_off);
        self.pending_off = 0;
        if progressed {
            self.stalled_since = Some(now);
        }
        let since = *self.stalled_since.get_or_insert(now);
        if now.saturating_duration_since(since) >= STALL_LIMIT {
            self.pending.clear();
            self.hang_up();
            return Err(ConnEnd::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "peer took no bytes for too long",
            )));
        }
        Ok(())
    }

    /// Bytes taken from the outbox and not yet accepted by the socket.
    pub fn unsent(&self) -> usize {
        self.pending.len() - self.pending_off
    }

    pub fn capture_count(&self) -> u64 {
        self.capture.as_ref().map(Capture::count).unwrap_or(0)
    }

    /// How many frames are waiting for their due time. Tests and diagnostics.
    pub fn queued(&self) -> usize {
        self.outbox.lock().map(|o| o.len()).unwrap_or(0)
    }

    /// Closed by [`Wire::close`], or by its outbox crossing a cap.
    pub fn is_closed(&self) -> bool {
        self.closed || self.outbox.lock().is_ok_and(|o| o.closed)
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
            if let Ok(mut o) = self.outbox.lock() {
                o.close();
            }
            let _ = self.stream.shutdown(Shutdown::Write);
        }
    }

    /// Close the write side **and** the read side (lane shutdown): the
    /// connection's thread sees end-of-stream on its next read, so its
    /// session ends through the normal path (`SessionGuard`) and the
    /// client sees the server close. Idempotent.
    pub fn hang_up(&mut self) {
        self.close();
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

/// Where to write, for every live connection.
///
/// `Weak`, not `Arc`, and that is the whole lifetime design: a connection's
/// strong handle lives in `serve_one`'s frame and nowhere else, so a connection
/// that has ended cannot be written to, and nothing has to be told that it
/// ended. The registry notices on its next pass.
///
/// **This is still not the session table.** It answers *where do I write*;
/// the session table answers *who is online* and owns identity
/// (`crate::session_table`). Keeping them apart is what stopped the session
/// table from being a rewrite of this, and the two now meet at exactly one
/// point: a `client_id`. `session_table::of_character` turns a character into
/// client ids, and [`Writers::wire_for`] turns a client id into somewhere to
/// write. Neither knows anything else about the other.
#[derive(Default)]
pub struct Writers {
    /// Keyed by `client_id` -- see `session::Session::client_id`, which is
    /// per connection and not per account. Two connections of one account are
    /// two entries, which is the whole reason the key can be used to pick one
    /// of them.
    ///
    /// A map (PA-1): `post` runs once per frame and used to scan every
    /// connection under this one mutex. `Row::seq` is the registration order,
    /// so [`Writers::live`] still answers in it. The first live row for an id
    /// wins, as the scan did.
    inner: Mutex<HashMap<i64, Row>>,
    next_seq: AtomicU64,
    /// The shard threads, once [`spawn_writers`] has started them. Chat
    /// connections (PA-5) hand their socket writes to them through
    /// [`Writers::wake`].
    shards: OnceLock<Vec<Shard>>,
}

/// What a shard thread calls on a connection that asked to be written
/// ([`Writers::wake`]). The connection owns its queue, its framing and its
/// socket; the pool only supplies the thread, so one connection's bytes keep
/// their order (it is always the same shard) and no connection has a thread.
pub trait Drain: Send + Sync {
    /// Write what is queued, for a bounded time. [`Drained::More`] asks to be
    /// called again soon (the socket took less than was queued); `Idle`
    /// means nothing is left and the next post will wake the shard again.
    fn drain(&self, now: Instant) -> Drained;
}

/// [`Drain::drain`]'s answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drained {
    Idle,
    More,
}

/// One shard's wake-up handle and the connections waiting to be drained.
struct Shard {
    thread: OnceLock<std::thread::Thread>,
    dirty: Mutex<Vec<Arc<dyn Drain>>>,
}

/// One live connection: where to write, and where to queue.
///
/// The `Weak<Mutex<Wire>>` is still the liveness authority -- a connection's
/// only strong wire handle lives in `serve_one`'s frame, so `strong_count()`
/// is how the registry notices a departure. The outbox handle is **strong**,
/// deliberately: a pusher must be able to queue without first proving the
/// socket is alive, and `Outbox::closed` is what makes a post to a departed
/// connection a refusal rather than a leak.
struct Row {
    seq: u64,
    wire: Weak<Mutex<Wire>>,
    outbox: Arc<Mutex<Outbox>>,
    /// Whether the wire records a capture (fixed at `Wire::new`): a
    /// pre-compressed frame keeps its uncompressed copy only then.
    captured: bool,
}

impl Writers {
    pub fn register(&self, client_id: i64, wire: &Arc<Mutex<Wire>>) {
        // Locking the wire here is uncontended by construction: `register`
        // runs on the connection's own thread before anything can be pushed
        // to it, which is the one moment in a connection's life when nobody
        // else holds this mutex.
        let (outbox, captured) = match wire.lock() {
            Ok(w) => (w.outbox(), w.capture.is_some()),
            Err(e) => {
                let w = e.into_inner();
                (w.outbox(), w.capture.is_some())
            }
        };
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let mut v = self.guard();
        v.retain(|_, r| r.wire.strong_count() > 0);
        v.entry(client_id).or_insert(Row {
            seq,
            wire: Arc::downgrade(wire),
            outbox,
            captured,
        });
    }

    /// Queue a frame for a connection **without touching its socket lock**.
    ///
    /// This is the function §26.2 asked for. Every caller that holds the world
    /// lock uses it, so the inversion that let one slow peer freeze the world
    /// for thirty seconds has nowhere left to happen: the only mutex taken
    /// here guards a `VecDeque`, and no I/O is ever done under it.
    ///
    /// `false` means the frame was not queued -- the connection has gone, or
    /// its outbox is closed. That is a normal answer, the same one
    /// [`Writers::wire_for`] gives by returning `None`.
    pub fn post(&self, client_id: i64, due: Instant, payload: Vec<u8>) -> bool {
        self.post_frame(client_id, due, Frame::Raw(payload))
    }

    /// NET-TAKEOVER: tell a connection a newer login replaced it, then hang it
    /// up. `payload` (a marshalled `GPSTransportClosed`) is queued due now;
    /// the wire hangs up once it has left or after `grace`, whichever is first
    /// (a stalled reader cannot hold the seat). Takes no wire lock and does no
    /// I/O, so it is safe under the world lock, but callers still run it after
    /// the lock is released. `false`: the connection has gone.
    pub fn evict(&self, client_id: i64, payload: Vec<u8>, grace: Duration) -> bool {
        let now = Instant::now();
        let Some(outbox) = self.guard().get(&client_id).map(|r| Arc::clone(&r.outbox)) else {
            return false;
        };
        // One outbox lock for both, so a flush cannot take the frame between
        // the post and the deadline.
        let posted = match outbox.lock() {
            Ok(mut o) => {
                let r = o.post(now, Frame::Raw(payload));
                if r.is_ok() {
                    o.evict_after = Some(now + grace);
                }
                r
            }
            Err(_) => return false,
        };
        match posted {
            Ok(()) => true,
            Err(r) => {
                r.end(format_args!("client {client_id}"));
                false
            }
        }
    }

    /// [`Self::post`] for a [`Frame`] (the encode phase's output). Same
    /// answers, same caps: they count the uncompressed length.
    pub(crate) fn post_frame(&self, client_id: i64, due: Instant, mut frame: Frame) -> bool {
        if let Frame::Part(part) = frame {
            return self.post_part(client_id, due, part);
        }
        let outbox = {
            let v = self.guard();
            match v.get(&client_id) {
                Some(r) => {
                    if !r.captured
                        && let Frame::Ready { raw, .. } = &mut frame
                    {
                        *raw = None;
                    }
                    Arc::clone(&r.outbox)
                }
                None => return false,
            }
        };
        // The guard is gone before `end` runs: a cap crossed here shuts the
        // socket down with no outbox lock held (§3.4 of
        // `docs/reference/threads-and-locks.md`), and `shutdown` does not block.
        let len = frame.cap_len();
        let posted = match outbox.lock() {
            Ok(mut o) => o.post(due, frame),
            Err(_) => return false,
        };
        match posted {
            Ok(()) => {
                POST_FRAMES.fetch_add(1, Ordering::Relaxed);
                POST_BYTES.fetch_add(len, Ordering::Relaxed);
                true
            }
            Err(r) => {
                r.end(format_args!("client {client_id}"));
                false
            }
        }
    }

    /// [`Self::post_frame`] for a [`Part`] (PA-10): the connection's outbox
    /// may merge it into its tail. Counts one post and its byte estimate, plus
    /// the part counters.
    pub(crate) fn post_part(&self, client_id: i64, due: Instant, part: Part) -> bool {
        let outbox = match self.guard().get(&client_id) {
            Some(r) => Arc::clone(&r.outbox),
            None => return false,
        };
        let est = part.est;
        let posted = match outbox.lock() {
            Ok(mut o) => o.post_part(due, part),
            Err(_) => return false,
        };
        match posted {
            Ok(merged) => {
                POST_FRAMES.fetch_add(1, Ordering::Relaxed);
                POST_BYTES.fetch_add(est, Ordering::Relaxed);
                DESTINY_PARTS.fetch_add(1, Ordering::Relaxed);
                if merged {
                    DESTINY_MERGED.fetch_add(1, Ordering::Relaxed);
                }
                true
            }
            Err(r) => {
                r.end(format_args!("client {client_id}"));
                false
            }
        }
    }

    /// Ask the shard that owns `key` (modulo the pool size) to call
    /// `conn.drain` now. `false` when no pool is running. A connection should
    /// ask once per burst, not once per message: the shard drains everything
    /// queued by the time it runs.
    pub fn wake(&self, key: u64, conn: Arc<dyn Drain>) -> bool {
        let Some(shards) = self.shards.get() else {
            return false;
        };
        let shard = &shards[(key % shards.len() as u64) as usize];
        shard
            .dirty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(conn);
        if let Some(t) = shard.thread.get() {
            t.unpark();
        }
        true
    }

    /// Whether [`spawn_writers`] has run on this registry.
    pub fn pool_running(&self) -> bool {
        self.shards.get().is_some()
    }

    /// Where to write for this connection, if it is still live.
    ///
    /// **The one thing that makes a notification deliverable to somebody
    /// other than the caller.** Until this existed, `GpsSession::notify`
    /// could only address the connection that was mid-payload, so every push
    /// told the actor about their own action -- fine for `OnAccountChange`
    /// and exactly wrong for a fleet invite, whose entire point is that
    /// someone else finds out. `docs/lanes/fleet.md` §4.
    ///
    /// `None` means the connection has gone. That is a normal answer and not
    /// an error: the player closed the client between the action and the
    /// notification, and dropping the frame is correct.
    pub fn wire_for(&self, client_id: i64) -> Option<Arc<Mutex<Wire>>> {
        // A dead row upgrades to `None`; the sweep is `live()`'s and
        // `register`'s, not a scan per lookup.
        self.guard().get(&client_id).and_then(|r| r.wire.upgrade())
    }

    /// Live handles, sweeping dead slots on the way past.
    ///
    /// **Drop the result before sleeping.** Each element is a strong handle, and
    /// one held across a sleep keeps a finished connection's write fd open for a
    /// tick -- which is the one way this design could still strand something.
    pub fn live(&self) -> Vec<Arc<Mutex<Wire>>> {
        let mut v = self.guard();
        v.retain(|_, r| r.wire.strong_count() > 0);
        let mut rows: Vec<&Row> = v.values().collect();
        rows.sort_by_key(|r| r.seq);
        rows.into_iter().filter_map(|r| r.wire.upgrade()).collect()
    }

    /// The live handles of one writer shard: connections whose `client_id`
    /// is `shard` modulo `of`. A connection stays in one shard for life, so
    /// one thread writes it and its frames keep their order.
    pub fn live_shard(&self, shard: usize, of: usize) -> Vec<Arc<Mutex<Wire>>> {
        let of = of.max(1) as i64;
        let mut v = self.guard();
        v.retain(|_, r| r.wire.strong_count() > 0);
        let mut rows: Vec<&Row> = v
            .iter()
            .filter(|(id, _)| id.rem_euclid(of) == shard as i64)
            .map(|(_, r)| r)
            .collect();
        rows.sort_by_key(|r| r.seq);
        rows.into_iter().filter_map(|r| r.wire.upgrade()).collect()
    }

    /// `lock`, recovering from poisoning -- the opposite policy from
    /// `conn::guard`, and for the opposite reason. There is no invariant here:
    /// the value is a list of weak handles and every operation on it is total.
    /// The alternative is that one panicking connection kills the push path for
    /// every other player.
    fn guard(&self) -> MutexGuard<'_, HashMap<i64, Row>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// How often the clock wakes.
///
/// Destiny runs at 1 Hz (`docs/archive/destiny-and-the-ballpark.md` §4.2). A clock that
/// wakes at the rate it must emit at has no slack, so this is a divisor: a frame
/// due at *t* goes out within 250 ms of *t* rather than within a full second of
/// it.
pub const TICK: Duration = Duration::from_millis(250);

/// How long a writer thread spends on one connection per pass. **[A]** Long
/// enough for a healthy peer to take a big frame, short enough that a shard
/// of 8 stalled peers costs about a second, not four minutes. Falsified by a
/// `[push] write failed` from a healthy client, or a shard pass over a tick.
pub const WRITE_BUDGET: Duration = Duration::from_millis(150);

/// A peer that accepts no byte for this long is hung up; the outbox cap
/// ([`OUTBOX_FRAMES`], [`OUTBOX_BYTES`]) still catches one that is merely
/// slow. **[A]** Ten seconds: a laptop that sleeps for less recovers.
pub const STALL_LIMIT: Duration = Duration::from_secs(10);

/// Most writer threads the default asks for.
pub const MAX_DEFAULT_WRITERS: usize = 8;

/// Writer threads when `--writers` is not given: a quarter of the cores,
/// between 1 and [`MAX_DEFAULT_WRITERS`].
pub fn default_writers() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    (cores / 4).clamp(1, MAX_DEFAULT_WRITERS)
}

/// How late a tick may be before it is counted against us.
///
/// EvEJS's bot harness asserts on tick lateness with a default ceiling of
/// 1000 ms for a 10 Hz world **[V2]**; ours wakes four times a second and does
/// far less per wake, so the budget is tighter. The number is a tripwire, not
/// a deadline: nothing breaks at 101 ms, but a clock that is routinely over
/// budget is a clock whose scheduled batches are drifting.
const LATE_BUDGET: Duration = Duration::from_millis(100);

/// How many ticks between health lines. 40 ticks is ten seconds at [`TICK`].
///
/// **Ten seconds, because a minute measures the wrong thing.** This was 240
/// ticks for the steady state with an early first report at 40, which looked
/// like the considerate choice and meant every `client-ci` run emitted exactly
/// ONE line, covering its first ten seconds -- and the client takes 60 to 90
/// seconds to launch, so that window is entirely before it connects. Six
/// scenario runs all reported `late mean 0.1 ms max 0.1 ms, over budget 0`,
/// identically, because all six were measuring the same idle server. An
/// instrument that only samples the period when nothing is happening is worse
/// than none: it reads healthy by construction.
///
/// A flat ten seconds gives a run several samples across the part that
/// matters, and six lines a minute is affordable for a server that lives for
/// one scenario and keeps its world in memory.
const REPORT_EVERY: u64 = 40;

// **Compile-time, not a test.** Two things have gone wrong with this interval
// already -- first it was so coarse that CI runs got no line at all, then it
// was two-tiered in a way that gave them exactly one line covering the idle
// window before the client connected. Both were "the number is wrong", so the
// number is guarded where it cannot be ignored. A runtime test asserting a
// property of a constant is also what clippy's `assertions_on_constants`
// exists to catch, and it was right.
const _: () = assert!(
    TICK.as_millis() * REPORT_EVERY as u128 <= 15_000,
    "a client takes 60-90 s to launch, so the health line has to sample more \
     often than that or it only ever measures an idle server"
);
const _: () = assert!(
    TICK.as_millis() * REPORT_EVERY as u128 >= 2_000,
    "and not so often that the log is mostly health lines"
);

/// What the clock thread knows about its own punctuality.
///
/// **Lateness is the number that matters, not duration**, and that is a lesson
/// taken from someone else's outage rather than ours. A third-party EvEJS fork
/// chased a world-thread stall with a profiler that measured only the inside
/// of its tick; the profiler reported 1.0-1.3 ms per tick throughout and said
/// nothing at all, because the 546 ms was being spent in handlers *between*
/// ticks (`doc/TICK_STALL_SCENE_LOAD.md` in that repo) **[V2]**. Tick duration
/// cannot see time spent outside the tick. The gap between when a tick was due
/// and when it actually ran can.
///
/// So this records four things per wake and the worst of each:
///
/// - **late**: actual start minus the deadline. Catches anything that delays
///   the thread, including work done by somebody else.
/// - **lock_wait**: from asking for the world write lock to getting it.
///   Another thread's turn, felt by the clock.
/// - **lock_held**: how long the clock held the world write lock, measured
///   **from acquisition** (CP15, V3 F3: it used to start before the
///   `write()`, so a 300 ms wait read as a 300 ms hold while the stages took
///   4.6 ms). Our analogue of their main-thread handler, and the one number
///   another thread can feel.
/// - **work**: the whole iteration. Lets `late` be attributed to us or not.
///
/// Plus each `tick::STAGES` entry's mean and max, so "the clock is slow"
/// comes with a name (C7 §5.2).
///
/// Counters only -- no percentiles. A histogram would need buckets we would
/// have to guess at, and `max` plus `over_budget` answers the question a
/// percentile is usually a proxy for.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct TickHealth {
    pub ticks: u64,
    pub late_max: Duration,
    pub late_total: Duration,
    pub over_budget: u64,
    pub wait_max: Duration,
    pub wait_total: Duration,
    pub held_max: Duration,
    pub held_total: Duration,
    /// The longest single slice of the world lock this window (PA-3): the
    /// most a payload turn waited behind the clock. Equal to `held_max`
    /// when the clock runs unsliced.
    pub slice_max: Duration,
    pub work_max: Duration,
    /// Every tick's whole iteration this window, for the p95 the bubble
    /// planner sheds on (`bot_control::BubbleState`).
    pub work_samples: Vec<Duration>,
    /// `(stage, max, total)`, in the order the stages first reported.
    pub stages: Vec<(&'static str, Duration, Duration)>,
    /// Deadlines abandoned because the clock fell more than one period behind.
    ///
    /// Caught up to *now* rather than firing a burst of back-to-back ticks:
    /// a clock that tries to repay its debt all at once turns one stall into a
    /// thundering herd on the world lock.
    pub skipped: u64,
}

impl TickHealth {
    fn observe(
        &mut self,
        late: Duration,
        lock_wait: Duration,
        lock_held: Duration,
        work: Duration,
    ) {
        self.ticks += 1;
        self.late_total += late;
        self.late_max = self.late_max.max(late);
        self.wait_total += lock_wait;
        self.wait_max = self.wait_max.max(lock_wait);
        self.held_total += lock_held;
        self.held_max = self.held_max.max(lock_held);
        self.work_max = self.work_max.max(work);
        self.work_samples.push(work);
        if late > LATE_BUDGET {
            self.over_budget += 1;
        }
    }

    /// One tick's longest lock slice.
    fn observe_slice(&mut self, longest: Duration) {
        self.slice_max = self.slice_max.max(longest);
    }

    /// Keep one tick's stage timings (`tick::run_all`'s return).
    fn observe_stages(&mut self, timings: &[(&'static str, Duration)]) {
        for &(name, took) in timings {
            match self.stages.iter_mut().find(|(n, _, _)| *n == name) {
                Some((_, max, total)) => {
                    *max = (*max).max(took);
                    *total += took;
                }
                None => self.stages.push((name, took, took)),
            }
        }
    }

    /// `total / ticks`, saturating to zero.
    fn mean(&self, total: Duration) -> Duration {
        match u32::try_from(self.ticks) {
            Ok(n) if n > 0 => total / n,
            // Unreachable at REPORT_EVERY, and saturating beats a panic in the
            // one thread every player's arrival from warp goes through.
            _ => Duration::ZERO,
        }
    }

    fn late_mean(&self) -> Duration {
        self.mean(self.late_total)
    }

    /// The second line: every stage's mean and max, each with its unit.
    fn stages_report(&self) -> String {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let parts: Vec<String> = self
            .stages
            .iter()
            .map(|(name, max, total)| {
                format!(
                    "{name} mean {:.2} ms max {:.2} ms",
                    ms(self.mean(*total)),
                    ms(*max)
                )
            })
            .collect();
        format!(
            "[world] tick stages: {}{}",
            parts.join(", "),
            crate::clock_probe::take_clock_sched_suffix()
        )
    }

    /// One line, greppable, with the units in it.
    ///
    /// Written for `tools/client-ci.sh`, which reads `eve-server.log` and has
    /// no way to scrape anything. It parses `max N ms, over budget` and
    /// `over budget N`, so the front of the line keeps that shape.
    fn report(&self) -> String {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        format!(
            "[world] tick health: {} ticks, late mean {:.1} ms max {:.1} ms, \
over budget {}, world lock wait mean {:.1} ms max {:.1} ms, \
world lock held mean {:.1} ms max {:.1} ms, work max {:.1} ms, skipped {}, \
work p95 {:.1} ms, out {} frames {} bytes, destiny {} frames {} bytes \
parts {} merged {}, slice_max {:.1} ms",
            self.ticks,
            ms(self.late_mean()),
            ms(self.late_max),
            self.over_budget,
            ms(self.mean(self.wait_total)),
            ms(self.wait_max),
            ms(self.mean(self.held_total)),
            ms(self.held_max),
            ms(self.work_max),
            self.skipped,
            ms(crate::bot_control::p95(&self.work_samples)),
            POST_FRAMES.swap(0, Ordering::Relaxed),
            POST_BYTES.swap(0, Ordering::Relaxed),
            DESTINY_FRAMES.swap(0, Ordering::Relaxed),
            DESTINY_BYTES.swap(0, Ordering::Relaxed),
            DESTINY_PARTS.swap(0, Ordering::Relaxed),
            DESTINY_MERGED.swap(0, Ordering::Relaxed),
            ms(self.slice_max),
        )
    }
}

/// One pass of one shard: flush everything due on each of its connections.
/// Public to the crate so tests drive it without a thread.
pub(crate) fn sweep_shard(writers: &Writers, shard: usize, of: usize, now: Instant) {
    // `live_shard`'s strong handles are dropped at the end of the pass, which
    // is before the next `sleep`. PA-0's `WriterPass` is per shard: its count
    // is passes (shards x ticks), its max the slowest shard pass.
    let pass_started = Instant::now();
    for wire in writers.live_shard(shard, of) {
        // `try_lock`, not `lock`: a connection thread inside `send_all` holds
        // the wire, and skipping one tick for it is the cheapest failure
        // available (the client extrapolates between Destiny updates). A
        // poisoned `Wire` lands here as a permanent skip, which is right: that
        // stream may be mid-frame.
        let Ok(mut w) = wire.try_lock() else { continue };
        if let Err(e) = w.flush_due(now) {
            // One connection's problem. Its own thread will meet the same
            // error on its next read and end there.
            println!("[push] write failed: {e}");
        }
    }
    crate::live_status::prof::add(
        crate::live_status::prof::Sec::WriterPass,
        pass_started.elapsed(),
    );
}

/// Start `n` writer threads (`push-writer-K`), each owning the connections
/// with `client_id % n == K`. The threads live as long as the process.
///
/// **What changed from the single `push-ticker`.** It did every `write_all`
/// and every deflate, with a 30 s timeout, so one live-but-stalled peer held
/// every other client's pushes. Now a write is bounded by [`WRITE_BUDGET`]
/// with the unsent tail kept in the `Wire` ([`Wire::flush_due`]), shards run
/// in parallel, and a stalled peer costs its own shard one budget per tick
/// until [`STALL_LIMIT`] hangs it up.
///
/// **Still one clock.** Every shard wakes on the same [`TICK`] cadence and a
/// frame's due time was fixed when it was posted; the Destiny stamp is bound
/// before the post, so more threads are not more clocks. Per-connection order
/// holds because a connection belongs to exactly one shard.
pub fn spawn_writers(writers: Arc<Writers>, n: usize) -> io::Result<()> {
    let n = n.max(1);
    let shards = (0..n)
        .map(|_| Shard {
            thread: OnceLock::new(),
            dirty: Mutex::new(Vec::new()),
        })
        .collect();
    // A second pool on one registry would not be reachable from `wake`.
    let _ = writers.shards.set(shards);
    for shard in 0..n {
        let writers = Arc::clone(&writers);
        std::thread::Builder::new()
            .name(format!("push-writer-{shard}"))
            .spawn(move || {
                let mut shown = (0, 0);
                let mut ticks: u64 = 0;
                let mine = &writers.shards.get().expect("set above")[shard];
                let _ = mine.thread.set(std::thread::current());
                let mut carry: Vec<Arc<dyn Drain>> = Vec::new();
                let mut next_tick = Instant::now() + TICK;
                loop {
                    // Chat connections first: they are woken on demand, not
                    // on the tick, so a message is not held for a quarter
                    // second. A connection that could not finish comes back
                    // soon, and the shard polls until none is left.
                    let mut ready = std::mem::take(&mut carry);
                    ready.append(&mut mine.dirty.lock().unwrap_or_else(|e| e.into_inner()));
                    for conn in ready {
                        if conn.drain(Instant::now()) == Drained::More {
                            carry.push(conn);
                        }
                    }
                    let now = Instant::now();
                    if now < next_tick {
                        let wait = if carry.is_empty() {
                            next_tick - now
                        } else {
                            CHAT_RETRY.min(next_tick - now)
                        };
                        std::thread::park_timeout(wait);
                        continue;
                    }
                    ticks += 1;
                    if shard == 0 && ticks.is_multiple_of(REPORT_EVERY) {
                        let peak = (
                            PEAK_FRAMES.load(Ordering::Relaxed),
                            PEAK_BYTES.load(Ordering::Relaxed),
                        );
                        if peak != shown {
                            println!("{}", outbox_peak_line(peak.0, peak.1));
                            shown = peak;
                        }
                    }
                    sweep_shard(&writers, shard, n, Instant::now());
                    next_tick = Instant::now() + TICK;
                }
            })?;
    }
    Ok(())
}

/// How soon a shard calls a chat connection back when the socket took only
/// part of its queue. [A] short enough that a reader catching up is served
/// promptly, long enough not to spin; falsified by a shard pass at 100% CPU
/// with one slow chat peer.
const CHAT_RETRY: Duration = Duration::from_millis(10);

/// The outbox high-water line, printed when it rises. Greppable for a soak
/// or a client run: how close any connection came to [`OUTBOX_FRAMES`] and
/// [`OUTBOX_BYTES`].
fn outbox_peak_line(frames: usize, bytes: usize) -> String {
    format!(
        "[push] outbox peak {frames} frames {bytes} bytes \
(cap {OUTBOX_FRAMES} frames {OUTBOX_BYTES} bytes)"
    )
}

/// Start the clock that lets the **world** speak first.
///
/// **This is the mechanism three features were waiting on.** Until it existed,
/// `Registry::take_pending` was called from exactly one place -- `dispatch`'s
/// payload turn -- so nothing the server decided on its own schedule could
/// reach a client that was sitting still. `docs/port-tracker.md` §30.2 lists
/// what wanted it: a warp's arrival reconciliation, a lock dropped when its
/// target leaves range, and the premise of this project's end goal, a bot that
/// acts while every human is idle.
///
/// ## Why a second ticker rather than work inside [`spawn_writers`]
///
/// [`spawn_writers`] takes **no lock but the wire's**, by design, and that is
/// what makes it safe to run while a connection holds the world: its comment
/// about `try_lock` and skipped ticks rests on it. This one must take the
/// world lock, so it is a separate thread with a separate failure mode, and
/// keeping them apart means the existing guarantee is not quietly weakened by
/// an edit to a loop that used to be six lines.
///
/// ## The lock discipline, which is the whole risk
///
/// Process-wide order is **world > writers > wire** (see this module's
/// header), and the rule from `docs/reference/world-state.md` §4 is that a socket write
/// never happens under a lock -- one blocked peer would stall the world for
/// everybody.
///
/// So this loop is in two halves and they do not overlap:
///
/// 1. take the world write lock, `take_due`, **resolve each audience to
///    client ids while still holding it**, drop the guard;
/// 2. with no world lock at all, build each frame and `post` it to the
///    matching wire.
///
/// Step 1 resolves the audience because the session table lives in the world,
/// and a second lock acquisition to look it up is exactly the kind of
/// re-entry that turns into a cycle. Step 2 only ever `post`s -- an in-memory
/// enqueue -- so the actual `write_all` happens on [`spawn_writers`]'s thread
/// within one [`TICK`], and no thread in this design holds the world and a
/// socket at once.
///
/// ## What it needs, and what it deliberately does not have
///
/// Only the node id, because that is all `MachoPacket::notification` needs
/// besides the client id -- `dispatch`'s own `notify_to` is the same four
/// lines. **There is no session here**, which is the point: a push from the
/// clock is not a reply to anybody, so there is no acting connection to
/// address it to and no `audience() == None` case to serve. A scheduled push
/// with no audience has nobody to go to and is dropped with a line, because a
/// notification that silently goes nowhere is the failure this whole path
/// exists to remove.
/// `(client_id, session_id, change)`: one session change for one connection.
pub(crate) type DueSessionChange = (i64, i64, Vec<(PyValue, PyValue)>);

/// Every queued session change, addressed to each connection of its
/// character as `(client_id, session_id, change)`.
///
/// Drains both queues -- `Registry::take_session_changes` and
/// `Fleets::take_session_changes` -- the same two `dispatch.rs` drains after
/// a payload. See the comment at the call in [`spawn_world_ticker`] for why
/// the clock drains them too and why that is safe.
pub(crate) fn session_changes_due(w: &mut crate::service::Registry) -> Vec<DueSessionChange> {
    let mut queued = w.take_session_changes();
    queued.extend(
        w.fleets_mut()
            .take_session_changes()
            .into_iter()
            .map(|state| {
                (
                    state.character_id,
                    crate::fleet::Fleets::session_change(state),
                )
            }),
    );
    let mut out = Vec::new();
    for (character_id, change) in queued {
        for entry in w.sessions().of_character(character_id) {
            out.push((entry.client_id, entry.session_id, change.clone()));
        }
    }
    out
}

/// How often the clock saves a dirty world (V3 R5, CP15, CP52).
///
/// **[A]** five seconds (C7 §5.4, owner decision O1). Since CP52 this is the
/// **only** periodic save: payload turns mark the world dirty and leave it
/// here, so RAM may lead disk by up to this much and a kill loses at most
/// that (`docs/reference/world-state.md` §4 rule 3, as revised). The save renders the
/// snapshot under the world lock and the store (`crate::store::Saver`)
/// writes it on its own thread, through IP11's tainted and downgrade
/// diverts. Falsified by a kill losing more than a player accepts (shorten
/// it), or by `[turns]` `save_max` (now the render, under the lock) at a
/// size friends feel every 5 s (then the ledger log, E4).
pub const SAVE_EVERY: Duration = Duration::from_secs(5);

/// One due push and the connections it goes to, as the clock's drain
/// leaves it.
///
/// **The push itself, not its arguments** (lane perf-fight): the drain used
/// to call `to_push` and deep-clone the arguments once per recipient, under
/// the world lock -- a belt's `AddBalls2` to thirty pilots was thirty copies
/// of the belt. `to_push` reads only the push, so it runs after the lock,
/// once, and [`encode_frames`] pickles the arguments once for every
/// recipient.
pub(crate) struct Outgoing {
    pub push: Box<dyn crate::notify::Push>,
    /// Client ids, in the order the frames go out.
    pub clients: Vec<i64>,
}

/// What one clock turn produced under the world lock (lane perf-fight: the
/// ticker's first half, a function so `fight_bench` times the same code).
#[derive(Default)]
pub(crate) struct ClockTurn {
    pub timings: Vec<(&'static str, Duration)>,
    /// Locks dropped out of range this turn.
    pub dropped: usize,
    pub shutdown_due: crate::shutdown::Due,
    pub frames: Vec<Outgoing>,
    /// Pushes scheduled with no audience, named in a log line after the
    /// lock.
    pub orphans: Vec<Box<dyn crate::notify::Push>>,
    pub changes: Vec<DueSessionChange>,
}

/// **The clock's work under the world lock**, in its order: the
/// out-of-range sweep, every stage, the shutdown countdown, the drain
/// resolved to client ids, and the session changes.
///
/// This is the **unsliced** path (one `&mut Registry` for the whole tick):
/// the oracle the golden test compares [`clock_slices`] with, and what
/// `fight_bench` times. Both run the same [`clock_slice`] bodies.
#[cfg(test)]
pub(crate) fn clock_turn(
    w: &mut crate::service::Registry,
    now: Instant,
    ft: crate::damage::BlueTime,
) -> ClockTurn {
    let mut turn = ClockTurn::default();
    let ranges = crate::tick::slice_ranges(&crate::module::plan().stages);
    let last = ranges.len() - 1;
    for (i, range) in ranges.into_iter().enumerate() {
        clock_slice(w, now, ft, range, i == 0, i == last, &mut turn);
    }
    turn
}

/// **One lock slice of the clock** (PA-3): the stages of `range`, and when
/// `first` the out-of-range sweep before them, when `last` the shutdown
/// countdown, the drain and the session changes after them. `turn` is the
/// scratch that crosses slice boundaries: the timings so far, the sweep's
/// drop count, and (from the last slice) the drain. Nothing else is carried;
/// see `tick::SLICE_STARTS` for what each stage hands the next through the
/// world.
fn clock_slice(
    w: &mut crate::service::Registry,
    now: Instant,
    ft: crate::damage::BlueTime,
    range: std::ops::Range<usize>,
    first: bool,
    last: bool,
    turn: &mut ClockTurn,
) {
    // **Everything scheduled from here to the drain counts from this tick's
    // `now`**, the one `take_due` drains with (#136): a zero-delay push made
    // by a stage is due in this tick, not one `TICK` on.
    w.set_tick_now(Some(now));
    if first {
        // **The clock's own work, before it drains.** A lock
        // whose target has left range is dropped here because
        // nobody is going to ask: `sweep_out_of_range` schedules
        // its notifications, so they are taken by the very same
        // `take_due` below and leave in this tick rather than the
        // next. See `targeting::sweep_out_of_range`.
        let started = Instant::now();
        let swept = crate::targeting::sweep_out_of_range_stats(w);
        turn.dropped = swept.dropped;
        crate::clock_probe::note_sweep(&swept, started.elapsed());
        turn.timings.push(("sweep", started.elapsed()));
    }
    // **Every subsystem's clock work**, in dependency order:
    // modules, combat, mining, npc, jump, skill training
    // (`crate::tick::STAGES`, which says why each sits where
    // it does). All before `take_due` below, so what they
    // schedule leaves in this tick. The timings go into
    // `health` (CP15).
    turn.timings.extend(crate::tick::run_range(
        &crate::module::plan().stages,
        range,
        w,
        now,
        ft,
    ));
    if !last {
        w.set_tick_now(None);
        return;
    }
    // The `/shutdown` countdown and the stop file (lane
    // shutdown): a notice due now is scheduled before the
    // drain below; the stop itself is acted on after the
    // lock, because `Stop::request` dials the listeners.
    turn.shutdown_due = crate::shutdown::tick(w, ft);
    let drained = Instant::now();
    let due = w.take_due(now);
    w.set_tick_now(None);
    let audience_started = Instant::now();
    for push in due {
        let Some(audience) = push.audience() else {
            turn.orphans.push(push);
            continue;
        };
        let mut clients = Vec::new();
        for character_id in audience {
            for entry in w.sessions().of_character(character_id) {
                clients.push(entry.client_id);
            }
        }
        if !clients.is_empty() {
            turn.frames.push(Outgoing { push, clients });
        }
    }
    // **Session changes nobody's turn would ever carry.** Both
    // queues used to drain only in `dispatch.rs`, after a
    // payload -- right for `ship.Board`, whose change belongs
    // in front of its own reply, and never reached for one the
    // clock caused. A ship destroyed on this tick moves its
    // pilot into a capsule (`combat::kill`), and a member who
    // disconnects can hand the fleet a new boss
    // (`Fleets::character_disconnected`, P1.10): both victims
    // may be calling nothing at all. Draining here as well is
    // safe because a dispatch turn holds this same write lock
    // from the mutation through its own drain, so a change
    // queued inside a call can never be taken by the clock
    // before that call's reply is built.
    crate::live_status::prof::add(
        crate::live_status::prof::Sec::DrainAudience,
        audience_started.elapsed(),
    );
    let changes_started = Instant::now();
    turn.changes = session_changes_due(w);
    crate::live_status::prof::add(
        crate::live_status::prof::Sec::SessionChanges,
        changes_started.elapsed(),
    );
    // `sweep` and `drain` are not `tick::STAGES` rows; they are timed
    // beside them so the stages line accounts for the whole hold
    // (lane perf-fight).
    turn.timings.push(("drain", drained.elapsed()));
}

/// What the world lock cost one sliced tick (PA-3).
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SliceStats {
    /// Waiting for the lock, summed over the slices.
    pub wait: Duration,
    /// Holding it, summed over the slices: the tick's whole cost, which
    /// degradation judges (a sliced tick costs the world what an unsliced
    /// one did).
    pub held: Duration,
    /// The longest single hold: the most a payload turn can wait behind
    /// the clock.
    pub max: Duration,
}

/// The world's write lock, recovering a poisoned one. One panicking
/// connection must not stop every other player's ship arriving from warp.
/// It **taints** the world, as `dispatch.rs` does (IP9), so no save of ours
/// overwrites the good file (IP11).
fn lock_clock(
    world: &std::sync::RwLock<crate::service::Registry>,
) -> std::sync::RwLockWriteGuard<'_, crate::service::Registry> {
    let waiting = crate::clock_probe::Waiter::enter();
    let guard = world.write().unwrap_or_else(recover_poisoned);
    drop(waiting);
    guard
}

fn recover_poisoned<'a>(
    e: std::sync::PoisonError<std::sync::RwLockWriteGuard<'a, crate::service::Registry>>,
) -> std::sync::RwLockWriteGuard<'a, crate::service::Registry> {
    let mut w = e.into_inner();
    if !w.tainted() {
        eprintln!(
            "[world] TAINTED: the clock found the world lock \
             poisoned; serving on, but the world will not be \
             saved over the good file"
        );
    }
    w.taint();
    w
}

/// **The clock's priority at the world lock** (lane fight-stall-fix, step 3).
///
/// A 200-ship warp burst queues hundreds of short payload turns, and std's
/// `RwLock` has no queue discipline the clock can use: each of its seven
/// slices waited behind whoever was ahead of it and the tick skipped (the
/// 200v200 run: clock wait 2.7 s, payload hold mostly run-queue time).
/// While the gate is raised a payload turn that is about to ask for the world
/// lock ([`Self::wait_for_clock`]) waits instead, so the clock waits once, for
/// the turns already queued, and then runs its slices back to back.
///
/// **The clock raises it only when it has to wait** (`try_write` failed), and
/// lowers it after its last slice, so a calm world keeps today's interleaving
/// of turns between slices. **A dead clock cannot wedge the turns**: a waiter
/// gives up after [`GATE_CAP`] and proceeds. Off with `EVE_CLOCK_GATE=0`
/// (`server.rs` then builds none).
pub struct ClockGate {
    wants: std::sync::atomic::AtomicBool,
    m: Mutex<()>,
    cv: std::sync::Condvar,
    cap: Duration,
}

/// The longest a payload turn waits at a raised gate. **[A]** One tick: a
/// healthy clock lowers it within a few hundred ms; a turn that waits this
/// long goes on, so a wedged clock costs 250 ms a turn, not the server.
/// Falsified by a `capped` count above zero in `[world] tick lock`.
pub const GATE_CAP: Duration = TICK;

impl Default for ClockGate {
    fn default() -> Self {
        Self::with_cap(GATE_CAP)
    }
}

impl ClockGate {
    pub fn new() -> ClockGate {
        ClockGate::default()
    }

    /// A gate with its own cap, for a test that cannot wait [`GATE_CAP`].
    pub fn with_cap(cap: Duration) -> ClockGate {
        ClockGate {
            wants: std::sync::atomic::AtomicBool::new(false),
            m: Mutex::new(()),
            cv: std::sync::Condvar::new(),
            cap,
        }
    }

    /// Whether the clock has asked payload turns to hold back.
    pub fn raised(&self) -> bool {
        self.wants.load(Ordering::Acquire)
    }

    /// The clock is about to wait for the world lock: payload turns hold
    /// back until [`Self::lower`].
    pub fn raise(&self) {
        crate::clock_probe::note_gate_raise();
        self.wants.store(true, Ordering::Release);
    }

    /// The clock is done with this tick's slices.
    pub fn lower(&self) {
        {
            // Under the mutex, so a waiter that saw `wants` set is already
            // parked in `wait_timeout` (which releases it) and cannot miss
            // the notify.
            let _g = self.m.lock().unwrap_or_else(|e| e.into_inner());
            self.wants.store(false, Ordering::Release);
        }
        self.cv.notify_all();
    }

    /// A payload turn's call before it asks for the world lock: returns at
    /// once while the gate is down, otherwise when it is lowered or after
    /// the cap.
    pub fn wait_for_clock(&self) {
        if !self.raised() {
            return;
        }
        let started = Instant::now();
        let mut g = self.m.lock().unwrap_or_else(|e| e.into_inner());
        let mut capped = false;
        while self.wants.load(Ordering::Acquire) {
            let Some(left) = self
                .cap
                .checked_sub(started.elapsed())
                .filter(|d| !d.is_zero())
            else {
                capped = true;
                break;
            };
            g = self
                .cv
                .wait_timeout(g, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        drop(g);
        crate::clock_probe::note_gate_wait(started.elapsed(), capped);
    }
}

/// Lowers the gate when dropped, so a clock that panics mid-tick does not
/// leave it up for [`GATE_CAP`] a turn.
struct Lowers<'a>(Option<&'a ClockGate>);

impl Drop for Lowers<'_> {
    fn drop(&mut self) {
        if let Some(g) = self.0 {
            g.lower();
        }
    }
}

/// **The clock in lock slices** (PA-3): [`clock_turn`]'s work, taking the
/// world write lock once per slice (`tick::SLICE_STARTS`) instead of once for
/// the tick, so a payload turn waits for the longest slice, not for the
/// whole tick. The tick's `ft` is read after the first acquire (as the
/// ticker always did) and kept for every slice.
///
/// `finish` runs inside the **last** slice's guard, after the drain: the
/// ticker's save and report work, which belongs with the drain it follows.
/// It is given the pending-queue length read at the top of the first slice
/// (what the clock's own pushes are measured against, `[turn-tail]`'s
/// `clock_left`). `ft_fixed` is for tests (a golden run needs both paths on
/// the same blue time); the ticker passes `None`.
/// With `sliced == false` the whole tick runs under one guard (the oracle,
/// and the `--no-slices` escape hatch): identical output, one long hold.
#[cfg(test)]
pub(crate) fn clock_slices<R>(
    world: &std::sync::RwLock<crate::service::Registry>,
    now: Instant,
    ft_fixed: Option<crate::damage::BlueTime>,
    sliced: bool,
    finish: impl FnOnce(&mut crate::service::Registry, crate::damage::BlueTime, usize) -> R,
) -> (ClockTurn, R, SliceStats) {
    clock_slices_gated(world, now, ft_fixed, sliced, None, finish)
}

/// [`clock_slices`] with the clock's priority at the lock ([`ClockGate`]).
pub(crate) fn clock_slices_gated<R>(
    world: &std::sync::RwLock<crate::service::Registry>,
    now: Instant,
    ft_fixed: Option<crate::damage::BlueTime>,
    sliced: bool,
    gate: Option<&ClockGate>,
    finish: impl FnOnce(&mut crate::service::Registry, crate::damage::BlueTime, usize) -> R,
) -> (ClockTurn, R, SliceStats) {
    let ranges = if sliced {
        crate::tick::slice_ranges(&crate::module::plan().stages)
    } else {
        std::iter::once(0..crate::module::plan().stages.len()).collect()
    };
    let n = ranges.len();
    let mut turn = ClockTurn::default();
    let mut stats = SliceStats::default();
    let mut ft = 0;
    let mut pending0 = 0;
    let mut finish = Some(finish);
    let mut out = None;
    let mut released = now;
    // Declared before the loop: dropped after the last slice's guard.
    let mut lowers = Lowers(None);
    for (i, range) in ranges.into_iter().enumerate() {
        let mut w = match gate {
            // Raised already this tick: queue for the lock like a turn.
            Some(_) if lowers.0.is_some() => lock_clock(world),
            // Calm: the lock is free or not. Only a clock that would wait
            // asks the turns to hold back.
            Some(g) => match world.try_write() {
                Ok(w) => w,
                Err(std::sync::TryLockError::Poisoned(e)) => recover_poisoned(e),
                Err(std::sync::TryLockError::WouldBlock) => {
                    g.raise();
                    lowers.0 = Some(g);
                    lock_clock(world)
                }
            },
            None => lock_clock(world),
        };
        // Held is measured from here, not from `now`: the gap is somebody
        // else's turn (V3 F3).
        let acquired = Instant::now();
        let waited = acquired.saturating_duration_since(released);
        stats.wait += waited;
        crate::clock_probe::note_slice_wait(i, waited);
        let sched0 = crate::sched_stat::sample();
        if i == 0 {
            ft = ft_fixed.unwrap_or_else(crate::pyutil::filetime_now);
            pending0 = w.pending_len();
        }
        clock_slice(&mut w, now, ft, range, i == 0, i + 1 == n, &mut turn);
        if i + 1 == n
            && let Some(f) = finish.take()
        {
            out = Some(f(&mut w, ft, pending0));
        }
        if let (Some(a), Some(b)) = (sched0, crate::sched_stat::sample()) {
            let (cpu, runq) = b.since(a);
            crate::clock_probe::note_clock_sched(cpu, runq);
        }
        let held = Instant::now().saturating_duration_since(acquired);
        stats.held += held;
        stats.max = stats.max.max(held);
        drop(w);
        released = Instant::now();
    }
    drop(lowers);
    let Some(out) = out else {
        unreachable!("the slice table is never empty");
    };
    (turn, out, stats)
}

/// **The clock's second half's encoding**, with no lock held: each push's
/// arguments built and pickled once, then addressed to each of its
/// connections as the `MachoPacket` notification that client reads --
/// byte for byte what `MachoPacket::notification` gives per recipient
/// (`eve_proto`'s and this module's tests). In the drain's order, so each
/// connection's frames keep theirs. A push that will not encode is dropped
/// with a line.
pub(crate) fn encode_frames(node_id: i64, frames: Vec<Outgoing>) -> Vec<(i64, Frame)> {
    encode_frames_with(node_id, &frames, crate::par::threads())
}

/// Frames in one tick's drain at or above which phase B runs on the kit and
/// compresses there. **[A]** Encode costs ~0.56 us a frame serial (fight, 500
/// pilots: 14.2 ms for 25.2k frames), so 2000 frames is ~1.1 ms, well past a
/// spawn. Falsified by `encode_bench`; never pin it in a test.
const ENCODE_PAR_MIN_FRAMES: usize = 2_000;

/// Pushes at or above which phase A runs on the kit. **[A]** Same falsifier.
const ENCODE_BODIES_PAR_MIN: usize = 64;

/// [`encode_frames`] with an explicit thread count. One thread (or a small
/// drain) is exactly the old serial path: `Frame::Raw` payloads, compressed by
/// the writers.
pub(crate) fn encode_frames_with(
    node_id: i64,
    frames: &[Outgoing],
    threads: usize,
) -> Vec<(i64, Frame)> {
    let bodies = encode_bodies(frames, threads, ENCODE_BODIES_PAR_MIN);
    encode_jobs_with(node_id, frames, &bodies, threads, ENCODE_PAR_MIN_FRAMES)
}

/// **Phase A**, once per push, in push order: the event and the pickled
/// arguments. `to_push` runs exactly once per push (a `RingUpdate` reads the
/// wall clock there), also when a worker panics: the serial retry below only
/// runs after a panic, which the old code would have propagated anyway.
pub(crate) fn encode_bodies(
    frames: &[Outgoing],
    threads: usize,
    min_pushes: usize,
) -> Vec<(&'static str, PyValue)> {
    let one = |o: &Outgoing| {
        let (event, args) = o.push.to_push();
        (event, eve_proto::MachoPacket::notification_body(args))
    };
    crate::par::par_map_with(threads, frames, min_pushes, one)
        .unwrap_or_else(|_| frames.iter().map(one).collect())
}

/// **Phase B**: each push addressed to each of its clients, in the drain's
/// order, over contiguous chunks of frames (so a 500-client push is split
/// across workers). Compression is decided here only when it runs parallel
/// (`threads > 1` and at least `min_frames` frames); otherwise the frames are
/// `Raw` and the writers compress, as before. A pure function of
/// `(node_id, bodies, audiences)`, so every thread count gives the same bytes.
pub(crate) fn encode_jobs_with(
    node_id: i64,
    frames: &[Outgoing],
    bodies: &[(&'static str, PyValue)],
    threads: usize,
    min_frames: usize,
) -> Vec<(i64, Frame)> {
    let jobs: Vec<(u32, i64)> = frames
        .iter()
        .enumerate()
        .flat_map(|(i, o)| o.clients.iter().map(move |&c| (i as u32, c)))
        .collect();
    let par = threads > 1 && jobs.len() >= min_frames;
    let one = |&(i, client_id): &(u32, i64)| {
        let (event, body) = &bodies[i as usize];
        address(node_id, event, body, client_id, par)
    };
    let chunk = |c: &[(u32, i64)]| c.iter().map(one).collect::<Vec<_>>();
    let parts = crate::par::par_chunks_with(threads, &jobs, min_frames, chunk)
        .unwrap_or_else(|_| vec![chunk(&jobs)]);
    let mut out = Vec::with_capacity(jobs.len());
    for r in parts.into_iter().flatten() {
        match r {
            Ok(f) => out.push(f),
            // After the join, so the lines keep the drain's order.
            Err(event) => println!("[world] {event} would not encode; dropped"),
        }
    }
    out
}

/// One pickled body addressed to one client as its frame; `Err` names the
/// event that would not encode. `deflate` decides the compression now (the
/// parallel phase) rather than in the writer.
fn address(
    node_id: i64,
    event: &'static str,
    body: &PyValue,
    client_id: i64,
    deflate: bool,
) -> Result<(i64, Frame), &'static str> {
    let Ok(payload) = eve_proto::MachoPacket::notification_pickled(
        eve_proto::MachoAddress::node(node_id),
        event,
        "*clientID",
        vec![PyValue::Int(client_id)],
        body.clone(),
    )
    .to_payload() else {
        return Err(event);
    };
    if event == "DoDestinyUpdate" {
        DESTINY_FRAMES.fetch_add(1, Ordering::Relaxed);
        DESTINY_BYTES.fetch_add(payload.len(), Ordering::Relaxed);
    }
    Ok((
        client_id,
        if deflate {
            Frame::encoded(payload)
        } else {
            Frame::Raw(payload)
        },
    ))
}

/// **The clock's frames with `--coalesce-destiny` on** (PA-10 §7.5): what
/// [`encode_frames`] gives, except a push [`destiny_part`] accepts becomes a
/// [`Frame::Part`] per client (an `Arc` clone, no per-client encode), for
/// [`Writers::post_part`] to merge into the connection's outbox tail. Every
/// other push is encoded as ever, in the same drain order. The writers do the
/// part encode off the clock. Runs `to_push` exactly once per push.
pub(crate) fn stage_frames(node_id: i64, frames: Vec<Outgoing>) -> Vec<(i64, Frame)> {
    stage_frames_with(node_id, &frames, crate::par::threads())
}

enum Staged {
    Part(PartSrc),
    Body(&'static str, PyValue),
}

pub(crate) fn stage_frames_with(
    node_id: i64,
    frames: &[Outgoing],
    threads: usize,
) -> Vec<(i64, Frame)> {
    let one = |o: &Outgoing| {
        let (event, args) = o.push.to_push();
        match PartSrc::of(event, &args) {
            Some(src) => Staged::Part(src),
            None => Staged::Body(event, eve_proto::MachoPacket::notification_body(args)),
        }
    };
    let staged: Vec<Staged> = crate::par::par_map_with(threads, frames, ENCODE_BODIES_PAR_MIN, one)
        .unwrap_or_else(|_| frames.iter().map(one).collect());
    let mut out = Vec::new();
    for (o, st) in frames.iter().zip(&staged) {
        for &client_id in &o.clients {
            match st {
                Staged::Part(src) => {
                    out.push((client_id, Frame::Part(src.for_client(node_id, client_id))));
                }
                Staged::Body(event, body) => {
                    match address(node_id, event, body, client_id, false) {
                        Ok(f) => out.push(f),
                        Err(event) => println!("[world] {event} would not encode; dropped"),
                    }
                }
            }
        }
    }
    out
}

/// `spawn_world_ticker` at [`SAVE_EVERY`]. `status` is `--status FILE`:
/// rewritten every report when set.
#[cfg(test)]
pub fn spawn_world_ticker(
    world: Arc<std::sync::RwLock<crate::service::Registry>>,
    writers: Arc<Writers>,
    node_id: i64,
    store: Arc<dyn crate::store::Store>,
    status: Option<std::path::PathBuf>,
) -> io::Result<()> {
    spawn_world_ticker_gated(world, writers, node_id, store, status, None)
}

/// [`spawn_world_ticker`] with the clock's priority at the world lock
/// ([`ClockGate`]); `server.rs` builds it unless `EVE_CLOCK_GATE=0`.
pub fn spawn_world_ticker_gated(
    world: Arc<std::sync::RwLock<crate::service::Registry>>,
    writers: Arc<Writers>,
    node_id: i64,
    store: Arc<dyn crate::store::Store>,
    status: Option<std::path::PathBuf>,
    gate: Option<Arc<ClockGate>>,
) -> io::Result<()> {
    spawn_world_ticker_every(world, writers, node_id, store, status, SAVE_EVERY, gate)
}

/// Whether the clock gets priority at the world lock ([`ClockGate`]). On
/// unless `EVE_CLOCK_GATE=0`.
pub fn clock_gate_on() -> bool {
    std::env::var("EVE_CLOCK_GATE").map_or(true, |v| v != "0")
}

/// Whether the clock takes the world lock a slice at a time (PA-3). On
/// unless `EVE_CLOCK_SLICES=0`: the escape hatch if a slice boundary turns
/// out to matter, with identical output either way.
fn clock_slices_on() -> bool {
    std::env::var("EVE_CLOCK_SLICES").map_or(true, |v| v != "0")
}

/// The ticker with its save interval as an argument, for a test that cannot
/// wait [`SAVE_EVERY`].
fn spawn_world_ticker_every(
    world: Arc<std::sync::RwLock<crate::service::Registry>>,
    writers: Arc<Writers>,
    node_id: i64,
    store: Arc<dyn crate::store::Store>,
    status: Option<std::path::PathBuf>,
    save_every: Duration,
    gate: Option<Arc<ClockGate>>,
) -> io::Result<()> {
    std::thread::Builder::new()
        .name("world-ticker".to_owned())
        .spawn(move || {
            // **A deadline, not a sleep.** `sleep(TICK)` makes the period
            // TICK *plus* however long the work took, so the clock drifts by
            // its own workload and "late" has nothing to be late against.
            // Against a deadline, lateness is measurable and the period holds.
            let mut due = Instant::now() + TICK;
            let mut health = TickHealth::default();
            let mut window_start = Instant::now();
            let mut last_save = Instant::now();
            // Reported with the health line rather than one line per drop: a
            // ship drifting at the boundary would otherwise flood the log,
            // and the count is the thing worth knowing.
            let mut out_of_range = 0usize;
            // BUBBLE: report windows so far, the file's `seq`.
            let mut windows = 0u64;
            let mut status_writer = crate::live_status::StatusWriter::default();
            // admin-web: ticks so far (its snapshot cadence) and the
            // listener, once a snapshot has named it.
            let mut admin_ticks = 0u64;
            let mut admin: Option<std::sync::Arc<crate::admin_web::Shared>> = None;
            let sliced = clock_slices_on();
            loop {
                if let Some(nap) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(nap);
                }
                let now = Instant::now();
                let late = now.saturating_duration_since(due);
                due += TICK;
                // More than a whole period behind: give up the missed
                // deadlines instead of firing them back to back.
                if due < now {
                    let behind = now.saturating_duration_since(due);
                    let missed = (behind.as_nanos() / TICK.as_nanos().max(1)) as u64 + 1;
                    health.skipped += missed;
                    due = now + TICK;
                }
                // This tick ends a report window: the `[turns]` and `[live]`
                // numbers and the status rows are taken under the lock.
                let reporting = health.ticks + 1 >= REPORT_EVERY;
                let mut turns = crate::live_status::TurnStats::default();
                let mut live = String::new();
                let mut status_body: Option<String> = None;
                // BUBBLE: where the humans are, taken under the lock; the
                // p95 joins it after the window's last tick is observed.
                let mut bubble: Option<(std::path::PathBuf, Vec<(i64, u32)>)> = None;
                // admin-web: the pilots online, every SNAPSHOT_EVERY ticks,
                // taken under the lock and published after it.
                admin_ticks += 1;
                let admin_due = admin_ticks.is_multiple_of(crate::admin_web::SNAPSHOT_EVERY);
                let mut admin_snap = None;
                // admin-web actions: run under the lock, answered after it.
                let mut admin_out: Vec<crate::admin_actions::Outcome> = Vec::new();
                // Half one: the world lock (a slice at a time, PA-3), and
                // nothing else inside it. The save and the report run inside
                // the last slice, after the drain.
                // The turn trace (TT-A): the clock's own logging, and the
                // pushes it leaves queued for the next payload turn. With
                // slices a payload turn can queue between them, so `left`
                // is an upper bound [A].
                crate::logline::take_thread_stats();
                let (turn, notices, slice_stats) = clock_slices_gated(
                    &world,
                    now,
                    None,
                    sliced,
                    gate.as_deref(),
                    |w, ft, pending0| {
                        // **The clock's save** (V3 R5, CP52): every `save_every`,
                        // if anything durable changed. The snapshot is rendered
                        // under this lock; the store writes it on its saver
                        // thread, through its own diverts. After the stages, so
                        // it carries what they just did.
                        if now.saturating_duration_since(last_save) >= save_every {
                            last_save = now;
                            if w.take_dirty() {
                                // Timed apart for `[save]` (E1): the clone and
                                // the render here, under the lock; the saver's
                                // write arrives with the report (below).
                                let start = Instant::now();
                                let snapshot = w.to_snapshot();
                                let cloned = start.elapsed();
                                crate::live_status::prof::add(
                                    crate::live_status::prof::Sec::Save,
                                    cloned,
                                );
                                match store.save_owned(snapshot) {
                                    Ok(cost) => crate::live_status::record_save_cost(
                                        w,
                                        crate::live_status::SaveCost {
                                            snapshot: cloned,
                                            ..cost
                                        },
                                    ),
                                    Err(e) => {
                                        eprintln!("[world] clock save failed: {e}");
                                        crate::live_status::record_save(w, start.elapsed());
                                    }
                                }
                            }
                        }
                        let left = w.pending_len().saturating_sub(pending0) as u32;
                        crate::live_status::record_clock_log(
                            w,
                            crate::logline::take_thread_stats(),
                            left,
                        );
                        if reporting {
                            // The window's worst hold so far feeds degradation
                            // (W14 CP40, C7 §5.3), with its slowest stage.
                            let slowest = health.stages.iter().max_by_key(|s| s.1).map(|s| s.0);
                            crate::liveness::note_report(w, health.held_max, slowest);
                            // The writes the saver finished this window (E1b),
                            // into the one `[save]` line.
                            crate::live_status::record_save_writes(w, store.take_writes());
                            turns = crate::live_status::take_turns(w);
                            live = crate::live_status::live_line(w, ft, &turns);
                            status_body =
                                status.as_ref().map(|_| crate::live_status::status_tsv(w));
                            bubble = crate::bot_control::bubble_path(w)
                                .map(|path| (path, crate::bot_control::bubble_humans(w)));
                        }
                        if admin_due {
                            admin_snap = crate::admin_web::take(w);
                        }
                        // The owner's admin actions (`POST /api/action/*`).
                        admin_out = crate::admin_actions::drain(w)
                            .into_iter()
                            .map(|o| match o {
                                crate::admin_actions::Outcome::Save(tx) => {
                                    // Save due now, as the clock's save does.
                                    w.take_dirty();
                                    let text = match store.save_owned(w.to_snapshot()) {
                                        Ok(_) => "saved".to_owned(),
                                        Err(e) => format!("save failed: {e}"),
                                    };
                                    crate::admin_actions::Outcome::Now(tx, text)
                                }
                                other => other,
                            })
                            .collect();
                        // GW-OUT: out under the lock, published after it.
                        crate::gateway_out::take(w)
                    },
                );
                // GW-OUT: the clock's notices, published with no guard held.
                crate::gateway_out::flush(notices);
                crate::admin_actions::finish(admin_out);
                if let Some((shared, snap)) = admin_snap {
                    shared.publish(snap);
                    admin = Some(shared);
                }
                let (lock_wait, lock_held) = (slice_stats.wait, slice_stats.held);
                crate::live_status::prof::add(crate::live_status::prof::Sec::ClockHeld, lock_held);
                let timings = turn.timings;
                out_of_range += turn.dropped;
                let shutdown_due = turn.shutdown_due;
                let frames = turn.frames;
                let orphans = turn.orphans;
                // `(client_id, session_id, change)`, built under the lock and
                // sent after it.
                let changes = turn.changes;
                for push in orphans {
                    let event = push.to_push().0;
                    println!("[world] {event} was scheduled with no audience; dropped");
                }
                shutdown_due.act();
                // Half two: no world lock held.
                let coalesce = coalesce_destiny();
                let outgoing = if coalesce {
                    stage_frames(node_id, frames)
                } else {
                    encode_frames(node_id, frames)
                };
                for (client_id, frame) in outgoing {
                    // `Writers::post`, not `wire_for(..).lock()`: the wire
                    // mutex is held across `write_all`, so one peer with a
                    // full receive window used to stall this whole tick
                    // (`docs/port-tracker.md` §57). A closed connection is
                    // refused quietly -- the player left between the schedule
                    // and its due time, which is normal.
                    writers.post_frame(client_id, now, frame);
                }
                for (client_id, session_id, change) in changes {
                    let Ok(payload) = eve_proto::MachoPacket::session_change(
                        eve_proto::MachoAddress::node(node_id),
                        eve_proto::MachoAddress::client(client_id),
                        session_id,
                        change,
                    )
                    .to_payload() else {
                        println!("[world] a session change would not encode; dropped");
                        continue;
                    };
                    writers.post(client_id, now, payload);
                }
                health.observe(
                    late,
                    lock_wait,
                    lock_held,
                    Instant::now().saturating_duration_since(now),
                );
                health.observe_slice(slice_stats.max);
                health.observe_stages(&timings);
                if reporting {
                    let window = now.saturating_duration_since(window_start);
                    window_start = now;
                    println!("{}", health.report());
                    println!("{}", health.stages_report());
                    println!("{}", crate::clock_probe::take_sweep_line());
                    println!("{}", turns.report(window));
                    println!("{}", turns.tail_report(window));
                    for line in turns.slow_lines() {
                        println!("{line}");
                    }
                    println!("{}", turns.save_report(window));
                    for line in crate::live_status::prof::report_lines(health.ticks, window) {
                        println!("{line}");
                    }
                    println!("{live}");
                    if let Some(shared) = &admin {
                        shared.note_health(crate::admin_web::Health {
                            unix: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |d| d.as_secs()),
                            ticks: health.ticks,
                            p95_ms: crate::bot_control::p95(&health.work_samples).as_secs_f64()
                                * 1000.0,
                            held_max_ms: health.held_max.as_secs_f64() * 1000.0,
                            skipped: health.skipped,
                        });
                    }
                    if let Some((path, humans)) = bubble {
                        windows += 1;
                        let ms =
                            crate::bot_control::p95(&health.work_samples).as_secs_f64() * 1000.0;
                        let state = crate::bot_control::BubbleState {
                            seq: windows,
                            unix_s: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |d| d.as_secs()),
                            p95_ms: ms,
                            humans,
                        };
                        if let Err(e) = crate::bot_control::write_bubble(&path, &state) {
                            println!("[bubble] {}: {e}", path.display());
                        }
                    }
                    if let (Some(path), Some(body)) = (&status, &status_body)
                        && let Some(e) = status_writer.write(path, body)
                    {
                        println!("[live] status file {}: {e}", path.display());
                    }
                    if out_of_range > 0 {
                        println!("[world] dropped {out_of_range} lock(s) out of range");
                        out_of_range = 0;
                    }
                    health = TickHealth::default();
                }
            }
        })
        .map(|_| ())
}

#[cfg(test)]
mod brawl_bench;
#[cfg(test)]
mod coalesce_tests;
#[cfg(test)]
mod fight_bench;

#[cfg(test)]
mod tests {
    use super::*;

    /// **The clock's encoding is byte for byte what it was** (lane
    /// perf-fight): pushes pickled once and addressed per connection give,
    /// for every connection and in the same order, the payload the drain
    /// built before -- `MachoPacket::notification` of `to_push` per
    /// recipient.
    #[test]
    fn the_clock_encodes_each_push_once_and_every_frame_as_before() {
        let fx = crate::modules::SpecialFx {
            to: vec![1, 2],
            ship_id: 100_000_001,
            module_id: 100_000_050,
            type_id: 990_001,
            target_id: Some(100_000_002),
            guid: "effects.Invented",
            is_offensive: true,
            start: true,
            duration_ms: 3_000,
            repeat: 1000,
        };
        let cap = crate::modules::AttributeChange {
            to: vec![1],
            owner_id: 1,
            item_id: 100_000_001,
            attribute_id: 9_001,
            time: 7,
            new_value: 12.5,
            old_value: 20.0,
        };
        let pushes: Vec<(Box<dyn crate::notify::Push>, Vec<i64>)> = vec![
            (Box::new(fx.clone()), vec![501, 502, 503]),
            (Box::new(cap.clone()), vec![501]),
        ];
        let mut before = Vec::new();
        for (push, clients) in &pushes {
            let (event, args) = push.to_push();
            for &client_id in clients {
                let payload = eve_proto::MachoPacket::notification(
                    eve_proto::MachoAddress::node(9),
                    event,
                    "*clientID",
                    vec![PyValue::Int(client_id)],
                    args.clone(),
                )
                .to_payload()
                .expect("encodes");
                before.push((client_id, payload));
            }
        }
        let frames = pushes
            .into_iter()
            .map(|(push, clients)| Outgoing { push, clients })
            .collect();
        let got: Vec<(i64, Vec<u8>)> = encode_frames(9, frames)
            .into_iter()
            .map(|(c, f)| (c, f.payload().to_vec()))
            .collect();
        assert_eq!(got, before);
    }

    #[test]
    fn tick_health_keeps_the_worst_of_each_and_the_mean_of_lateness() {
        let mut h = TickHealth::default();
        assert_eq!(
            h.late_mean(),
            Duration::ZERO,
            "no ticks is not a divide by zero"
        );
        h.observe(
            Duration::from_millis(10),
            Duration::ZERO,
            Duration::from_millis(5),
            Duration::from_millis(7),
        );
        h.observe(
            Duration::from_millis(30),
            Duration::ZERO,
            Duration::from_millis(1),
            Duration::from_millis(2),
        );
        assert_eq!(h.ticks, 2);
        assert_eq!(h.late_max, Duration::from_millis(30), "max, not last");
        assert_eq!(h.late_mean(), Duration::from_millis(20));
        assert_eq!(h.held_max, Duration::from_millis(5), "the worst lock hold");
        assert_eq!(h.work_max, Duration::from_millis(7));
        assert_eq!(h.over_budget, 0, "both samples are inside the budget");
    }

    #[test]
    fn only_a_tick_past_the_budget_counts_against_it() {
        let mut h = TickHealth::default();
        // Exactly at the budget is not over it: the comparison is strict so
        // that a budget of N means "N is fine", which is how a reader reads it.
        h.observe(LATE_BUDGET, Duration::ZERO, Duration::ZERO, Duration::ZERO);
        assert_eq!(h.over_budget, 0);
        h.observe(
            LATE_BUDGET + Duration::from_millis(1),
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
        );
        assert_eq!(h.over_budget, 1);
    }

    #[test]
    fn the_health_line_names_every_number_and_its_unit() {
        // The harness greps `eve-server.log` for this line, so the shape of it
        // is the interface. A number without a unit is not reportable.
        let mut h = TickHealth::default();
        h.observe(
            Duration::from_millis(12),
            Duration::from_millis(6),
            Duration::from_millis(3),
            Duration::from_millis(4),
        );
        h.skipped = 2;
        let line = h.report();
        assert!(
            line.starts_with("[world] tick health:"),
            "greppable prefix: {line}"
        );
        for needle in [
            "1 ticks",
            "late mean 12.0 ms",
            // `tools/client-ci.sh` parses these two shapes.
            "max 12.0 ms, over budget",
            "over budget 0",
            "world lock wait mean 6.0 ms max 6.0 ms",
            "world lock held mean 3.0 ms max 3.0 ms",
            "work max 4.0 ms",
            "skipped 2",
            // PA-10: the reduction is merged / parts.
            " bytes parts ",
            " merged ",
        ] {
            assert!(line.contains(needle), "{needle:?} missing from {line:?}");
        }
        assert!(!line.contains('\n'), "one line, so one grep: {line:?}");
    }

    /// **Wait and hold are separate numbers** (V3 F3): a tick that waited
    /// long and held briefly reports exactly that, not their sum.
    #[test]
    fn wait_and_hold_are_reported_separately() {
        let mut h = TickHealth::default();
        let (wait, held) = (Duration::from_millis(300), Duration::from_millis(5));
        h.observe(Duration::ZERO, wait, held, wait + held);
        assert_eq!((h.wait_max, h.held_max), (wait, held));
        let line = h.report();
        assert!(line.contains("wait mean 300.0 ms max 300.0 ms"), "{line}");
        assert!(line.contains("held mean 5.0 ms max 5.0 ms"), "{line}");
    }

    /// The stage line names every `tick::STAGES` entry with its unit, from
    /// what `run_all` really returns, and keeps the max and the mean.
    #[test]
    fn the_health_line_names_every_stage_and_its_unit() {
        let mut w = crate::service::Registry::new(crate::model::SandboxConfig::default());
        let mut h = TickHealth::default();
        let ran = crate::tick::run_all(&mut w, Instant::now(), crate::pyutil::filetime_now());
        h.observe(
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
        );
        h.observe_stages(&ran);
        let line = h.stages_report();
        assert!(line.starts_with("[world] tick stages: "), "{line}");
        assert!(!line.contains('\n'));
        for stage in &crate::module::plan().stages {
            let at = line
                .find(&format!("{} mean ", stage.name))
                .unwrap_or_else(|| panic!("{} missing from {line:?}", stage.name));
            let rest = &line[at..];
            let entry = rest.split(", ").next().unwrap_or(rest);
            assert_eq!(
                entry.matches(" ms").count(),
                2,
                "mean and max in ms: {entry:?}"
            );
        }
        // Max and mean over two ticks of one fake stage.
        let mut h = TickHealth::default();
        for took in [2, 6] {
            h.observe(
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
            );
            h.observe_stages(&[("fake", Duration::from_millis(took))]);
        }
        // The clock thread's own cpu / run-queue maxima ride at the end.
        let line = h.stages_report();
        assert!(
            line.starts_with("[world] tick stages: fake mean 4.00 ms max 6.00 ms"),
            "{line}"
        );
        assert!(line.contains("; clock "), "{line}");
    }

    /// A store that keeps every snapshot it is handed.
    #[derive(Default)]
    struct Kept(Mutex<Vec<crate::store::Snapshot>>);

    impl crate::store::Store for Kept {
        fn load(&self) -> io::Result<Option<crate::store::Snapshot>> {
            Ok(None)
        }
        fn save(&self, snapshot: &crate::store::Snapshot) -> io::Result<()> {
            self.0.lock().expect("kept").push(snapshot.clone());
            Ok(())
        }
    }

    /// **A clock-only mutation is saved with no payload** (V3 R5). The
    /// world changes with no connection and no turn (a new `world` record,
    /// the durable change a test can make without a stage's fixtures), and
    /// the clock alone carries it to the store; a world with nothing dirty
    /// is not saved again.
    #[test]
    fn a_clock_only_mutation_is_saved_with_no_payload() {
        let mut reg = crate::service::Registry::new(crate::model::SandboxConfig::default());
        reg.take_dirty();
        let minted = crate::delta::World::mint(crate::pyutil::filetime_now());
        reg.deltas_mut().set_world(minted);
        let world = Arc::new(std::sync::RwLock::new(reg));
        let kept = Arc::new(Kept::default());
        spawn_world_ticker_every(
            Arc::clone(&world),
            Arc::new(Writers::default()),
            1,
            Arc::clone(&kept) as Arc<dyn crate::store::Store>,
            None,
            TICK,
            None,
        )
        .expect("spawn world ticker");
        let deadline = Instant::now() + Duration::from_secs(20);
        while kept.0.lock().expect("kept").is_empty() && Instant::now() < deadline {
            std::thread::sleep(TICK);
        }
        // Several more periods: a clean world must not be saved again.
        std::thread::sleep(TICK * 4);
        let saves = kept.0.lock().expect("kept");
        assert_eq!(saves.len(), 1, "saved once, and only while dirty");
        assert_eq!(
            saves[0].deltas.world(),
            Some(&minted),
            "the snapshot carries the clock-only change"
        );
    }
    use eve_proto::try_split_frame;
    use std::io::Read;
    use std::net::{TcpListener, TcpStream};

    /// A connected pair of loopback sockets: (ours, the peer's).
    pub(super) fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local_addr");
        let ours = TcpStream::connect(addr).expect("connect");
        let (peer, _) = listener.accept().expect("accept");
        (ours, peer)
    }

    /// Read `n` bytes, or as many as arrive before the peer closes.
    pub(super) fn read_frames(mut peer: TcpStream, want: usize) -> Vec<Vec<u8>> {
        peer.set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        let mut buf = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        let mut frames = Vec::new();
        loop {
            while let Ok(Some((frame, consumed))) = try_split_frame(&buf) {
                frames.push(frame.payload.to_vec());
                buf.drain(..consumed);
            }
            if frames.len() >= want {
                return frames;
            }
            match peer.read(&mut chunk) {
                Ok(0) => return frames,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => return frames,
            }
        }
    }

    /// **The test this increment exists for.** A push must not be able to land
    /// in the middle of a reply's bytes.
    ///
    /// One thread writes a 100 KB payload over and over -- big enough that the
    /// kernel cannot take it in one `write` and `write_all` must loop -- while
    /// another pushes a small one. Read the stream back through the client's own
    /// splitter and assert every frame is byte-for-byte one of the two. This is
    /// the test that fails if the lock is byte-granular, or if the framed buffer
    /// were built *inside* the I/O rather than before it.
    #[test]
    fn a_push_cannot_interleave_a_reply() {
        let (ours, peer) = pair();
        let big = vec![0xABu8; 100 * 1024];
        let small = vec![0x5Au8; 64];
        let wire = Arc::new(Mutex::new(Wire::new(ours, None)));

        let reader = std::thread::spawn(move || read_frames(peer, 40));

        let mut writers = Vec::new();
        for payload in [big.clone(), small.clone()] {
            let wire = Arc::clone(&wire);
            writers.push(std::thread::spawn(move || {
                for _ in 0..20 {
                    let one = [payload.clone()];
                    if wire.lock().expect("wire").send_all(&one).is_err() {
                        return;
                    }
                }
            }));
        }
        for w in writers {
            w.join().expect("writer thread");
        }
        wire.lock().expect("wire").close();

        let frames = reader.join().expect("reader thread");
        assert!(frames.len() >= 40, "got {} frames", frames.len());
        for f in &frames {
            // A large payload leaves compressed (`compress_payload`).
            let f = &if eve_inflate::looks_like_zlib(f) {
                eve_inflate::inflate_zlib(f).expect("a whole zlib stream")
            } else {
                f.clone()
            };
            assert!(
                *f == big || *f == small,
                "a frame arrived that is neither payload: {} bytes starting {:02x?}",
                f.len(),
                &f[..8.min(f.len())]
            );
        }
    }

    /// Incompressible bytes, so a frame's size on the wire is its size here.
    pub(super) fn noise(len: usize, seed: u64) -> Vec<u8> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    }

    pub(super) fn registered(writers: &Writers, id: i64) -> (Arc<Mutex<Wire>>, TcpStream) {
        let (ours, peer) = pair();
        let wire = Arc::new(Mutex::new(Wire::new(ours, None)));
        writers.register(id, &wire);
        (wire, peer)
    }

    /// A push whose pickled body is `len` bytes of a repeating pattern (so it
    /// deflates) or noise (so it does not), with a fixed audience.
    #[derive(Debug)]
    struct Blob {
        len: usize,
        seed: u64,
        compressible: bool,
        to: Vec<i64>,
    }

    impl crate::notify::Push for Blob {
        fn to_push(&self) -> (&'static str, Vec<PyValue>) {
            let body = if self.compressible {
                (0..self.len).map(|i| (i % 7) as u8).collect()
            } else {
                noise(self.len, self.seed)
            };
            ("DoDestinyUpdate", vec![PyValue::Bytes(body)])
        }

        fn audience(&self) -> Option<Vec<i64>> {
            Some(self.to.clone())
        }
    }

    /// Deterministic drain: invented ids, audiences of 1-40 clients drawn from
    /// 60 with repeats across pushes, small pushes and a few over 8 KiB.
    fn drain(count: usize, with_ring: bool) -> Vec<Outgoing> {
        let mut x = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = move |m: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % m
        };
        (0..count)
            .map(|i| {
                let n = 1 + next(40) as usize;
                let clients: Vec<i64> = (0..n).map(|_| 1_000 + next(60) as i64).collect();
                let push: Box<dyn crate::notify::Push> = match (i % 5, with_ring) {
                    (0, _) => Box::new(Blob {
                        len: 9_000 + next(20_000) as usize,
                        seed: i as u64,
                        compressible: i % 10 != 0,
                        to: clients.clone(),
                    }),
                    (1, _) => Box::new(crate::modules::SpecialFx {
                        to: clients.clone(),
                        ship_id: 100_000_001 + i as i64,
                        module_id: 100_000_050,
                        type_id: 990_001,
                        target_id: Some(100_000_002),
                        guid: "effects.Invented",
                        is_offensive: true,
                        start: i % 2 == 0,
                        duration_ms: 3_000,
                        repeat: 1000,
                    }),
                    (2, true) => Box::new(crate::combat::RingUpdate {
                        to: clients.clone(),
                        owner_id: 7,
                        ship_id: 100_000_003,
                        change: crate::damage::RingChange {
                            attribute_id: 9_001,
                            new: 1.5,
                            old: 2.5,
                        },
                    }),
                    _ => Box::new(crate::modules::AttributeChange {
                        to: clients.clone(),
                        owner_id: 1,
                        item_id: 100_000_001,
                        attribute_id: 9_001,
                        time: i as i64,
                        new_value: 12.5,
                        old_value: 20.0,
                    }),
                };
                Outgoing { push, clients }
            })
            .collect()
    }

    /// What the old `encode_frames` did, kept as the oracle.
    fn encode_frames_reference(node_id: i64, frames: &[Outgoing]) -> Vec<(i64, Vec<u8>)> {
        let mut out = Vec::new();
        for Outgoing { push, clients } in frames {
            let (event, args) = push.to_push();
            let body = eve_proto::MachoPacket::notification_body(args);
            for &client_id in clients {
                let payload = eve_proto::MachoPacket::notification_pickled(
                    eve_proto::MachoAddress::node(node_id),
                    event,
                    "*clientID",
                    vec![PyValue::Int(client_id)],
                    body.clone(),
                )
                .to_payload()
                .expect("encodes");
                out.push((client_id, payload));
            }
        }
        out
    }

    /// **PA-9 byte identity.** Phase A runs once (a `RingUpdate` reads the wall
    /// clock in `to_push`, so two phase-A runs would differ); phase B runs at
    /// every thread count from that one result and must give the same frames,
    /// and every frame's wire bytes must equal what the writer would send.
    #[test]
    fn the_parallel_encode_is_byte_for_byte_the_serial_one() {
        let frames = drain(300, true);
        let bodies = encode_bodies(&frames, 1, usize::MAX);
        assert_eq!(bodies.len(), frames.len());
        let serial = encode_jobs_with(9, &frames, &bodies, 1, 0);
        assert!(
            serial.iter().all(|(_, f)| matches!(f, Frame::Raw(_))),
            "one thread is the old path: the writers compress"
        );
        assert!(serial.len() > 2_000, "enough frames: {}", serial.len());
        let big = serial
            .iter()
            .filter(|(_, f)| f.cap_len() >= COMPRESS_MIN_BYTES);
        assert!(big.count() > 20, "some frames pass the deflate floor");
        for threads in [2, 3, 8] {
            let par = encode_jobs_with(9, &frames, &bodies, threads, 0);
            assert_eq!(par.len(), serial.len(), "{threads} threads");
            for (i, (a, b)) in serial.iter().zip(&par).enumerate() {
                assert_eq!(a.0, b.0, "{threads} threads, frame {i}: client");
                assert_eq!(a.1.payload(), b.1.payload(), "{threads} threads, frame {i}");
                assert_eq!(
                    a.1.wire_bytes(),
                    b.1.wire_bytes(),
                    "{threads} threads, frame {i}"
                );
                assert_eq!(a.1.cap_len(), b.1.cap_len());
            }
            assert!(
                par.iter()
                    .any(|(_, f)| matches!(f, Frame::Ready { raw: Some(_), .. })),
                "a big compressible frame was deflated in the encode phase"
            );
        }
        // Below the threshold the parallel setting is the serial path.
        let small = encode_jobs_with(9, &frames, &bodies, 8, usize::MAX);
        assert_eq!(small, serial);
        // Deterministic pushes: the whole function equals the old body.
        let det = drain(300, false);
        let old = encode_frames_reference(9, &det);
        for threads in [1, 2, 8] {
            let got: Vec<(i64, Vec<u8>)> = encode_frames_with(9, &det, threads)
                .into_iter()
                .map(|(c, f)| (c, f.payload().to_vec()))
                .collect();
            assert_eq!(got, old, "{threads} threads");
        }
    }

    #[test]
    fn per_client_order_survives_the_parallel_encode() {
        let frames = drain(300, true);
        let bodies = encode_bodies(&frames, 8, 0);
        let serial = encode_jobs_with(9, &frames, &bodies, 1, 0);
        let par = encode_jobs_with(9, &frames, &bodies, 8, 0);
        for client in 1_000..1_060_i64 {
            let of = |v: &[(i64, Frame)]| -> Vec<Vec<u8>> {
                v.iter()
                    .filter(|(c, _)| *c == client)
                    .map(|(_, f)| f.payload().to_vec())
                    .collect()
            };
            let (a, b) = (of(&serial), of(&par));
            assert!(!a.is_empty(), "client {client} is addressed");
            assert_eq!(a, b, "client {client}");
        }
    }

    /// Payloads either side of every decision in `compress_payload`.
    fn decision_payloads() -> Vec<Vec<u8>> {
        vec![
            vec![5; 100],
            vec![5; COMPRESS_MIN_BYTES - 1],
            (0..COMPRESS_MIN_BYTES).map(|i| (i % 7) as u8).collect(),
            (0..20 * 1024).map(|i| (i % 11) as u8).collect(),
            noise(16 * 1024, 3),
        ]
    }

    #[test]
    fn a_ready_frame_writes_the_bytes_a_raw_frame_would() {
        let writers = Writers::default();
        let (_w1, p1) = registered(&writers, 1);
        let (_w2, p2) = registered(&writers, 2);
        let now = Instant::now();
        let all = decision_payloads();
        for p in &all {
            assert!(writers.post(1, now, p.clone()));
            assert!(writers.post_frame(2, now, Frame::encoded(p.clone())));
        }
        sweep_shard(&writers, 0, 1, Instant::now());
        let (a, b) = (read_frames(p1, all.len()), read_frames(p2, all.len()));
        assert_eq!(a.len(), all.len());
        assert_eq!(a, b);
    }

    #[test]
    fn the_capture_sees_the_uncompressed_payload_either_way() {
        let dir = std::env::temp_dir().join(format!("pa9-cap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let writers = Writers::default();
        let mut paths = Vec::new();
        let mut peers = Vec::new();
        let mut keep = Vec::new();
        for id in [1_i64, 2] {
            let path = dir.join(format!("{id}.evecap"));
            let (ours, peer) = pair();
            let cap = Capture::create(&path).expect("capture");
            let wire = Arc::new(Mutex::new(Wire::new(ours, Some(cap))));
            writers.register(id, &wire);
            keep.push(wire);
            peers.push(peer);
            paths.push(path);
        }
        let now = Instant::now();
        let all = decision_payloads();
        for p in &all {
            assert!(writers.post(1, now, p.clone()));
            assert!(writers.post_frame(2, now, Frame::encoded(p.clone())));
        }
        sweep_shard(&writers, 0, 1, Instant::now());
        for peer in peers {
            assert_eq!(read_frames(peer, all.len()).len(), all.len());
        }
        let records = |p: &std::path::Path| -> Vec<Vec<u8>> {
            crate::capture::read_all(p)
                .expect("capture reads")
                .into_iter()
                .map(|r| r.bytes)
                .collect()
        };
        let (a, b) = (records(&paths[0]), records(&paths[1]));
        assert_eq!(a, all, "raw frames: the payloads");
        assert_eq!(b, all, "ready frames: the payloads, not the deflates");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_ready_frame_counts_the_raw_length_against_the_cap() {
        let writers = Writers::default();
        let (_w1, _p1) = registered(&writers, 1);
        let (_w2, _p2) = registered(&writers, 2);
        let far = Instant::now() + Duration::from_secs(3600);
        let chunk = OUTBOX_BYTES / 4;
        // Highly compressible, so a Ready frame's wire is a few hundred bytes.
        let big = vec![7u8; chunk];
        let ready = Frame::encoded(big.clone());
        match &ready {
            Frame::Ready { wire, raw_len, .. } => {
                assert!(wire.len() < raw_len / 100);
                assert_eq!(*raw_len, chunk);
            }
            Frame::Raw(_) | Frame::Part(_) => panic!("deflated"),
        }
        for i in 0..4 {
            assert!(writers.post(1, far, big.clone()), "raw {i}");
            assert!(writers.post_frame(2, far, ready.clone()), "ready {i}");
        }
        assert!(!writers.post(1, far, vec![1]), "raw: refused past the cap");
        assert!(
            !writers.post_frame(2, far, Frame::Raw(vec![1])),
            "ready frames filled the cap at the same frame"
        );
    }

    /// Per-client order under concurrent shards: many clients, each posted a
    /// numbered sequence, swept by several shard threads at once; every peer
    /// reads its own numbers in order.
    #[test]
    fn shards_keep_each_clients_frame_order() {
        let writers = Arc::new(Writers::default());
        let shards = 3;
        let ids = [10_i64, 11, 12, 13, 14, 15, 16];
        let per = 200u32;
        let mut keep = Vec::new();
        let mut readers = Vec::new();
        for id in ids {
            let (wire, peer) = registered(&writers, id);
            keep.push(wire);
            readers.push(std::thread::spawn(move || read_frames(peer, per as usize)));
        }
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut pool = Vec::new();
        for s in 0..shards {
            let (w, stop) = (Arc::clone(&writers), Arc::clone(&stop));
            pool.push(std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    sweep_shard(&w, s, shards, Instant::now());
                    std::thread::sleep(Duration::from_millis(1));
                }
            }));
        }
        let now = Instant::now();
        for n in 0..per {
            for id in ids {
                assert!(writers.post(id, now, n.to_be_bytes().to_vec()));
            }
        }
        for (r, id) in readers.into_iter().zip(ids) {
            let frames = r.join().expect("reader");
            let got: Vec<u32> = frames
                .iter()
                .map(|f| u32::from_be_bytes(f[..4].try_into().expect("4 bytes")))
                .collect();
            assert_eq!(got, (0..per).collect::<Vec<_>>(), "client {id}");
        }
        stop.store(true, Ordering::Relaxed);
        for t in pool {
            t.join().expect("shard");
        }
    }

    /// A peer that never reads stalls its own shard for one budget per pass,
    /// not for 30 s: a neighbour in the same shard still gets its frame within
    /// a bound, and the partial write is kept (`unsent`).
    #[test]
    fn a_stalled_peer_does_not_hold_up_its_neighbours() {
        let writers = Writers::default();
        let (stalled, _never_read) = registered(&writers, 1);
        let (healthy, peer) = registered(&writers, 2);
        // Both in one shard (of = 1): the worst case.
        let big = noise(256 * 1024, 3);
        let now = Instant::now();
        // Enough to fill the socket buffers on loopback, under the cap.
        for _ in 0..56 {
            assert!(writers.post(1, now, big.clone()));
        }
        assert!(writers.post(2, now, vec![7; 32]));
        // The first pass deflates the batch (slow in a debug build) and fills
        // the window; the bound is on a pass that only meets the shut window.
        sweep_shard(&writers, 0, 1, now);
        let t = Instant::now();
        sweep_shard(&writers, 0, 1, now);
        let took = t.elapsed();
        assert!(
            took < WRITE_BUDGET * 3,
            "one pass over a stalled peer took {took:?}"
        );
        let frames = read_frames(peer, 1);
        assert_eq!(frames, vec![vec![7; 32]]);
        assert!(stalled.lock().expect("wire").unsent() > 0, "tail kept");
        assert!(!stalled.lock().expect("wire").is_closed());
        drop(healthy);
    }

    /// A partial write resumes: the peer, reading slowly, receives exactly the
    /// bytes one blocking write of every frame would have produced.
    #[test]
    fn a_partial_write_resumes_byte_identical() {
        let writers = Writers::default();
        let (wire, mut peer) = registered(&writers, 5);
        let frames: Vec<Vec<u8>> = (0..200u64).map(|i| noise(40 * 1024, i)).collect();
        let mut expect = Vec::new();
        for f in &frames {
            write_frame(&compress_payload(f), &mut expect).expect("frame");
        }
        let now = Instant::now();
        for f in &frames {
            assert!(writers.post(5, now, f.clone()));
        }
        let want = expect.len();
        let reader = std::thread::spawn(move || {
            let mut got = Vec::new();
            let mut chunk = vec![0u8; 8192];
            peer.set_read_timeout(Some(Duration::from_secs(10))).ok();
            while got.len() < want {
                // Slow enough that the writer meets a full window.
                std::thread::sleep(Duration::from_millis(2));
                match peer.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got.extend_from_slice(&chunk[..n]),
                }
            }
            got
        });
        let mut resumed = false;
        for _ in 0..2000 {
            sweep_shard(&writers, 0, 1, Instant::now());
            let w = wire.lock().expect("wire");
            resumed |= w.unsent() > 0;
            if w.unsent() == 0 && w.queued() == 0 {
                break;
            }
            drop(w);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(reader.join().expect("reader"), expect);
        assert!(resumed, "the test never met a partial write");
    }

    /// A stalled peer's outbox still hits its cap and disconnects; a peer that
    /// takes no byte for [`STALL_LIMIT`] is hung up by the writer.
    #[test]
    fn a_stalled_peer_is_hung_up_and_overflow_still_disconnects() {
        let writers = Writers::default();
        let (wire, _never_read) = registered(&writers, 9);
        let big = noise(256 * 1024, 4);
        let t0 = Instant::now();
        for _ in 0..56 {
            assert!(writers.post(9, t0, big.clone()));
        }
        for _ in 0..3 {
            sweep_shard(&writers, 0, 1, t0);
        }
        assert!(wire.lock().expect("wire").unsent() > 0);
        // Time passes with the window still shut.
        sweep_shard(&writers, 0, 1, t0 + STALL_LIMIT + Duration::from_secs(1));
        assert!(wire.lock().expect("wire").is_closed(), "hung up");

        // And the cap, as before, through the same API.
        let (ours, _peer) = pair();
        let w2 = Arc::new(Mutex::new(Wire::new(ours, None)));
        writers.register(10, &w2);
        let far = Instant::now() + Duration::from_secs(3600);
        for _ in 0..OUTBOX_FRAMES {
            assert!(writers.post(10, far, vec![1]));
        }
        assert!(!writers.post(10, far, vec![1]));
        assert!(w2.lock().expect("wire").is_closed());
    }

    /// The shard split is a partition of the live connections.
    #[test]
    fn live_shard_partitions_the_registry() {
        let writers = Writers::default();
        let keep: Vec<_> = (0..10).map(|i| registered(&writers, i)).collect();
        let mut n = 0;
        for s in 0..4 {
            n += writers.live_shard(s, 4).len();
        }
        assert_eq!(n, keep.len());
        assert_eq!(writers.live_shard(1, 4).len(), 3);
    }

    #[test]
    fn a_closed_wire_refuses_a_push_and_the_peer_gets_nothing() {
        let (ours, peer) = pair();
        let mut wire = Wire::new(ours, None);
        wire.close();
        let err = wire.send_all(&[vec![1, 2, 3]]).expect_err("must refuse");
        assert!(matches!(err, ConnEnd::DispatchClosed), "{err:?}");
        assert!(read_frames(peer, 1).is_empty());
    }

    /// **The inversion of §26.2, as a test rather than a paragraph.**
    ///
    /// A pusher must be able to queue a frame for a connection whose socket
    /// write is in progress. The `Wire` mutex is what a blocked `write_all`
    /// holds, for up to `conn::WRITE_TIMEOUT`, and two callers held the world
    /// write lock while waiting on it -- so one peer with a full TCP receive
    /// window froze the world for every player.
    ///
    /// Holding the wire guard *is* the blocked write, modelled exactly: this
    /// test keeps it alive across the post.
    ///
    /// **Measured rather than argued**, because the old form's failure is a
    /// hang and a hanging test proves nothing in a suite that would hang with
    /// it. A standalone probe held a wire mutex on one thread and had another
    /// try each form: the old `wire.lock()` **blocked for the full 500 ms
    /// timeout**, the outbox lock got through immediately and the frame
    /// landed. Across threads the old form blocks for as long as the write
    /// does -- up to `conn::WRITE_TIMEOUT`; on one thread, as here, it
    /// deadlocks outright. Either way the symptom is a freeze and not a wrong
    /// answer, which is why this sat unnoticed: nothing fails, the game just
    /// stops for everyone.
    #[test]
    fn a_push_does_not_wait_on_a_socket_that_is_mid_write() {
        let (ours, _peer) = pair();
        let wire = Arc::new(Mutex::new(Wire::new(ours, None)));
        let writers = Writers::default();
        writers.register(7, &wire);

        // The blocked write: this guard stands for a thread inside
        // `write_all` against a peer that is not reading.
        let guard = wire.lock().expect("wire");

        assert!(
            writers.post(7, Instant::now(), vec![4; 8]),
            "a push must be queueable while the socket is mid-write",
        );

        // And it really landed, rather than being quietly dropped.
        drop(guard);
        assert_eq!(wire.lock().expect("wire").queued(), 1);
    }

    /// A post to a client nobody registered, and to one that has closed, are
    /// both refusals rather than panics -- the same normal answer `wire_for`
    /// gives by returning `None`.
    #[test]
    fn posting_to_an_unknown_or_closed_connection_is_refused() {
        let (ours, _peer) = pair();
        let wire = Arc::new(Mutex::new(Wire::new(ours, None)));
        let writers = Writers::default();
        writers.register(7, &wire);

        assert!(
            !writers.post(999, Instant::now(), vec![1]),
            "unknown client"
        );

        wire.lock().expect("wire").close();
        assert!(
            !writers.post(7, Instant::now(), vec![1]),
            "a closed connection must not accumulate frames",
        );
        assert_eq!(wire.lock().expect("wire").queued(), 0);
    }

    /// **IP30 (V3 R6).** A peer that stops reading cannot grow the outbox past
    /// its cap: the post that would cross it is refused, the wire is marked
    /// closed, and the socket is shut down, so the connection's own read
    /// returns and its thread ends the session (`server::SessionGuard`).
    #[test]
    fn posting_past_the_cap_marks_the_wire_closed() {
        let far = Instant::now() + Duration::from_secs(3600);
        // By frame count.
        let (ours, mut peer) = pair();
        let mut reader = ours.try_clone().expect("clone");
        let wire = Arc::new(Mutex::new(Wire::new(ours, None)));
        let writers = Writers::default();
        writers.register(7, &wire);
        for _ in 0..OUTBOX_FRAMES {
            assert!(writers.post(7, far, vec![1]), "below the cap is taken");
        }
        assert!(!wire.lock().expect("wire").is_closed());
        assert!(!writers.post(7, far, vec![1]), "past the cap is refused");
        assert!(wire.lock().expect("wire").is_closed());
        assert_eq!(wire.lock().expect("wire").queued(), 0, "and emptied");
        assert!(!writers.post(7, far, vec![1]), "and stays refused");
        // The connection's thread wakes: its read sees the end of the stream.
        reader
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        let mut b = [0u8; 1];
        assert_eq!(reader.read(&mut b).expect("woken, not timed out"), 0);
        peer.set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        assert_eq!(peer.read(&mut b).expect("peer sees the end"), 0);
        // A reply on the owner's thread is not written either.
        assert!(wire.lock().expect("wire").send_all(&[vec![2]]).is_err());

        // By bytes, through the owner's own `Wire::post`.
        let (ours, _peer) = pair();
        let mut wire = Wire::new(ours, None);
        wire.post(far, vec![0; OUTBOX_BYTES]);
        assert_eq!(wire.queued(), 1, "exactly the byte cap is taken");
        assert!(!wire.is_closed());
        wire.post(far, vec![0; 1]);
        assert!(wire.is_closed());
        assert_eq!(wire.queued(), 0);
    }

    /// A normal flush below the cap is unchanged, and what it drains is
    /// returned to the budget: a client that reads never meets the cap.
    #[test]
    fn a_drained_outbox_takes_a_full_budget_again() {
        let (ours, _peer) = pair();
        let mut wire = Wire::new(ours, None);
        let now = Instant::now();
        for round in 0..3 {
            for _ in 0..OUTBOX_FRAMES {
                wire.post(now, vec![round; OUTBOX_BYTES / OUTBOX_FRAMES]);
            }
            assert_eq!(wire.queued(), OUTBOX_FRAMES);
            let drained = wire.outbox.lock().expect("outbox").take_due(now);
            assert_eq!(drained.len(), OUTBOX_FRAMES);
            assert!(!wire.is_closed(), "round {round} stayed open");
        }
    }

    #[test]
    fn the_outbox_peak_line_names_both_numbers_and_the_caps() {
        let line = outbox_peak_line(3, 1234);
        assert!(line.starts_with("[push] outbox peak "), "{line}");
        for needle in [
            "3 frames",
            "1234 bytes",
            &format!("cap {OUTBOX_FRAMES} frames {OUTBOX_BYTES} bytes"),
        ] {
            assert!(line.contains(needle), "{needle:?} missing from {line:?}");
        }
    }

    /// `close` clears the outbox, so a frame queued before the connection ended
    /// cannot be written after it.
    #[test]
    fn closing_discards_queued_frames() {
        let (ours, _peer) = pair();
        let mut wire = Wire::new(ours, None);
        wire.post(Instant::now(), vec![7; 8]);
        assert_eq!(wire.queued(), 1);
        wire.close();
        assert_eq!(wire.queued(), 0);
        assert!(wire.is_closed());
    }

    /// NET-TAKEOVER: an evicted connection gets its close frame and is hung
    /// up; with a reader that takes nothing and frames queued behind it, it is
    /// hung up at the deadline all the same.
    #[test]
    fn evict_hangs_up_at_the_deadline_with_a_stalled_reader() {
        let writers = Writers::default();
        let (ours, mut peer) = pair();
        let wire = Arc::new(Mutex::new(Wire::new(ours, None)));
        writers.register(10000000601, &wire);
        // Fill the socket buffers so the reader (which never reads) stalls us.
        let now = Instant::now();
        for _ in 0..40 {
            writers.post(10000000601, now, vec![9; 256 * 1024]);
        }
        assert!(writers.evict(10000000601, vec![1; 16], Duration::from_millis(50)));
        sweep_shard(&writers, 0, 1, now);
        assert!(!wire.lock().unwrap().is_closed(), "inside the grace");
        sweep_shard(&writers, 0, 1, now + Duration::from_millis(60));
        assert!(wire.lock().unwrap().is_closed(), "hung up at the deadline");
        // The peer sees the stream end once it drains what was sent.
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut sink = vec![0u8; 1 << 16];
        let ended = loop {
            match peer.read(&mut sink) {
                Ok(0) => break true,
                Ok(_) => {}
                Err(_) => break false,
            }
        };
        assert!(ended, "end of stream after the hang-up");
        assert!(
            !writers.evict(10000000602, vec![1], Duration::ZERO),
            "unknown id"
        );
    }

    /// A healthy reader: the close frame leaves, then the wire hangs up at
    /// once, long before the grace.
    #[test]
    fn evict_hangs_up_as_soon_as_the_close_frame_has_left() {
        let writers = Writers::default();
        let (ours, peer) = pair();
        let wire = Arc::new(Mutex::new(Wire::new(ours, None)));
        writers.register(10000000611, &wire);
        assert!(writers.evict(10000000611, vec![5; 8], Duration::from_secs(60)));
        sweep_shard(&writers, 0, 1, Instant::now());
        assert!(wire.lock().unwrap().is_closed());
        let frames = read_frames(peer, 1);
        assert_eq!(frames, vec![vec![5; 8]]);
    }

    /// A frame posted for the future stays in the outbox; one posted for the
    /// past goes out. The ticker's whole contract, with no thread involved.
    #[test]
    fn flush_due_writes_only_what_is_due() {
        let (ours, peer) = pair();
        let mut wire = Wire::new(ours, None);
        let now = Instant::now();
        wire.post(now, vec![1; 16]);
        wire.post(now + Duration::from_secs(60), vec![2; 16]);
        wire.flush_due(now).expect("flush");
        assert_eq!(wire.queued(), 1, "the future frame must still be queued");
        wire.close();
        let frames = read_frames(peer, 1);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], vec![1; 16]);
    }

    /// The registry holds `Weak`, so a connection that ended leaves it without
    /// being told. This is the lifetime claim in `Writers`' doc comment, tested.
    #[test]
    fn a_dead_connection_leaves_the_registry() {
        let writers = Writers::default();
        let (ours, _peer) = pair();
        let wire = Arc::new(Mutex::new(Wire::new(ours, None)));
        writers.register(10000000423, &wire);
        assert_eq!(writers.live().len(), 1);
        drop(wire);
        assert!(writers.live().is_empty());
    }

    /// Registering twice must not leave two slots for one connection, and a
    /// dead slot must not survive a later registration.
    #[test]
    fn register_sweeps_dead_slots() {
        let writers = Writers::default();
        {
            let (ours, _peer) = pair();
            let gone = Arc::new(Mutex::new(Wire::new(ours, None)));
            writers.register(10000000437, &gone);
        }
        let (ours, _peer) = pair();
        let live = Arc::new(Mutex::new(Wire::new(ours, None)));
        writers.register(10000000441, &live);
        assert_eq!(writers.live().len(), 1);
    }

    /// PA-1: the map answers like the Vec did -- `live()` in registration
    /// order, a dead id gives `None`/`false`, a duplicate id keeps the first.
    #[test]
    fn writers_map_keeps_registration_order_and_dead_answers() {
        let writers = Writers::default();
        let mut keep = Vec::new();
        for id in [30i64, 10, 20, 40] {
            let (ours, peer) = pair();
            let wire = Arc::new(Mutex::new(Wire::new(ours, None)));
            writers.register(id, &wire);
            keep.push((id, wire, peer));
        }
        let order: Vec<_> = writers.live().iter().map(Arc::as_ptr).collect();
        let want: Vec<_> = keep.iter().map(|(_, w, _)| Arc::as_ptr(w)).collect();
        assert_eq!(order, want);
        let (_, dead, _) = keep.remove(1); // id 10
        drop(dead);
        assert!(writers.wire_for(10).is_none());
        assert_eq!(writers.live().len(), 3, "the sweep drops the dead row");
        assert!(!writers.post(10, Instant::now(), vec![1]));
        assert!(writers.post(20, Instant::now(), vec![1]));
        // a second wire under a live id does not displace the first
        let (ours, _p) = pair();
        let dup = Arc::new(Mutex::new(Wire::new(ours, None)));
        writers.register(20, &dup);
        let got = writers.wire_for(20).expect("live");
        assert!(Arc::ptr_eq(&got, &keep[1].1), "first live row wins");
    }

    /// PA-1 benchmark: 500 connections, `post` per connection, map vs the
    /// Vec scan it replaced. `cargo test --release -- --ignored writers_bench --nocapture`.
    #[test]
    #[ignore]
    fn writers_bench_post_500() {
        let writers = Writers::default();
        let mut keep = Vec::new();
        let (base, _peer) = pair();
        for k in 0..500i64 {
            let ours = base.try_clone().expect("clone");
            let wire = Arc::new(Mutex::new(Wire::new(ours, None)));
            writers.register(10_000_000_000 + k, &wire);
            keep.push(wire);
        }
        let rows: Vec<(i64, Arc<Mutex<Outbox>>)> = keep
            .iter()
            .enumerate()
            .map(|(k, w)| (10_000_000_000 + k as i64, w.lock().expect("w").outbox()))
            .collect();
        let rounds = 200;
        let t0 = Instant::now();
        for _ in 0..rounds {
            for k in 0..500i64 {
                let g = writers.guard();
                std::hint::black_box(g.get(&(10_000_000_000 + k)).map(|r| Arc::clone(&r.outbox)));
            }
        }
        let indexed = t0.elapsed();
        let t0 = Instant::now();
        for _ in 0..rounds {
            for k in 0..500i64 {
                let g = writers.guard();
                std::hint::black_box(
                    rows.iter()
                        .find(|r| r.0 == 10_000_000_000 + k)
                        .map(|r| Arc::clone(&r.1)),
                );
                drop(g);
            }
        }
        let scan = t0.elapsed();
        eprintln!(
            "Writers lookup x{}: map {:?}, scan {:?} ({:.0}x)",
            rounds * 500,
            indexed,
            scan,
            scan.as_secs_f64() / indexed.as_secs_f64().max(1e-9)
        );
    }

    /// The ticker is one thread for the process and it writes what is due.
    #[test]
    fn the_ticker_writes_a_due_frame() {
        let writers = Arc::new(Writers::default());
        spawn_writers(Arc::clone(&writers), 2).expect("spawn writers");
        let (ours, peer) = pair();
        let wire = Arc::new(Mutex::new(Wire::new(ours, None)));
        writers.register(10000000452, &wire);
        wire.lock().expect("wire").post(Instant::now(), vec![9; 32]);
        let frames = read_frames(peer, 1);
        assert_eq!(frames.len(), 1, "the ticker never wrote");
        assert_eq!(frames[0], vec![9; 32]);
    }

    /// **A session change the clock caused reaches the player without a
    /// turn.** A ship destroyed on the world tick has to move its pilot
    /// into a capsule, and the pilot may be calling nothing; before this the
    /// queue drained only after a payload. Addressed per connection, with
    /// that connection's own session id.
    #[test]
    fn the_clock_drains_queued_session_changes_to_every_connection() {
        let mut w = crate::service::Registry::new(crate::model::SandboxConfig::default());
        let who = w.config().characters[0].character_id;
        for client_id in [10_000_000_001, 10_000_000_002] {
            w.session_opened(crate::session_table::SessionEntry {
                client_id,
                session_id: client_id + 7,
                user_id: 1,
                character_id: None,
            });
            w.session_selected_character(client_id, who);
        }
        let change = crate::pyutil::pypairs(vec![(
            "shipid",
            PyValue::Tuple(vec![PyValue::Int(1), PyValue::Int(2)]),
        )]);
        w.queue_session_change(who, change.clone());
        let mut got = session_changes_due(&mut w);
        got.sort_by_key(|(c, _, _)| *c);
        assert_eq!(
            got,
            vec![
                (10_000_000_001, 10_000_000_008, change.clone()),
                (10_000_000_002, 10_000_000_009, change),
            ]
        );
        assert!(session_changes_due(&mut w).is_empty(), "drained once");
    }
}

#[cfg(test)]
mod compress_tests {
    use super::*;

    #[test]
    fn small_payloads_stay_plain_and_large_ones_inflate_back() {
        let small = vec![0x7Eu8; COMPRESS_MIN_BYTES - 1];
        assert!(matches!(compress_payload(&small), Cow::Borrowed(_)));
        let mut big = vec![0x7E, 0, 0, 0, 0];
        big.extend((0..COMPRESS_MIN_BYTES as u32 * 4).map(|i| (i % 7) as u8));
        let z = compress_payload(&big);
        assert!(z.len() < big.len() / 4);
        // The client's own test: a first byte that is not 0x7D/0x7E is zlib.
        assert!(eve_marshal::compression::is_compressed(&z));
        assert_eq!(eve_inflate::inflate_zlib(&z).unwrap(), big);
    }

    #[test]
    fn incompressible_payloads_are_sent_as_they_are() {
        let mut x = 0x2545_F491u32;
        let noise: Vec<u8> = (0..COMPRESS_MIN_BYTES * 2)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 8) as u8
            })
            .collect();
        let mut p = vec![0x7E];
        p.extend(noise);
        assert!(matches!(compress_payload(&p), Cow::Borrowed(_)));
    }

    /// **The clock's world-lock reads are gone** (lane fight-stall-fix,
    /// step 2): std's `RwLock` refuses a reader while any writer waits, so a
    /// `world.read()` at the top of the tick starved behind the writers' whole
    /// convoy and was counted as the clock's lock wait. The ticker takes the
    /// queue length inside its first slice instead. A source scan, in the
    /// style of `dispatch`'s read-turn test.
    #[test]
    fn the_ticker_takes_no_world_read_lock() {
        let src = include_str!("push.rs");
        let body = src
            .split("fn spawn_world_ticker_every(")
            .nth(1)
            .and_then(|s| s.split("#[cfg(test)]\nmod brawl_bench;").next())
            .expect("the function");
        assert!(body.contains("clock_slices_gated("));
        assert!(!body.contains("world.read()"));
        assert!(!body.contains(".read()"));
    }

    fn quiet_world() -> Arc<std::sync::RwLock<crate::service::Registry>> {
        Arc::new(std::sync::RwLock::new(crate::service::Registry::new(
            crate::model::SandboxConfig::default(),
        )))
    }

    /// A raised gate holds a waiter until it is lowered.
    #[test]
    fn a_raised_gate_holds_a_waiter_until_it_is_lowered() {
        let gate = Arc::new(ClockGate::with_cap(Duration::from_secs(60)));
        gate.wait_for_clock();
        gate.raise();
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = {
            let gate = Arc::clone(&gate);
            std::thread::spawn(move || {
                gate.wait_for_clock();
                let _ = tx.send(());
            })
        };
        assert!(
            rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "held back while raised"
        );
        gate.lower();
        rx.recv_timeout(Duration::from_secs(2))
            .expect("released by lower");
        waiter.join().expect("joined");
        assert!(!gate.raised());
    }

    /// A clock that never lowers the gate cannot wedge the turns.
    #[test]
    fn the_cap_releases_a_waiter_when_the_gate_is_never_lowered() {
        let gate = Arc::new(ClockGate::with_cap(Duration::from_millis(100)));
        gate.raise();
        let (tx, rx) = std::sync::mpsc::channel();
        let g = Arc::clone(&gate);
        std::thread::spawn(move || {
            g.wait_for_clock();
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(10))
            .expect("the cap released the waiter");
        assert!(gate.raised(), "the cap does not lower it");
    }

    /// An uncontended tick never raises the gate: calm periods keep today's
    /// interleaving.
    #[test]
    fn a_calm_tick_leaves_the_gate_down() {
        let world = quiet_world();
        let gate = ClockGate::with_cap(Duration::from_secs(60));
        let (_, raised_inside, _) = clock_slices_gated(
            &world,
            Instant::now(),
            None,
            true,
            Some(&gate),
            |_, _, _| gate.raised(),
        );
        assert!(!raised_inside);
        assert!(!gate.raised());
    }

    /// **Priority, without a clock to time** (lane fight-stall-fix, step 3):
    /// 16 threads that each loop "wait for the clock; take the world write
    /// lock; hold it" queue behind a held lock; the clock finds it busy,
    /// raises the gate and waits once for those already queued. A hog can
    /// take the lock while the gate is up only if it passed the gate before
    /// the raise, and then its next turn waits at the gate: so at most 16
    /// acquisitions happen between the raise and the lower, across all of the
    /// tick's slices.
    #[test]
    fn the_gate_bounds_payload_turns_between_the_raise_and_the_lower() {
        const HOGS: usize = 16;
        let world = quiet_world();
        let gate = Arc::new(ClockGate::with_cap(Duration::from_secs(60)));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let during_raise = Arc::new(AtomicUsize::new(0));
        let total = Arc::new(AtomicUsize::new(0));
        let held = world.write().expect("unpoisoned");
        let hogs: Vec<_> = (0..HOGS)
            .map(|_| {
                let (world, gate, stop) =
                    (Arc::clone(&world), Arc::clone(&gate), Arc::clone(&stop));
                let (during, total) = (Arc::clone(&during_raise), Arc::clone(&total));
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        gate.wait_for_clock();
                        let _w = world.write().expect("unpoisoned");
                        total.fetch_add(1, Ordering::Relaxed);
                        if gate.raised() {
                            during.fetch_add(1, Ordering::Relaxed);
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                })
            })
            .collect();
        let clock = {
            let (world, gate) = (Arc::clone(&world), Arc::clone(&gate));
            std::thread::spawn(move || {
                let (_, raised_in_last_slice, _) = clock_slices_gated(
                    &world,
                    Instant::now(),
                    None,
                    true,
                    Some(&gate),
                    |_, _, _| gate.raised(),
                );
                raised_in_last_slice
            })
        };
        // The clock found the lock held and raised the gate.
        let waited = Instant::now();
        while !gate.raised() {
            assert!(waited.elapsed() < Duration::from_secs(30), "never raised");
            std::thread::yield_now();
        }
        drop(held);
        let raised_in_last_slice = clock.join().expect("the clock finished under 16 hogs");
        assert!(raised_in_last_slice, "kept up through the last slice");
        assert!(!gate.raised(), "lowered after the last slice");
        assert!(
            during_raise.load(Ordering::Relaxed) <= HOGS,
            "{} turns took the lock under a raised gate",
            during_raise.load(Ordering::Relaxed)
        );
        // And the turns go on afterwards.
        let after = total.load(Ordering::Relaxed);
        let waited = Instant::now();
        while total.load(Ordering::Relaxed) < after + HOGS {
            assert!(waited.elapsed() < Duration::from_secs(30), "turns resume");
            std::thread::sleep(Duration::from_millis(2));
        }
        stop.store(true, Ordering::Relaxed);
        for h in hogs {
            h.join().expect("joined");
        }
    }
}
