//! One connection: read frames, hand each payload to a [`Dispatch`], write
//! what comes back, log everything.
//!
//! Blocking I/O on a dedicated thread, from `std::net` alone. At the scale this
//! server targets — one player, or ten — a thread per connection is not a
//! compromise, it is the right answer: no runtime, no dependency, and a stack
//! trace that reads top to bottom. The framing is `eve_proto::try_split_frame`,
//! which is the client's own splitter reimplemented from disassembly
//! (`docs/reference/framing.md`).

use crate::dispatch::Dispatch;
use crate::push::Wire;
use eve_proto::{FrameError, MAX_FRAME, try_split_frame};
use std::io::{self, Read};
use std::net::TcpStream;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How much to ask the kernel for per `read`. A handshake payload is a few
/// hundred bytes; a cached-object blob can be megabytes.
const READ_CHUNK: usize = 64 * 1024;

/// How long a silent peer may stay connected.
///
/// **Comfortably longer than the client is entitled to be silent**, which is
/// the only constraint that matters: `machoNet`'s keepalive is 240 s and the
/// client legitimately sends nothing between pings
/// (`docs/archive/destiny-and-the-ballpark.md` §5.3), so anything near that would
/// disconnect a healthy player sitting in a station. What this catches is the
/// peer that has gone away without closing -- a killed client, a jail torn
/// down under it -- whose thread and whose *write* half would otherwise be
/// held forever. That second half is new: the push ticker now holds a `Weak`
/// to every live connection's write side, so a connection that never ends is
/// a frame queue that is never drained. `eve-dns` has had a read timeout
/// since it was written; this crate was the gap.
const IDLE_TIMEOUT: Duration = Duration::from_secs(600);

/// The server-side keepalive (NET-PING). A half-open connection -- a Wi-Fi
/// drop, a sleeping laptop -- sends nothing and closes nothing, so without
/// this it keeps its ship and its session for [`IDLE_TIMEOUT`]. The client
/// answers a `PingReq` (`machoNet.HandleMessage`), so after `probe_after` of
/// silence the server sends one, and gives up `dead_after` later if nothing
/// at all has come back. **[A]** 30 s and 120 s: the client's own keepalive is
/// 240 s, so a healthy client is never silent for 30 s *and* deaf to a ping;
/// falsified by `no answer to a keepalive ping` in the log for a client that
/// was still playing. Any inbound byte counts as an answer.
#[derive(Debug, Clone, Copy)]
pub struct KeepAlive {
    /// Silence before the server pings.
    pub probe_after: Duration,
    /// Time after the ping with nothing heard before the connection ends.
    pub dead_after: Duration,
}

impl KeepAlive {
    pub const DEFAULT: KeepAlive = KeepAlive {
        probe_after: Duration::from_secs(30),
        dead_after: Duration::from_secs(120),
    };
}

/// How long a single socket write may block before the connection is given up
/// on.
///
/// **This bounds a freeze of the whole world, not just of one player.**
/// `dispatch::deliver_elsewhere` posts a frame to another connection's wire
/// while holding the world write lock, and `serve` holds that same wire mutex
/// across `send_all`. So a peer whose TCP window has closed -- a laptop whose
/// lid shut, a jail torn down, a client stopped in a debugger -- makes its own
/// `send_all` block, which makes the next push to it block, which stalls every
/// other player until the socket gives up. With no timeout set, "gives up" is
/// the kernel's retransmission limit: minutes.
///
/// Thirty seconds is far longer than any frame this server sends needs on any
/// link a co-op game runs over, and far shorter than a player will tolerate a
/// frozen world. A write that misses it ends **that** connection, with the
/// partial frame making the stream unusable, which is the right outcome: a
/// client we cannot write to has already left.
///
/// This is a bound, not the fix. The fix is to stop holding a socket mutex
/// behind the world lock at all, by moving the outbox out from behind it --
/// `docs/port-tracker.md` §26 has the design. `eve-dns` has set both timeouts
/// since it was written; this crate set only the read one.
pub(crate) const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound on buffered bytes before we give up on a peer, after login.
///
/// `try_split_frame` already refuses a declared length over [`MAX_FRAME`], so
/// this only catches a peer that dribbles a legitimately huge frame. One frame
/// plus one chunk is all a correct peer ever needs buffered.
///
/// After login the ceiling is the client's own: `MAX_FRAME` is the native
/// per-socket cap `SocketGPS` installs **[V]** (`docs/reference/framing.md`), so a
/// logged-in client may legitimately send a frame that large and no limit
/// below it is justified.
const MAX_BUFFERED: usize = MAX_FRAME + READ_CHUNK;

/// The largest frame, header included, accepted **before the handshake is
/// done** (SEC-FIX 11) **[A]**. The client has no pre-login limit to cite;
/// what it sends there is the handshake's six small tuples, the largest
/// being the login request with its token (a few hundred bytes to a few
/// kilobytes: `docs/reference/login-path.md` records a 174-byte one). 256 KiB is two
/// orders of magnitude above that, and is all a peer that has proved nothing
/// can make us hold (instead of 10 MiB, per connection, for the whole
/// pre-login leash). Falsified by a real client's login frame over it: it
/// would be refused with `DeclaredTooLong` in the log.
pub const PREAUTH_MAX_FRAME: usize = 256 * 1024;

/// Refuse a frame from its 4-byte header, before waiting for any of its
/// body (SEC-FIX 11): a flag this server cannot honour (zlib, or the OOB
/// section machoNet never sets), or a declared length over `limit`.
/// `Ok(())` while the header is not all here yet or is acceptable.
pub(crate) fn check_header(buf: &[u8], limit: usize) -> Result<(), FrameError> {
    if buf.len() < 4 {
        return Ok(());
    }
    let word0 = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let h = eve_proto::FrameHeader::parse(word0);
    if h.zlib {
        return Err(FrameError::Compressed);
    }
    if h.oob {
        return Err(FrameError::UnknownFlags { word0 });
    }
    if h.frame_len() > limit.min(MAX_FRAME) {
        return Err(FrameError::DeclaredTooLong { plen: h.plen });
    }
    Ok(())
}

/// Why a connection ended.
#[derive(Debug)]
pub enum ConnEnd {
    /// The peer closed cleanly on a frame boundary.
    PeerClosed,
    /// We closed, because the dispatch asked us to.
    DispatchClosed,
    /// The peer closed mid-frame.
    Truncated { buffered: usize },
    /// Framing was violated.
    Framing(FrameError),
    /// The dispatch refused a payload.
    Dispatch(String),
    /// A socket or capture-file error.
    Io(io::Error),
    /// Another thread panicked holding this connection's write side.
    Poisoned,
    /// The peer said nothing for [`IDLE_TIMEOUT`] and did not close.
    Idle,
    /// A keepalive ping went unanswered for [`KeepAlive::dead_after`].
    KeepAliveExpired(Duration),
}

impl core::fmt::Display for ConnEnd {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ConnEnd::PeerClosed => write!(f, "peer closed"),
            ConnEnd::DispatchClosed => write!(f, "closed by us"),
            ConnEnd::Truncated { buffered } => {
                write!(f, "peer closed with {buffered} bytes of a partial frame")
            }
            ConnEnd::Framing(e) => write!(f, "framing error: {e}"),
            ConnEnd::Dispatch(e) => write!(f, "dispatch refused a payload: {e}"),
            ConnEnd::Io(e) => write!(f, "io error: {e}"),
            ConnEnd::Poisoned => write!(f, "write side poisoned by a panic"),
            ConnEnd::Idle => write!(f, "silent for {}s", IDLE_TIMEOUT.as_secs()),
            ConnEnd::KeepAliveExpired(d) => {
                write!(f, "no answer to a keepalive ping for {}s", d.as_secs())
            }
        }
    }
}

/// Run a connection to completion.
///
/// Returns how it ended rather than a `Result`: every outcome here is
/// information, and an `Err` would suggest some of them are bugs on our side
/// when most of them are facts about the peer.
pub fn serve(stream: TcpStream, wire: &Arc<Mutex<Wire>>, dispatch: &mut dyn Dispatch) -> ConnEnd {
    serve_with(stream, wire, dispatch, KeepAlive::DEFAULT)
}

/// [`serve`] with an explicit [`KeepAlive`] (tests use short ones).
pub fn serve_with(
    mut stream: TcpStream,
    wire: &Arc<Mutex<Wire>>,
    dispatch: &mut dyn Dispatch,
    keep: KeepAlive,
) -> ConnEnd {
    // `Nagle` would coalesce our handshake replies into one segment. The
    // client reads frame-at-a-time and does not care, but a capture is easier
    // to reason about when our writes land as we issued them.
    let _ = stream.set_nodelay(true);
    // Best-effort: a platform that refuses the option is better served by a
    // connection with no timeout than by no connection.
    let _ = stream.set_read_timeout(Some(IDLE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));

    let mut buf: Vec<u8> = Vec::with_capacity(READ_CHUNK);
    let mut chunk = vec![0u8; READ_CHUNK];
    // Keepalive state: when bytes last arrived, and when we pinged since.
    let mut last_inbound = Instant::now();
    let mut ping_sent: Option<Instant> = None;

    // The GPS handshake is server-initiated: H1 goes out before we read.
    let greeting = dispatch.greeting();
    if !greeting.is_empty()
        && let Err(end) = guard(wire).and_then(|mut w| w.send_all(&greeting))
    {
        return end;
    }

    loop {
        // Drain every complete frame already buffered before reading again.
        loop {
            // The header alone refuses what no frame of ours can be, and a
            // frame over the leash for this stage (`Dispatch::max_frame`),
            // before any of its body is waited for (SEC-FIX 11).
            let limit = dispatch.max_frame().unwrap_or(MAX_FRAME);
            if let Err(e) = check_header(&buf, limit) {
                return ConnEnd::Framing(e);
            }
            let split = match try_split_frame(&buf) {
                Ok(Some((frame, consumed))) => {
                    if frame.oob.is_some() {
                        // machoNet never sets the OOB flag. Seeing one means
                        // either a flag we mis-read or a path we have never
                        // traced, and both are worth stopping for.
                        return ConnEnd::Framing(FrameError::UnknownFlags {
                            word0: frame.header.plen,
                        });
                    }
                    (frame.payload.to_vec(), consumed)
                }
                Ok(None) => break,
                Err(e) => return ConnEnd::Framing(e),
            };
            let (payload, consumed) = split;
            buf.drain(..consumed);

            if let Err(end) = guard(wire).and_then(|mut w| w.record_inbound(&payload)) {
                return end;
            }

            let reply = match dispatch.on_payload(&payload) {
                Ok(r) => r,
                Err(e) => return ConnEnd::Dispatch(e.to_string()),
            };
            // One lock for the whole reply, so nothing can land between a
            // session change and the answer that follows it -- and the
            // immediate pushes go out in the *same* flush, which is what
            // `Duration::ZERO` means.
            let now = Instant::now();
            if let Err(end) = guard(wire).and_then(|mut w| {
                let mut out: Vec<Vec<u8>> = reply.payloads.clone();
                for (after, payload) in &reply.pushes {
                    if after.is_zero() {
                        out.push(payload.clone());
                    } else {
                        w.post(now + *after, payload.clone());
                    }
                }
                w.send_all(&out)
            }) {
                return end;
            }
            if reply.close {
                return ConnEnd::DispatchClosed;
            }
        }

        let limit = dispatch.max_frame().unwrap_or(MAX_FRAME);
        if buf.len() > limit.saturating_add(READ_CHUNK).min(MAX_BUFFERED) {
            return ConnEnd::Framing(FrameError::TooLong { len: buf.len() });
        }

        // The dispatch's leash before every read: short until login (V2 F5).
        // A dispatch with no leash of its own (logged in) gets the keepalive
        // schedule: wake when a ping is due, or when the unanswered one is
        // dead, and never later than the idle limit.
        let leash = dispatch.idle_timeout();
        let wait = match leash {
            Some(t) => t,
            None => {
                let until = match ping_sent {
                    Some(at) => keep.dead_after.saturating_sub(at.elapsed()),
                    None => keep.probe_after.saturating_sub(last_inbound.elapsed()),
                };
                until
                    .min(IDLE_TIMEOUT.saturating_sub(last_inbound.elapsed()))
                    .max(Duration::from_millis(1))
            }
        };
        let _ = stream.set_read_timeout(Some(wait));

        match stream.read(&mut chunk) {
            Ok(0) => {
                return if buf.is_empty() {
                    ConnEnd::PeerClosed
                } else {
                    ConnEnd::Truncated {
                        buffered: buf.len(),
                    }
                };
            }
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                last_inbound = Instant::now();
                ping_sent = None;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            // What a read timeout surfaces as is platform-dependent, and both
            // kinds mean the same thing here. Named rather than reported as an
            // I/O error, because "silent for 600s" is a fact about the peer
            // and `io error: Resource temporarily unavailable` reads like a
            // bug of ours.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if leash.is_some() || last_inbound.elapsed() >= IDLE_TIMEOUT {
                    return ConnEnd::Idle;
                }
                match ping_sent {
                    Some(at) if at.elapsed() >= keep.dead_after => {
                        return ConnEnd::KeepAliveExpired(at.elapsed());
                    }
                    Some(_) => {}
                    None if last_inbound.elapsed() >= keep.probe_after => {
                        if let Some(p) = dispatch.keepalive_ping() {
                            if let Err(end) = guard(wire).and_then(|mut w| w.send_all(&[p])) {
                                return end;
                            }
                            ping_sent = Some(Instant::now());
                        }
                    }
                    None => {}
                }
            }
            Err(e) => return ConnEnd::Io(e),
        }
    }
}

/// Lock this connection's write side, or end the connection.
///
/// **The opposite policy from `push::Writers::guard`, and for the opposite
/// reason.** A poisoned `Wire` means a thread panicked while holding the
/// socket, so the stream may be mid-frame: there is no byte we can write after
/// that which the client can parse, and pretending otherwise would produce a
/// framing error on the client with nothing in it that names us. The registry,
/// by contrast, holds a list of weak handles with no invariant at all, so
/// recovering there is right and refusing there would let one panicking
/// connection kill the push path for every other player.
fn guard(wire: &Arc<Mutex<Wire>>) -> Result<MutexGuard<'_, Wire>, ConnEnd> {
    wire.lock().map_err(|_| ConnEnd::Poisoned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{DispatchError, Reply};
    use eve_proto::write_frame;
    use std::io::Write as _;
    use std::net::TcpListener;

    /// Authenticated from the start; every inbound frame is answered with
    /// nothing and the ping is a fixed payload.
    struct Stub {
        pings: usize,
    }

    impl Dispatch for Stub {
        fn on_payload(&mut self, _payload: &[u8]) -> Result<Reply, DispatchError> {
            Ok(Reply::nothing())
        }
        fn state(&self) -> String {
            "stub".to_owned()
        }
        fn keepalive_ping(&mut self) -> Option<Vec<u8>> {
            self.pings += 1;
            Some(b"ping-payload".to_vec())
        }
    }

    const SHORT: KeepAlive = KeepAlive {
        probe_after: Duration::from_millis(50),
        dead_after: Duration::from_millis(150),
    };

    /// A server thread running `serve_with` over loopback, and the peer end.
    fn pair(keep: KeepAlive) -> (std::thread::JoinHandle<(ConnEnd, usize)>, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let peer = TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        let handle = std::thread::spawn(move || {
            let wire = Arc::new(Mutex::new(Wire::new(server.try_clone().unwrap(), None)));
            let mut d = Stub { pings: 0 };
            let end = serve_with(server, &wire, &mut d, keep);
            (end, d.pings)
        });
        (handle, peer)
    }

    /// Read whole frames off `peer` until it has `want` of them or `limit`
    /// passes; returns the payloads.
    fn frames(peer: &mut TcpStream, want: usize, limit: Duration) -> Vec<Vec<u8>> {
        peer.set_read_timeout(Some(Duration::from_millis(20))).ok();
        let (mut buf, mut out) = (Vec::new(), Vec::new());
        let start = Instant::now();
        let mut chunk = [0u8; 4096];
        while out.len() < want && start.elapsed() < limit {
            match peer.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => {}
            }
            while let Ok(Some((f, used))) = try_split_frame(&buf) {
                out.push(f.payload.to_vec());
                buf.drain(..used);
            }
        }
        out
    }

    #[test]
    fn a_silent_peer_gets_one_ping_then_expires() {
        let started = Instant::now();
        let (handle, mut peer) = pair(SHORT);
        let got = frames(&mut peer, 2, Duration::from_millis(600));
        assert_eq!(got, vec![b"ping-payload".to_vec()], "exactly one ping");
        let (end, pings) = handle.join().unwrap();
        assert!(matches!(end, ConnEnd::KeepAliveExpired(_)), "{end}");
        assert_eq!(pings, 1);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_peer_that_answers_stays_connected() {
        let (handle, mut peer) = pair(SHORT);
        let start = Instant::now();
        let mut pings = 0;
        while start.elapsed() < SHORT.dead_after * 5 {
            if !frames(&mut peer, 1, Duration::from_millis(100)).is_empty() {
                pings += 1;
                let mut framed = Vec::new();
                write_frame(b"pong", &mut framed).unwrap();
                peer.write_all(&framed).unwrap();
            }
        }
        assert!(pings >= 3, "pinged {pings} times");
        assert!(
            !handle.is_finished(),
            "still connected after 5 x dead_after"
        );
        drop(peer);
        let (end, _) = handle.join().unwrap();
        assert!(matches!(end, ConnEnd::PeerClosed), "{end}");
    }
}
