//! The seam between framing and meaning.
//!
//! The listener's job stops at "here is one complete payload". What happens to
//! that payload is a [`Dispatch`], and there are two of them for a reason: the
//! handshake is fully specified and implementable today, while the machoNet
//! traffic that follows it is not, because the object-wrapper type codes
//! (`0x17`/`0x22`/`0x23`) are still unknown. Keeping those apart means the part
//! we understand can run at full strength without pretending the rest works.

use crate::handshake::{Handshake, HandshakeError};
use crate::service::{Caller, Registry, Request, RoValue, ServiceError};

/// `cluster.MACHONETERR_WRAPPEDEXCEPTION`, read from `cluster/__init__.pyc`.
///
/// The other two codes are `UNMACHODESTINATION` (0) and `UNMACHOCHANNEL` (1),
/// which the client turns into routing exceptions of its own rather than
/// re-raising our payload. Only `2` reaches
/// `newException = machobase.LoadsSanitized(e.payload[0]); raise newException`.
const MACHONETERR_WRAPPEDEXCEPTION: i64 = 2;
use crate::session::Session;
use crate::wire::{dumps, loads};
use eve_marshal::{MachoNetMsg, PyValue, WireError};
use eve_proto::{CallPayload, MachoAddress, MachoPacket};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// How many distinct keys [`GpsSession::unanswered`] keeps before folding
/// the rest into [`UNANSWERED_OTHER`] (V2 F7).
///
/// **[A]** 256. The keys are formatted error text that carries byte offsets
/// and client strings, so a hostile or broken client mints a new one per
/// packet; a real session has produced a few dozen. Falsifier: a real
/// session's end-of-connection line showing `(other)`, which would hide a
/// to-do item behind the fold.
pub const UNANSWERED_KEYS: usize = 256;

/// The key every unanswered occurrence past [`UNANSWERED_KEYS`] is counted
/// under.
pub const UNANSWERED_OTHER: &str = "(other)";

/// How many bound objects one connection may hold (V2 F7).
///
/// **[A]** 4096. Past it a new object is still handed out but **not
/// registered**, so a call on it is answered as a call on an unknown object.
/// Nothing is evicted: an evicted id is a live reference the client still
/// calls. Falsifier: the `bound-object cap` line in a real session's log.
/// NET-TAKEOVER: how long a replaced connection has to take its close frame
/// before the server hangs up on it anyway. **[A]** Falsified by a client run
/// that shows no connection-lost dialog on a healthy link.
pub const EVICT_GRACE: Duration = Duration::from_secs(2);
const ACCOUNT_ELSEWHERE: &str = "Another client logged in to this account";
const CHARACTER_ELSEWHERE: &str = "Your character logged in elsewhere";

pub const BOUND_CAP: usize = 4096;

/// How long a connection that has not finished the handshake may stay
/// silent (V2 F5).
///
/// **[A]** 30 s. The client answers every handshake step within seconds; the
/// 600 s `conn::IDLE_TIMEOUT` is for a logged-in client between keepalives.
/// Falsifier: a real login over a slow link ending `silent for` before H10.
pub const PREAUTH_TIMEOUT: Duration = Duration::from_secs(30);

/// What to send after handling a payload.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Reply {
    /// Each element becomes one frame, in order.
    pub payloads: Vec<Vec<u8>>,
    /// Frames to **push**: ones the client did not ask for, each with how long
    /// to wait first. `Duration::ZERO` means "in the same flush as
    /// `payloads`".
    ///
    /// **A zero-delay push goes out *after* every payload, and that is a
    /// guarantee rather than an accident of how `conn::serve` is written.**
    /// `conn.rs` clones `payloads` and then *appends* the zero-delay pushes
    /// to the same `send_all`, under one `Wire` lock, so the order on the
    /// wire is: session change (if the arm put one in `payloads`), the
    /// `CallRsp`, then every push the arm queued. Pinned by
    /// `a_zero_delay_push_is_sent_after_the_payloads_in_one_flush`.
    ///
    /// It matters because some client handlers **silently drop** a change
    /// for an item that is not yet in a cache which the in-flight call's own
    /// response fills -- `clientDogmaLocation.OnModuleAttributeChanges`
    /// skips any item where `IsItemLoaded` is false, in one branch with no
    /// log at all, and godma's `ApplyAttributeChange` returns after one
    /// `LogInfo`. A push that arrives before the response it depends on is
    /// not an ordering nit, it is a change that never happened and left
    /// nothing to find.
    ///
    /// A push that must come *before* a response is not expressed here: an
    /// arm that needs that puts the notification in `payloads` itself, which
    /// is what `with_session_change` and `handle_select_character` do.
    ///
    /// A dispatch arm can only *ask* for a push; it cannot perform one, and
    /// there is no handle in scope that would let it. That is the type system
    /// enforcing `docs/reference/world-state.md` §4 rule 4 -- never hold a lock across a
    /// socket write -- in advance of the lock that makes it matter: once
    /// `Registry` sits behind an `RwLock`, an arm runs with that lock held, and
    /// the only code that can touch a socket is `conn::serve` (after the arm
    /// returned) and the push ticker (which never takes the world lock while
    /// holding a `Wire`).
    pub pushes: Vec<(std::time::Duration, Vec<u8>)>,
    /// Close once these are flushed.
    pub close: bool,
}

impl Reply {
    pub fn nothing() -> Self {
        Reply::default()
    }
}

/// A payload a dispatch could not handle.
#[derive(Debug)]
pub enum DispatchError {
    /// The bytes are not a well-formed marshal payload.
    Marshal(WireError),
    /// The payload was well-formed but wrong for this point in the handshake.
    Handshake(HandshakeError),
    /// A packet could not be built or parsed.
    Proto(eve_proto::ProtoError),
}

impl core::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DispatchError::Marshal(e) => write!(f, "marshal: {e}"),
            DispatchError::Handshake(e) => write!(f, "handshake: {e}"),
            DispatchError::Proto(e) => write!(f, "packet: {e}"),
        }
    }
}

impl std::error::Error for DispatchError {}

/// How a connection interprets payloads.
pub trait Dispatch {
    /// Payloads to send on accept, before reading anything. The GPS handshake
    /// is server-initiated, so this is where H1 comes from.
    fn greeting(&mut self) -> Vec<Vec<u8>> {
        Vec::new()
    }

    /// Handle one payload.
    fn on_payload(&mut self, payload: &[u8]) -> Result<Reply, DispatchError>;

    /// A short line for the log, describing where this connection has got to.
    fn state(&self) -> String;

    /// How long `conn::serve` may wait for the peer's next byte right now,
    /// or `None` for its own default. Asked before every read, so a dispatch
    /// can give a connection a short leash until it has logged in (V2 F5).
    fn idle_timeout(&self) -> Option<Duration> {
        None
    }

    /// The largest frame (header included) `conn::serve` may buffer from
    /// the peer right now, or `None` for its own default
    /// (`eve_proto::MAX_FRAME`). Asked before every read: a dispatch that
    /// has not authenticated its peer yet gives it a far lower ceiling
    /// (SEC-FIX 11).
    fn max_frame(&self) -> Option<usize> {
        None
    }

    /// The payload `conn::serve` sends to find out whether a silent peer is
    /// still there (NET-PING), or `None` while there is nobody to ask: before
    /// login, or for a dispatch that does not speak machoNet.
    fn keepalive_ping(&mut self) -> Option<Vec<u8>> {
        None
    }
}

/// Logs and answers nothing.
///
/// For a blind capture: point the client at this, let it talk until it gives
/// up, and read the frames afterwards. Useful precisely when the handshake is
/// suspected of being wrong, since it removes our replies from the picture.
#[derive(Debug, Default)]
pub struct CaptureOnly {
    seen: usize,
}

impl Dispatch for CaptureOnly {
    fn on_payload(&mut self, _payload: &[u8]) -> Result<Reply, DispatchError> {
        self.seen += 1;
        Ok(Reply::nothing())
    }

    fn state(&self) -> String {
        format!("capture-only, {} payloads seen", self.seen)
    }
}

/// Runs the GPS handshake, then observes.
///
/// After H10 the client starts sending `MachoPacket`s. Those now decode — the
/// object wrapper is implemented — but there is no service layer to answer
/// them with, so this dispatch logs and stays quiet.
///
/// It still records every type code it could not read. That is a narrower
/// question than it used to be, and therefore a more useful one: with the
/// wrapper handled, anything that turns up is a genuinely unimplemented code
/// rather than the one we already knew about. A failure is recorded as a
/// finding and the connection is kept open, rather than treated as a fault —
/// hanging up on the first unreadable packet would throw away the capture that
/// makes it readable.
/// The serial for bound-object ids, **shared by the whole process**.
///
/// This was per connection and started at 1, so two clients both held
/// `"N=1:1"` for different objects. Harmless today -- `bound_objects` is
/// per-connection, correctly, because it mirrors the client's own
/// per-session `machoObjectsByID` -- but it made a two-client capture
/// ambiguous at exactly the moment we started taking them, and it becomes a
/// real bug the first time anything is keyed globally by object id.
///
/// Process-global is also what CCP does: `basesession.GetObjectUUID` is
/// `'N=%s:%s' % (nodeID, counter)` over a process-global counter
/// (`docs/reference/bound-objects.md:160-163`). **[inferred]** that the part after the
/// colon may be any string -- the client parses `long(nodeID)` from *before*
/// it and we have not traced what it does with the rest -- but a monotonic
/// integer is safe either way.
///
/// A static rather than an `Arc<AtomicI64>` threaded from `Server`: the
/// property wanted is "unique in this process", which is what a static says,
/// and the plumbing would have put a constructor argument on every call site
/// to express something no caller gets to choose.
static NEXT_OBJECT_ID: AtomicI64 = AtomicI64::new(1);

/// A turn's counters after its arm; see [`crate::live_status::TurnTail`].
#[derive(Debug, Clone, Copy, Default)]
struct TailCounters {
    take: Duration,
    fanout: Duration,
    pushes: u32,
    frames_inline: u32,
    frames_posted: u32,
    bytes: u64,
    sc: u32,
}

pub struct GpsSession {
    handshake: Handshake,
    session: Session,
    /// The world, shared with every other connection.
    ///
    /// **Acquired exactly once per payload, in `on_payload`, and passed down
    /// as `&mut Registry`.** That is not a style choice. The client gives us
    /// no ordering guarantee between a session change and the answer to the
    /// call that caused it (`docs/re/session-and-binding.md` §1), so there is
    /// no protocol excuse for computing the two against different worlds --
    /// and `handle_bind` does exactly that on the undock path if the lock is
    /// taken per-arm. `std::sync::RwLock` is also not reentrant, so a second
    /// acquisition inside an arm would be an instant self-deadlock on this
    /// connection's own thread, which the client shows as a black screen with
    /// nothing in either log.
    ///
    /// The handlers below take `w: &mut Registry` rather than reaching for
    /// this field, which makes "one acquisition" a property of the types
    /// instead of a rule to remember.
    world: Arc<RwLock<Registry>>,
    /// The clock's priority at the world lock (`push::ClockGate`): this
    /// connection holds back before asking for the lock while the clock is
    /// queued for it. `None` in a session built without one (a test).
    gate: Option<Arc<crate::push::ClockGate>>,
    /// Where to write for **other** connections.
    ///
    /// `None` in a `GpsSession` built without one -- a unit test, or capture
    /// mode -- and then a notification addressed to another character is
    /// computed, logged and dropped rather than quietly lost.
    /// `server::serve_one` always supplies it, so the real path always has
    /// it.
    ///
    /// The only field here that can reach a socket which is not this
    /// connection's, which is why `deliver_elsewhere` only ever `post`s to
    /// it and never writes.
    writers: Option<Arc<crate::push::Writers>>,
    /// Tags seen post-handshake that the marshal decoder does not implement,
    /// as `(offset, tag)`, first occurrence of each.
    pub unknown_tags: Vec<(usize, u8)>,
    /// Calls we were asked for and could not answer, as `service.method`.
    /// The most useful diagnostic this server produces: it is the to-do list
    /// for the next milestone, written by the client itself.
    /// Keyed by what was unanswered, valued by how many times. **A count and
    /// not a list**, because the list grew one entry per *occurrence*: a call
    /// the client retries every three seconds appended a line every three
    /// seconds and the end-of-connection report printed all of them. The
    /// count is also the more useful number -- a repeat count is how we found
    /// the live locks that produce no traceback at all.
    pub unanswered: BTreeMap<String, u64>,
    post_handshake_payloads: usize,
    session_sent: bool,
    /// The object id this connection last handed out, for the end-of-run
    /// report only. The serial itself is [`NEXT_OBJECT_ID`].
    last_object_id: i64,
    /// What this turn did after its arm, for the turn trace (TT-A). Reset at
    /// the top of every payload turn.
    tail: TailCounters,
    /// TT-B C1: a `/bots` relay the last call deferred, with its packet and
    /// the index of the placeholder reply it replaces. Run after the lock.
    post_lock: Option<(MachoPacket, crate::bot_control::PostLock, usize)>,
    /// NET-TAKEOVER: connections this turn's login or character select
    /// replaces, as `(client id, text)`. Closed by [`Self::run_evictions`]
    /// once the world lock is released.
    evictions: Vec<(i64, &'static str)>,
    /// Objects we have handed out, by the `objectID` string we gave them, so
    /// a later call on one can be routed back to the service that owns it.
    ///
    /// The client addresses a bound-object call to the **node** and names the
    /// object only by this string -- there is no service in the packet (see
    /// `docs/reference/bound-objects.md` §1.6) -- so without this map such a call
    /// cannot be dispatched at all. `bindParams` is kept beside the service
    /// name because it is what distinguishes two objects of the same service,
    /// and because it belongs in the log line when something goes wrong.
    bound_objects: BTreeMap<String, (String, PyValue)>,
    /// The read timeout until the handshake completes: [`PREAUTH_TIMEOUT`]
    /// unless a test injects a shorter one.
    preauth_timeout: Duration,
    /// Serve the PA-12a read-eligible calls under `world.read()`. On unless
    /// `EVE_READ_TURNS=0`, which forces the old write-lock path (the oracle).
    read_turns: bool,
    /// Payload turns this connection served under the read lock.
    read_turns_served: u64,
    /// Where an object id's serial comes from: [`next_object_serial`], except
    /// in a test that needs two sessions to mint the same ids.
    serial: fn() -> i64,
}

fn next_object_serial() -> i64 {
    NEXT_OBJECT_ID.fetch_add(1, Ordering::Relaxed)
}

/// How [`GpsSession::try_read_turn`] ended.
enum ReadTurn {
    /// Answered under `world.read()`.
    Served(Reply),
    /// Not a read-eligible payload: no lock was taken.
    NotRead,
    /// Read-eligible, but the write path must serve it (queued output, an arm
    /// that declined, a poisoned lock). Nothing was changed.
    Fallback,
}

/// What [`GpsSession::read_route`] decided from the payload alone.
enum ReadRoute<'a> {
    Ping,
    /// The client's answer to our keepalive ping: nothing to do (NET-PING).
    PingRsp,
    Notice,
    /// A service call. `mint`: it is a handle-getter (`OBJECT_RETURNING`).
    Service {
        service: &'a str,
        method: &'a str,
        args: &'a [PyValue],
        kwargs: &'a [(PyValue, PyValue)],
        mint: Option<&'static str>,
    },
    /// A call on an object this session handed out.
    Bound {
        service: String,
        bind_params: PyValue,
        object_id: &'a str,
        method: &'a str,
        args: &'a [PyValue],
        kwargs: &'a [(PyValue, PyValue)],
        mint: Option<&'static str>,
    },
}

/// A read turn's answer before encoding: built under the guard, or held
/// raw so it is encoded after the guard drops (PERF-BURST).
enum Staged<'r> {
    Ready(Reply),
    Late {
        req: Request<'r>,
        result: Result<RoValue, ServiceError>,
        why: String,
    },
}

/// What the shared step after a dispatch decided (see
/// [`GpsSession::answer_call`]).
enum Answered {
    Value(PyValue),
    Reply(Reply),
}

/// A payload decoded **before** the world lock is taken (V2 F1).
///
/// Inflating and decoding a payload is the expensive part of a turn, and it
/// needs nothing from the world, so [`GpsSession::decode`] does it with no
/// lock held and [`GpsSession::handle_inbound`] takes this, never bytes. A
/// client that sends a large or slow-to-decode payload, before login or
/// after it, then costs its own thread and no one else's.
#[derive(Debug)]
enum Inbound {
    /// A handshake step, before `Phase::Authenticated`.
    Handshake(PyValue),
    /// After login: a type code the decoder does not implement.
    UnknownTag { at: usize, tag: u8 },
    /// After login: bytes that are not a marshal stream.
    Undecodable(WireError),
    /// After login: a value that is not a packet we model.
    Unparsed(eve_proto::ProtoError),
    /// After login: a packet, and its inner call decoded too when it is a
    /// `CallReq` or a `Notification`.
    Packet(Box<Decoded>),
}

impl Inbound {
    /// `service.method` for `[turns]`' slowest call (CP15); a bound call
    /// names its object's service. `None` when the payload is not a call.
    fn label(&self) -> Option<String> {
        let Inbound::Packet(d) = self else {
            return None;
        };
        let service = d.packet.destination.service().unwrap_or("bound");
        match d.call.as_ref()?.as_ref().ok()? {
            CallPayload::Service { method, .. } | CallPayload::BoundObject { method, .. } => {
                Some(format!("{service}.{method}"))
            }
            _ => None,
        }
    }
}

#[derive(Debug)]
struct Decoded {
    packet: MachoPacket,
    /// `None` when the command carries no call, or the body is empty.
    call: Option<Result<CallPayload, eve_proto::ProtoError>>,
}

impl GpsSession {
    /// **No store**: a session never saves (CP52). An arm still cannot reach
    /// the store -- it may record that something happened and may not
    /// perform I/O -- and now neither can the turn around it, which keeps
    /// "no store I/O on the draw path" a property of the types.
    pub fn new(handshake: Handshake, session: Session, world: Arc<RwLock<Registry>>) -> Self {
        GpsSession {
            handshake,
            session,
            world,
            gate: None,
            writers: None,
            last_object_id: 0,
            tail: TailCounters::default(),
            post_lock: None,
            evictions: Vec::new(),
            bound_objects: BTreeMap::new(),
            unknown_tags: Vec::new(),
            unanswered: BTreeMap::new(),
            post_handshake_payloads: 0,
            session_sent: false,
            preauth_timeout: PREAUTH_TIMEOUT,
            read_turns: std::env::var("EVE_READ_TURNS").map_or(true, |v| v != "0"),
            read_turns_served: 0,
            serial: next_object_serial,
        }
    }

    /// Serve read-eligible calls under `world.read()` (`true`) or force the
    /// write path (`false`); overrides `EVE_READ_TURNS`.
    pub fn with_read_turns(mut self, on: bool) -> Self {
        self.read_turns = on;
        self
    }

    /// How many payload turns this connection served under the read lock.
    pub fn read_turns_served(&self) -> u64 {
        self.read_turns_served
    }

    /// Test seam: a fixed serial source, so two sessions can mint the same
    /// object ids.
    #[cfg(test)]
    fn with_serial(mut self, serial: fn() -> i64) -> Self {
        self.serial = serial;
        self
    }

    /// Give this session the clock's gate (`push::ClockGate`); without one
    /// it never holds back.
    pub fn with_gate(mut self, gate: Option<Arc<crate::push::ClockGate>>) -> Self {
        self.gate = gate;
        self
    }

    /// Hold back while the clock is queued for the world lock; the call every
    /// payload turn makes just before it asks for the lock.
    fn wait_for_clock(&self) {
        if let Some(g) = &self.gate {
            g.wait_for_clock();
        }
    }

    /// Give this session the process-wide writer registry, so a
    /// notification whose audience is another character can reach them.
    pub fn with_writers(mut self, writers: Arc<crate::push::Writers>) -> Self {
        self.writers = Some(writers);
        self
    }

    /// Replace [`PREAUTH_TIMEOUT`] for this connection: tests inject a short
    /// one so they do not wait 30 s.
    pub fn with_preauth_timeout(mut self, timeout: Duration) -> Self {
        self.preauth_timeout = timeout;
        self
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// This connection as a [`Caller`], for [`Registry::dispatch_as`]:
    /// every client payload is dispatched as its session's user and role
    /// (`docs/lanes/account-spec.md` §3.5), which is what the GM gate in
    /// `service/gm.rs` reads.
    fn caller(&self) -> Caller {
        Caller::Session {
            user_id: self.session.user_id,
            role: self.session.role,
        }
    }

    /// The `SessionInitialStateNotification` that follows H10.
    ///
    /// Addressing, **inferred and worth validating against the first
    /// capture**: `source` is a node address carrying our `proxy_nodeid` and
    /// `destination` is the client. That is what `docs/reference/login-path.md` records
    /// the real proxy doing, and `machoNetTransport.LockSession` calls
    /// `WaitForSequenceNumber(packet.source, ...)` for every even-numbered
    /// command, so the source has to be an address the client will accept as
    /// a node.
    /// **`machoNet.InitClientGlobalConfig(config)` -- the first call this
    /// server has ever made.**
    ///
    /// Everything else we send is a reply or a notification. This is a
    /// `CALL_REQ` addressed to a service *on the client*, which
    /// `machoNet.__clientallowedcommands__` permits and
    /// `ServiceCallGPCS.CallUp` gates only on a `__restrictedcalls__` deny
    /// list that machoNet does not have -- `docs/port-tracker.md` §41.4 and
    /// §41.5 have the derivation.
    ///
    /// **Why send it at all.** On a client `machoNet.GetGlobalConfig` is
    /// `elif macho.mode == 'client' and self.globalConfig is None: return {}`
    /// **[V]**, so until this arrives *every* `globalConfig` flag sits at its
    /// compiled-in default. That is why the client asks `essMgr` for ESS data
    /// we do not have: `ess_disabled` defaults to `False`. Setting it is
    /// CCP's own off switch, which beats answering `None` and hoping -- the
    /// client then takes its own documented path
    /// (`dynamicResourceSvc.GetESSDataForCurrentSystem` returns `None` by
    /// itself) and never asks.
    ///
    /// **The dict is built value-first.** A marshal dict pair is
    /// `(value, key)`, and `pyutil::pypairs` is the one place that inversion
    /// is written down -- §39 is what happens when it is spelled out a second
    /// time and got backwards.
    ///
    /// The `call_id` is ours, not the client's, so it comes from a range
    /// nothing else uses; the client answers with a `CALL_RSP` carrying it
    /// back, which [`Self::handle_call_response`] absorbs.
    fn session_init_packet(&self) -> Result<Vec<u8>, DispatchError> {
        let packet = MachoPacket::session_initial_state(
            // The *node* id, not the session id. These were the same value by
            // coincidence -- both 1 -- and this line read `session_id` until
            // bound objects made the distinction matter: an object id names
            // the node that owns it, and the client addresses every later
            // call on that object to it.
            MachoAddress::node(self.handshake.node_id()),
            MachoAddress::client(self.session.client_id),
            self.session.session_id,
            self.session.session_type,
            self.session.initial_state(),
        );
        packet.to_payload().map_err(DispatchError::Proto)
    }

    /// Answer a `CallReq`, or record that we could not.
    ///
    /// `decoded` was built by [`Self::decode`] with no lock held, inner call
    /// pickle included; nothing here decodes bytes.
    fn handle_packet(
        &mut self,
        w: &mut Registry,
        decoded: Decoded,
    ) -> Result<Reply, DispatchError> {
        let Decoded { packet, call } = decoded;
        if packet.command == MachoNetMsg::PingReq {
            return self.handle_ping(&packet);
        }
        // The answer to our keepalive (`Dispatch::keepalive_ping`): arriving
        // at all is the whole message. Not an unanswered command.
        if packet.command == MachoNetMsg::PingRsp {
            return Ok(Reply::nothing());
        }
        if packet.command == MachoNetMsg::Notification {
            return self.handle_notification(call);
        }
        if packet.command == MachoNetMsg::CallRsp {
            return self.handle_call_response(&packet);
        }
        // **The client refusing or failing a call of ours.** Observed on the
        // first real client run of §41's `InitClientGlobalConfig`: the client
        // answered with an `ErrorResponse`, not a `CallRsp`, and this arm
        // exists to say *why* instead of counting it as unanswered. The body
        // carries the macho error code and whatever the client wrapped --
        // printed verbatim, because a code we cannot name is still evidence.
        if packet.command == MachoNetMsg::ErrorResponse {
            crate::outln!(
                "[svc] the client REFUSED our call (id {}): {:?}",
                packet
                    .destination
                    .call_id()
                    .or_else(|| packet.source.call_id())
                    .map_or_else(|| "none".to_owned(), |id| id.to_string()),
                packet.body
            );
            return Ok(Reply::nothing());
        }
        if packet.command != MachoNetMsg::CallReq {
            self.note_unanswered(format!("unhandled command {:?}", packet.command));
            return Ok(Reply::nothing());
        }

        let service = packet.destination.service().unwrap_or("").to_owned();
        // **Every parsed `CallReq` gets an answer, failures included** (V2
        // F6). The header parsed, so the call id is known, and a `CallReq`
        // left unanswered is a client tasklet blocked for good with nothing
        // in its log. These used to answer nothing (an undecodable payload)
        // or disconnect (no payload at all); now each is the same
        // `MACHONETERR_WRAPPEDEXCEPTION` an arm's raise produces, so the
        // client raises, keeps going, and logs what we could not read.
        let call = match call {
            Some(Ok(c)) => c,
            Some(Err(e)) => {
                return self.refuse_call(&packet, format!("undecodable call payload: {e}"));
            }
            None => return self.refuse_call(&packet, "CallReq has no payload".to_owned()),
        };
        let (method, args, kwargs) = match &call {
            CallPayload::Service {
                method,
                args,
                kwargs,
            } => (method, args, kwargs),
            CallPayload::BoundObject {
                object_id,
                method,
                args,
                kwargs,
            } => {
                return self.handle_bound_call(w, &packet, object_id, method, args, kwargs);
            }
            // A release arriving as a CallReq would mean the client expects an
            // answer to it, which `__ReleaseObjectHelper` never does. Record
            // it rather than guess: it would be news about the protocol.
            CallPayload::Release { .. } => {
                return self.refuse_call(
                    &packet,
                    "release sent as a CallReq, not a Notification".to_owned(),
                );
            }
        };

        // `MachoBindObject` is not a service method, it is the bind protocol
        // wearing a service call's clothes -- see `docs/reference/bound-objects.md`.
        if method == "MachoBindObject" {
            return self.handle_bind(w, &packet, &service, args);
        }
        // Selecting a character is not a service method either: its *effect*
        // is a session change, and the return value is discarded.
        if service == "charUnboundMgr" && method == "SelectCharacterID" {
            return self.handle_select_character(w, &packet, args);
        }
        // Logging a character off is a session change too: the character's
        // keys go, and the connection goes back to the select screen.
        if service == "userSvc" && method == "UserLogOffCharacter" {
            return self.handle_log_off(w, &packet);
        }
        // And a few plain calls return a live object rather than data.
        let req = Request {
            service: &service,
            method,
            args,
            kwargs,
            bind_params: None,
            actor: self.session.character_id,
        };
        // Built before the check so the object-returning path can dispatch
        // the very same request rather than reassemble it.
        if let Some(bound) = Registry::object_returning(&service, method) {
            return self.handle_object_returning_call(w, &packet, &req, bound);
        }
        // TT-B C1: only the slash service may defer a relay past the lock.
        let slash = service == "slash";
        if slash {
            crate::bot_control::allow_deferral(w, true);
        }
        let dispatched = w.dispatch_as(self.caller(), &req);
        let parked = if slash {
            crate::bot_control::allow_deferral(w, false);
            crate::bot_control::take_post_lock(w)
        } else {
            None
        };
        let result = match self.answer_call(&packet, &req, dispatched, ": ")? {
            Answered::Value(v) => v,
            Answered::Reply(reply) => return Ok(reply),
        };

        // Built by `MachoPacket::respond_to`, which mirrors the client's own
        // `Response()`: the addresses are swapped, not invented, and the trace
        // fields are carried over.
        // The return value is carried as a *pickle*: the client hands a
        // response payload straight to `blue.marshal.Load` (see
        // `eve_proto::response_payload`). A `PingRsp` is different -- it is
        // handled in `machoNet.HandleMessage` without going through
        // `ObjectCall.CallDown`, so its `times` stay a live list.
        let payloads =
            self.with_session_change(w, &service, method, Self::call_rsp(&packet, &result)?)?;
        if let Some(p) = parked {
            // The placeholder is the last payload (`with_session_change`).
            self.post_lock = Some((packet.clone(), p, payloads.len() - 1));
        }
        Ok(Reply {
            payloads,
            pushes: Vec::new(),
            close: false,
        })
    }

    /// NET-TAKEOVER: send each replaced connection its `GPSTransportClosed`
    /// (`CLIENTDISCONNECTED`) and let `Writers::evict` hang it up after the
    /// grace. After the world lock; the writers take no wire lock.
    fn run_evictions(&mut self) {
        let Some(writers) = self.writers.clone() else {
            self.evictions.clear();
            return;
        };
        for (client_id, text) in std::mem::take(&mut self.evictions) {
            let payload = dumps(&crate::account_book::transport_closed(
                text,
                Some("CLIENTDISCONNECTED"),
            ));
            let sent = writers.evict(client_id, payload, EVICT_GRACE);
            crate::outln!(
                "[login] client {} closed client {client_id} ({text}){}",
                self.session.client_id,
                if sent { "" } else { ": already gone" }
            );
        }
    }

    /// Run a deferred `/bots` relay now that the world lock is free, and put
    /// its answer where the placeholder reply is (TT-B C1).
    fn finish_post_lock(&mut self, reply: &mut Reply) -> Result<(), DispatchError> {
        let Some((packet, p, idx)) = self.post_lock.take() else {
            return Ok(());
        };
        let started = Instant::now();
        let answer = p.finish();
        crate::outln!(
            "[bots] relay {:.1} ms (off the world lock)",
            started.elapsed().as_secs_f64() * 1000.0
        );
        let payload = match answer {
            Ok(v) => MachoPacket::respond_to(
                &packet,
                MachoNetMsg::CallRsp,
                vec![eve_proto::response_payload(&v)],
            )
            .to_payload()
            .map_err(DispatchError::Proto)?,
            Err(ServiceError::Raise(e)) => {
                crate::outln!(
                    "[svc] slash.SlashCmd/1 as {} raises UserError (by design)",
                    self.session.character_id.unwrap_or(0)
                );
                Self::error_reply(&packet, &e)?
                    .payloads
                    .pop()
                    .unwrap_or_default()
            }
            Err(e) => {
                crate::errln!("[svc] slash.SlashCmd after the lock: {e}");
                return Ok(());
            }
        };
        if let Some(slot) = reply.payloads.get_mut(idx) {
            *slot = payload;
        }
        Ok(())
    }

    /// **Classify with no lock** (PA-12a): may this payload be served under
    /// `world.read()`? Pure; reads only `self.bound_objects`. `None` = no
    /// (the write path serves it, and no lock was taken to find out).
    fn read_route<'a>(&self, decoded: &'a Decoded) -> Option<ReadRoute<'a>> {
        let packet = &decoded.packet;
        match packet.command {
            MachoNetMsg::PingReq => return Some(ReadRoute::Ping),
            MachoNetMsg::PingRsp => return Some(ReadRoute::PingRsp),
            MachoNetMsg::Notification => return Some(ReadRoute::Notice),
            MachoNetMsg::CallReq => {}
            _ => return None,
        }
        let service = packet.destination.service().unwrap_or("");
        match decoded.call.as_ref()?.as_ref().ok()? {
            CallPayload::Service {
                method,
                args,
                kwargs,
            } => {
                // The special calls of `handle_packet`, in its order.
                if method == "MachoBindObject"
                    || (service == "charUnboundMgr" && method == "SelectCharacterID")
                    || (service == "userSvc" && method == "UserLogOffCharacter")
                {
                    return None;
                }
                if !Registry::read_eligible(service, method) {
                    return None;
                }
                let mint = Registry::object_returning(service, method);
                Some(ReadRoute::Service {
                    service,
                    method,
                    args,
                    kwargs,
                    mint,
                })
            }
            CallPayload::BoundObject {
                object_id,
                method,
                args,
                kwargs,
            } => {
                let (service, bind_params) = self.bound_objects.get(object_id)?.clone();
                let mint = Registry::object_returning(&service, method);
                if mint.is_none() && !Registry::read_eligible(&service, method) {
                    return None;
                }
                Some(ReadRoute::Bound {
                    service,
                    bind_params,
                    object_id,
                    method,
                    args,
                    kwargs,
                    mint,
                })
            }
            CallPayload::Release { .. } => None,
        }
    }

    /// **The read turn** (PA-12a): serve a read-eligible payload under
    /// `world.read()`, so it neither waits for nor holds up the clock's and
    /// the writers' exclusive turns beyond a shared hold.
    ///
    /// Byte identity with the write path rests on one precondition, checked
    /// under the guard: nothing is queued for the turn to drain
    /// ([`Registry::turn_output_pending`]); a read turn cannot drain, the
    /// write path would have put it in this reply. Otherwise
    /// [`ReadTurn::Fallback`], and the write path replays the request from
    /// the same session state: **every early return is before the first
    /// `self` mutation** (the mint, the counters, `note_unanswered`), which
    /// all sit after the last point that can decline.
    fn try_read_turn(
        &mut self,
        inbound: &Inbound,
        label: Option<&str>,
        asked: Instant,
    ) -> Result<ReadTurn, DispatchError> {
        let Inbound::Packet(decoded) = inbound else {
            return Ok(ReadTurn::NotRead);
        };
        let Some(route) = self.read_route(decoded) else {
            return Ok(ReadTurn::NotRead);
        };
        let world = Arc::clone(&self.world);
        self.wait_for_clock();
        // A poisoned lock goes to the write path, which taints.
        let waiting = crate::clock_probe::Waiter::enter();
        let Ok(w) = world.read() else {
            return Ok(ReadTurn::Fallback);
        };
        drop(waiting);
        let acquired = Instant::now();
        if w.turn_output_pending() {
            return Ok(ReadTurn::Fallback);
        }
        let packet = &decoded.packet;
        let actor = self.session.character_id;
        let staged = match &route {
            ReadRoute::Ping => {
                self.post_handshake_payloads += 1;
                Staged::Ready(self.handle_ping(packet)?)
            }
            ReadRoute::PingRsp => {
                self.post_handshake_payloads += 1;
                Staged::Ready(Reply::nothing())
            }
            ReadRoute::Notice => {
                self.post_handshake_payloads += 1;
                Staged::Ready(self.handle_notification(decoded.call.clone())?)
            }
            ReadRoute::Service {
                service,
                method,
                args,
                kwargs,
                mint,
            } => {
                let req = Request {
                    service,
                    method,
                    args,
                    kwargs,
                    bind_params: None,
                    actor,
                };
                let Some(result) = w.dispatch_ro(self.caller(), &req) else {
                    return Ok(ReadTurn::Fallback);
                };
                if let Some(bound) = *mint {
                    // A handle-getter: the answer is thrown away, and
                    // `NoSuchMethod` is the expected one.
                    if !matches!(result, Ok(_) | Err(ServiceError::NoSuchMethod { .. })) {
                        return Ok(ReadTurn::Fallback);
                    }
                    self.post_handshake_payloads += 1;
                    let reference = self.mint_object(bound, PyValue::None);
                    Staged::Ready(Reply {
                        payloads: vec![Self::call_rsp(packet, &reference)?],
                        pushes: Vec::new(),
                        close: false,
                    })
                } else {
                    self.post_handshake_payloads += 1;
                    Staged::Late {
                        req,
                        result,
                        why: ": ".to_owned(),
                    }
                }
            }
            ReadRoute::Bound {
                service,
                bind_params,
                object_id,
                method,
                args,
                kwargs,
                mint,
            } => {
                if let Some(bound) = *mint {
                    self.post_handshake_payloads += 1;
                    let reference = self.mint_object(bound, PyValue::Tuple(args.to_vec()));
                    Staged::Ready(Reply {
                        payloads: vec![Self::call_rsp(packet, &reference)?],
                        pushes: Vec::new(),
                        close: false,
                    })
                } else {
                    let req = Request {
                        service,
                        method,
                        args,
                        kwargs,
                        bind_params: Some(bind_params),
                        actor,
                    };
                    let Some(result) = w.dispatch_ro(self.caller(), &req) else {
                        return Ok(ReadTurn::Fallback);
                    };
                    self.post_handshake_payloads += 1;
                    let why = format!(" on {object_id} {bind_params:?}: ");
                    Staged::Late { req, result, why }
                }
            }
        };
        let hold = acquired.elapsed();
        let wait = acquired.saturating_duration_since(asked);
        crate::live_status::prof::payload_turn(
            label,
            wait,
            hold,
            crate::live_status::prof::TurnKind::Read,
        );
        crate::live_status::record_turn(
            &w,
            crate::live_status::Turn {
                wait,
                hold,
                label,
                read: true,
                ..Default::default()
            },
        );
        drop(w);
        // PERF-BURST: a call's answer is encoded with the guard gone, and a
        // `Shared` one straight from its `Arc`.
        let payload = match staged {
            Staged::Ready(reply) => reply,
            Staged::Late { req, result, why } => match result {
                Ok(RoValue::Shared(shared)) => Reply {
                    payloads: vec![Self::call_rsp(packet, &shared)?],
                    pushes: Vec::new(),
                    close: false,
                },
                other => {
                    match self.answer_call(packet, &req, other.map(RoValue::into_owned), &why)? {
                        Answered::Value(v) => Reply {
                            payloads: vec![Self::call_rsp(packet, &v)?],
                            pushes: Vec::new(),
                            close: false,
                        },
                        Answered::Reply(reply) => reply,
                    }
                }
            },
        };
        self.read_turns_served += 1;
        Ok(ReadTurn::Served(payload))
    }

    /// What every dispatched call does with its result, whichever path ran
    /// it: a value goes on to the `CallRsp`; a raise is the
    /// `ErrorResponse` the client re-raises (this method is specified to
    /// raise, and the client reads the answer out of the exception, so a
    /// `CallRsp` -- even a correct-looking one -- is the wrong packet
    /// entirely); anything else is logged `UNANSWERED` and answers `None`.
    /// `why` leads the error text in that log line.
    fn answer_call(
        &mut self,
        packet: &MachoPacket,
        req: &Request<'_>,
        result: Result<PyValue, ServiceError>,
        why: &str,
    ) -> Result<Answered, DispatchError> {
        match result {
            Ok(v) => Ok(Answered::Value(v)),
            Err(ServiceError::Raise(exception)) => {
                self.raised(packet, req, &exception).map(Answered::Reply)
            }
            Err(e) => {
                let what = Registry::describe(req);
                Ok(Answered::Value(self.unhandled(what, &format!("{why}{e}"))))
            }
        }
    }

    /// The `CallRsp` payload for `result`, built by `MachoPacket::respond_to`,
    /// which mirrors the client's own `Response()`: the addresses are
    /// swapped, not invented, and the trace fields are carried over. The
    /// value rides as a *pickle*: the client hands a response payload
    /// straight to `blue.marshal.Load` (see `eve_proto::response_payload`).
    fn call_rsp(packet: &MachoPacket, result: &PyValue) -> Result<Vec<u8>, DispatchError> {
        MachoPacket::respond_to(
            packet,
            MachoNetMsg::CallRsp,
            vec![eve_proto::response_payload(result)],
        )
        .to_payload()
        .map_err(DispatchError::Proto)
    }

    /// Answer a `PingReq`.
    ///
    /// A ping is **annotated, not acknowledged**: every hop appends one
    /// `(wallclock, wallclock-now, label)` triple to the list it received and
    /// passes the whole list on. The client seeds it with `client::start` and
    /// reads the differences back out, so dropping the list it sent — or
    /// answering with a fresh one — throws away the measurement it asked for.
    ///
    /// **Which labels, and in which order, is not a free choice.**
    /// `MachoNetService.Ping` ends with, unconditionally:
    ///
    /// ```text
    /// server1 = [p[0].strip() for p in prepared].index('proxy::writing')
    /// server2 = [p[0].strip() for p in prepared].index('proxy::handle_message', server1)
    /// ```
    ///
    /// so `proxy::writing` must be present and `proxy::handle_message` must
    /// follow it. We first answered with a single `proxy::turnaround`, and the
    /// client raised `ValueError: 'proxy::writing' is not in list` inside its
    /// login-progress stats sampler.
    ///
    /// The order looks strange until you see where those labels come from:
    ///
    /// - `MachoNetService.HandleMessage` appends `<mode>::handle_message` to
    ///   every ping request or response it receives.
    /// - `MachoNetService.FragileWrite` appends `<mode>::writing` to every
    ///   ping it forwards.
    /// - The `PING_REQ` branch of `HandleMessage` appends `<mode>::turnaround`
    ///   and replies.
    ///
    /// On a real cluster the client's ping is routed **through** the proxy to
    /// a server node, so `proxy::writing` is the proxy forwarding it onward
    /// and the later `proxy::handle_message` is the proxy taking the server's
    /// answer — the pair measures the server leg, which is why the client
    /// names them `server1`/`server2`. A proxy that answered a ping by itself
    /// would never emit `proxy::writing` at all, which is exactly the list we
    /// were sending.
    ///
    /// We are one process standing in for both nodes, so we reproduce the
    /// chain both of them would have written. The timestamps are all the same
    /// instant, and that is the truth rather than a placeholder: there is no
    /// network between our "proxy" and our "server".
    /// Mint an object reference and register it so later calls on it route.
    ///
    /// `bound` is the synthetic service its methods live under. One function
    /// for all three ways an object can leave here -- a bind, a fused call
    /// that returns an object, and a plain call that returns one -- because
    /// the id format is load-bearing (`docs/reference/bound-objects.md` §1.5) and one
    /// copy of it is enough.
    /// Record a call we had no real answer for, and answer `None` anyway.
    ///
    /// **This never returns silence, and that is the whole point.** An
    /// unanswered `CallReq` is a hang, not a no-op:
    /// `RemoteServiceCallWithoutTheStars` blocks its tasklet with no
    /// client-side timeout that recovers into a drawn screen, and the client
    /// logs nothing, because nothing threw. Every such call used to cost a
    /// full round trip with a human -- launch, hang, read the log, implement
    /// one method, launch again -- and the log line below was the only thing
    /// that made it diagnosable at all.
    ///
    /// `None` is strictly better than nothing, by the corollary in
    /// `docs/guides/debugging-the-client.md` section 1: a call answered *wrongly*
    /// usually produces a traceback, which is evidence, while a call not
    /// answered produces silence. So the client either tolerates the `None`
    /// and keeps going -- revealing the *next* missing method in the same run
    /// -- or throws, and names the shape it wanted in its own log.
    ///
    /// Neither outcome is a fix, and the `[svc] UNANSWERED` line still says
    /// so in as many words. It stays on `self.unanswered` so the end-of-
    /// session summary and the guard tests keep seeing it.
    /// Record one occurrence of something we did not answer.
    ///
    /// **At most [`UNANSWERED_KEYS`] distinct keys, plus
    /// [`UNANSWERED_OTHER`]** (V2 F7). A key already present keeps
    /// counting; a new one past the cap is counted under the fold, so the
    /// map stops growing while the total stays exact.
    fn note_unanswered(&mut self, what: String) {
        let key = if self.unanswered.contains_key(&what) || self.unanswered.len() < UNANSWERED_KEYS
        {
            what
        } else {
            UNANSWERED_OTHER.to_owned()
        };
        *self.unanswered.entry(key).or_insert(0) += 1;
    }

    /// A call whose arm raised: the `MACHONETERR_WRAPPEDEXCEPTION`
    /// `ErrorResponse` the client re-raises, whichever route the call came
    /// by -- service, bound object, or fused onto a bind.
    fn raised(
        &self,
        packet: &MachoPacket,
        req: &Request<'_>,
        exception: &PyValue,
    ) -> Result<Reply, DispatchError> {
        crate::outln!(
            "[svc] {} raises UserError (by design)",
            Registry::describe(req)
        );
        Self::error_reply(packet, exception)
    }

    /// The `ErrorResponse` answering `packet` with `exception`, addressed by
    /// swapping the request's addresses, so it carries its call id back.
    fn error_reply(packet: &MachoPacket, exception: &PyValue) -> Result<Reply, DispatchError> {
        let rsp = MachoPacket::error_response(
            packet.destination.clone(),
            packet.source.clone(),
            MachoNetMsg::CallReq,
            MACHONETERR_WRAPPEDEXCEPTION,
            eve_proto::wrapped_exception_payload(exception),
        );
        Ok(Reply {
            payloads: vec![rsp.to_payload().map_err(DispatchError::Proto)?],
            pushes: Vec::new(),
            close: false,
        })
    }

    /// Answer a `CallReq` we cannot even route with an error, never silence
    /// (V2 F6).
    ///
    /// The exception is `CustomNotify`, the server's generic escape hatch
    /// (`jump.rs` uses it the same way). **[A]** that the client shows it as
    /// a notice and carries on; what is known is that `ErrorResponse` code 2
    /// makes it raise rather than wait. Falsifier: a client log showing a
    /// hang or a crash after one of these lines.
    fn refuse_call(&mut self, packet: &MachoPacket, why: String) -> Result<Reply, DispatchError> {
        crate::outln!(
            "[svc] UNANSWERABLE call (id {}): {why} -- answering with an error",
            packet
                .source
                .call_id()
                .or_else(|| packet.destination.call_id())
                .map_or_else(|| "none".to_owned(), |id| id.to_string())
        );
        let exception = eve_proto::user_error(
            "CustomNotify",
            crate::pyutil::pypairs(vec![(
                "notify",
                crate::pyutil::bstr(&format!("The server could not read this call: {why}")),
            )]),
        );
        self.note_unanswered(why);
        Self::error_reply(packet, &exception)
    }

    fn unhandled(&mut self, what: String, why: &str) -> PyValue {
        // Say it NOW, not when the connection closes. A client that is stuck
        // never closes the connection, so a summary printed from `state()`
        // was never reached -- which is how the answer sat unread in this
        // list for two separate debugging rounds.
        crate::outln!("[svc] UNANSWERED {what}{why} -- answering None so the client keeps going");
        self.note_unanswered(what);
        PyValue::None
    }

    /// A `Notification` the client sends us, which by definition wants no
    /// answer.
    ///
    /// In practice there is exactly one, and it arrives once per session,
    /// about thirty seconds after the UI settles: the client telling us which
    /// bound objects it has dropped. Everything about it is a special case.
    /// Its source is a node address with **no node id** -- the client is not a
    /// node, so the field is `None` -- which our decoder rejected outright,
    /// and the packet was logged as `unparsed packet: packet shape: nodeID`
    /// and thrown away. Its payload then keeps the bound-object shape while
    /// putting a *list* where an objectID belongs.
    ///
    /// Dropping it was harmless in the sense that nothing hangs: a
    /// notification is non-blocking at both ends. It was not harmless as
    /// bookkeeping -- we hold every object we ever minted -- and it was
    /// actively misleading, because an unreadable packet in the log reads
    /// like a decoder that cannot keep up with the client.
    /// **The client answering a call *we* made.**
    ///
    /// Nothing in this server has ever been the caller, so this arm is the
    /// prerequisite for the first thing that is -- §41's
    /// `InitClientGlobalConfig`, which has to be a `CALL_REQ` to the client's
    /// `machoNet` because it is a plain method and not a notify event.
    ///
    /// **Written before the call that needs it, deliberately.** Without this
    /// arm the response falls through to `unhandled command CallRsp` and the
    /// server prints an `UNANSWERED` line on every login -- which would be a
    /// poor trade for the one `UNANSWERED` line §41 removes, and the kind of
    /// self-inflicted noise `note_unanswered`'s own doc warns about.
    ///
    /// There is nothing to *do* with the answer. A `CALL_REQ` we send carries
    /// a call id, the client replies with it, and the reply is an
    /// acknowledgement: the config either arrived or the client is not
    /// listening, and either way nothing downstream waits on it. So this
    /// records the arrival at normal volume and returns -- the alternative,
    /// swallowing it silently, would make a client that answers and a client
    /// that does not look identical in the log.
    fn handle_call_response(&mut self, packet: &MachoPacket) -> Result<Reply, DispatchError> {
        crate::outln!(
            "[svc] client answered our call (id {})",
            packet
                .destination
                .call_id()
                .or_else(|| packet.source.call_id())
                .map_or_else(|| "none".to_owned(), |id| id.to_string())
        );
        Ok(Reply::nothing())
    }

    fn handle_notification(
        &mut self,
        call: Option<Result<CallPayload, eve_proto::ProtoError>>,
    ) -> Result<Reply, DispatchError> {
        let Some(call) = call else {
            return Ok(Reply::nothing());
        };
        match call {
            Ok(CallPayload::Release { objects }) => {
                let mut freed = Vec::new();
                for (object_id, _ref_id) in &objects {
                    if self.bound_objects.remove(object_id).is_some() {
                        freed.push(object_id.clone());
                    }
                }
                crate::outln!(
                    "[svc] client released {} bound object(s): {}",
                    freed.len(),
                    freed.join(", ")
                );
                Ok(Reply::nothing())
            }
            // Any other notification: say what it was rather than that it
            // was a notification. We have never seen one, and the day we do
            // the method name is the whole of the news.
            Ok(other) => {
                self.note_unanswered(format!("unhandled notification {other:?}"));
                Ok(Reply::nothing())
            }
            Err(e) => {
                self.note_unanswered(format!("undecodable notification payload: {e}"));
                Ok(Reply::nothing())
            }
        }
    }

    /// A session change a service call is **defined by**, rather than
    /// answered with.
    ///
    /// `charUnboundMgr.SelectCharacterID` was the first of these and got its
    /// own function, because it is a plain service call whose reply is a bare
    /// `None`. `ship.Undock` is the second and cannot reuse that shape: it
    /// arrives as a *fused* `MachoBindObject`, so its reply is the
    /// `(boundObject, retval)` tuple that `handle_bind` builds. What the two
    /// have in common is only the **ordering** — notification first, answer
    /// second — so that is all this hook does.
    /// Notifications a **bind** owes the client, keyed on its fused call.
    ///
    /// The sibling of [`Self::session_change_for`], and it exists for the
    /// same reason: a bind arrives as `MachoBindObject` and the method the
    /// client wanted is fused onto it, so nothing keyed on the method name
    /// can be queued from a dispatch arm -- the arm for that method is never
    /// entered. Anything that must follow, say, `dogmaIM.GetAllInfo` has to
    /// be queued from here.
    ///
    /// **Empty today, and that is the honest state of it.** It is not
    /// speculative plumbing: `docs/lanes/notify-spec.md` §3.4 names fitting
    /// and implants as the two Round 1 subsystems that need it, because
    /// theirs are the notifications that follow a `dogmaIM` bind. Everything
    /// else in Round 1 -- skills, market, industry, corporations, wallet,
    /// fleets -- mutates through ordinary calls whose arms can queue
    /// directly, and should keep doing that rather than route through here.
    ///
    /// EvEJS has to dig `args[1][0]` out of the bind to recover the method
    /// (`dogmaService.js:4266-4281`); `handle_bind` already hoists it as
    /// `fused_method` for `session_change_for`, so we get it for nothing.
    ///
    /// Queues onto `Registry::pending`, which drains **after** the response
    /// (see [`Reply::pushes`]) -- which is the whole point, since the client
    /// handlers these would feed silently drop a change for an item the
    /// in-flight response has not loaded yet.
    fn pending_for_bound_call(&mut self, w: &mut Registry, service: &str, method: &str) {
        // **The undocked tier's entry point, and the first user of this
        // function.** `beyonce.MachoBindObject(solarSystemID, None)` carries
        // no fused call, so `fused_method` is the empty string -- and this
        // bind is the first moment at which `michelle.DoDestinyUpdate` does
        // anything at all, because the ballpark it needs does not exist until
        // a few seconds after the undock session change.
        // `crate::destiny::on_ballpark_bind` has the evidence; the one-line
        // delegation is the point, so that everything else about Destiny
        // lives in a file a lane can own.
        //
        // One arm, so `if let` rather than a `match`: the doc comment above
        // says which Round 1 subsystems will add their own here, and which
        // must not -- a push keyed on an ordinary method belongs in that
        // method's arm.
        if let ("beyonce", "") = (service, method) {
            crate::destiny::on_ballpark_bind(w, self.session.character_id);
        }
    }

    fn session_change_for(
        &self,
        w: &mut Registry,
        service: &str,
        method: &str,
    ) -> Option<Vec<(PyValue, PyValue)>> {
        match (service, method) {
            // `None` with no actor is the right answer and not a failure:
            // the call is still answered, it just carries no session change.
            // Undocking before selecting a character is not a thing the
            // client can do anyway.
            ("ship", "Undock") => {
                // A trade in progress ends here (GAP-TRADE).
                crate::trade::on_leave(w, self.session.character_id);
                w.undock_change(self.session.character_id)
            }
            // **The way back in.** `CmdDock` arrives as an ordinary bound
            // call on `beyonce` rather than a fused bind, and nothing clever
            // is needed for that: `handle_bound_call` already routes through
            // `with_session_change`, which puts the change in front of the
            // answer. The answer is discarded by the client anyway --
            // `movementFunctions.RealDock` hands the method to
            // `sessionMgr.PerformSessionChange('dock', ...)` and `POP_TOP`s
            // the result **[V]** -- so the session change *is* the dock.
            ("beyonce", "CmdDock" | "CmdTurboDock") => w.dock_change(self.session.character_id),
            // **An undock from an Upwell structure** (U4): `structureDocking`
            // is a service, not the bound `ship`. Its dock is queued by the
            // call itself (`structure_dock::seat_in`); only the way out is
            // keyed here, and (like the station's) only on success.
            ("structureDocking", "Undock") => w.undock_change(self.session.character_id),
            _ => None,
        }
    }

    /// Put any session change this call caused in front of its answer.
    ///
    /// Two sources, and they differ in how they are found. A change the call
    /// is *defined by* -- undock -- is keyed on `(service, method)` by
    /// [`Self::session_change_for`]. A change a **mutator** asked for is
    /// drained from the subsystem that asked, because it is keyed on nothing:
    /// `fleetMgr.AcceptInvite` changes the caller's session, while
    /// `fleetMgr.KickMember` changes a session belonging to a character who
    /// called nothing at all. The first goes into this reply; the second is
    /// posted to that character's own connections.
    ///
    /// Safe in that order for the same reason it is safe for
    /// `SelectCharacterID`: the two are handled by different tasklets on the
    /// client. A `CallRsp` goes straight to the waiting tasklet's channel and
    /// never takes the session lock, while the notification takes the write
    /// lock and runs the handler chain — whose own blocking calls are
    /// answered from `conn::serve`'s next loop iteration.
    fn with_session_change(
        &mut self,
        w: &mut Registry,
        service: &str,
        method: &str,
        rsp: Vec<u8>,
    ) -> Result<Vec<Vec<u8>>, DispatchError> {
        let mut payloads = Vec::new();

        // The method-keyed change: undock, and whatever else joins it.
        if let Some(change) = self.session_change_for(w, service, method) {
            crate::outln!("[svc] session change: {service}.{method}");
            self.tail.sc += 1;
            payloads.push(self.session_change_payload(
                self.session.client_id,
                self.session.session_id,
                change,
            )?);
        }

        // The changes a *mutator* asked for, which are keyed on nothing:
        // joining a fleet is `fleetMgr.AcceptInvite` for the joiner and
        // `fleetObjectHandler.Invite` for the inviter, and kicking somebody
        // changes a session belonging to a character who did not call
        // anything. So these are drained rather than looked up, exactly like
        // `Registry::take_pending`.
        for state in w.fleets_mut().take_session_changes() {
            let change = crate::fleet::Fleets::session_change(state);
            if Some(state.character_id) == self.session.character_id {
                self.tail.sc += 1;
                payloads.push(self.session_change_payload(
                    self.session.client_id,
                    self.session.session_id,
                    change,
                )?);
            } else {
                self.post_session_change(w, state.character_id, change)?;
            }
        }

        // And the same for every other mutator: boarding a ship is the first.
        for (character_id, change) in w.take_session_changes() {
            if Some(character_id) == self.session.character_id {
                self.tail.sc += 1;
                payloads.push(self.session_change_payload(
                    self.session.client_id,
                    self.session.session_id,
                    change,
                )?);
            } else {
                self.post_session_change(w, character_id, change)?;
            }
        }

        payloads.push(rsp);
        Ok(payloads)
    }

    /// One `session_change` notification, addressed to one connection.
    ///
    /// `client_id` and `session_id` travel together on purpose: they are the
    /// *target's*, and for a session change bound elsewhere neither of them
    /// is ours. CCP's own sender pairs them the same way --
    /// `SessionChangeGPCS.SessionChanged` builds
    /// `SessionChangeNotification(destination=MachoAddress(clientID=clientID),
    /// sid=sid, change=(0, change))` from one session's two ids and then
    /// routes it by `GetMachoTransportByClientID(clientID)` **[V]**. The sid
    /// is how the recipient finds the session a change belongs to at all:
    /// `sessions.py` keys them in `sessionsBySID` **[V]**.
    ///
    /// So reusing the caller's sid for somebody else's connection would be
    /// the quiet kind of wrong -- right socket, wrong session named. What
    /// exactly the client does with a sid it does not know is `[I]`: not
    /// worth finding out, since the right value is in hand either way.
    fn session_change_payload(
        &self,
        client_id: i64,
        session_id: i64,
        change: Vec<(PyValue, PyValue)>,
    ) -> Result<Vec<u8>, DispatchError> {
        MachoPacket::session_change(
            MachoAddress::node(self.handshake.node_id()),
            MachoAddress::client(client_id),
            session_id,
            change,
        )
        .to_payload()
        .map_err(DispatchError::Proto)
    }

    /// Post a session change to **another character's** connections.
    ///
    /// The sibling of [`Self::deliver_elsewhere`] and the same three rules:
    /// every live session of that character gets it (a character may be
    /// logged in twice), `post` rather than `send` because the world write
    /// lock is held, and an offline character is not an error.
    ///
    /// Unlike a notification, this one needs the target's **session id** as
    /// well as its client id, so it reads whole `SessionEntry`s rather than
    /// mapping them down to ids.
    fn post_session_change(
        &mut self,
        w: &Registry,
        character_id: i64,
        change: Vec<(PyValue, PyValue)>,
    ) -> Result<(), DispatchError> {
        let Some(writers) = self.writers.clone() else {
            self.note_unanswered(format!(
                "session change for character {character_id}: no writer registry"
            ));
            return Ok(());
        };
        let targets: Vec<(i64, i64)> = w
            .sessions()
            .of_character(character_id)
            .iter()
            .map(|e| (e.client_id, e.session_id))
            .collect();
        if targets.is_empty() {
            return Ok(());
        }
        let due = Instant::now();
        for (client_id, session_id) in targets {
            let payload = self.session_change_payload(client_id, session_id, change.clone())?;
            self.tail.sc += 1;
            self.tail.frames_posted += 1;
            self.tail.bytes += payload.len() as u64;
            // **`Writers::post`, not `wire_for(..).lock()`.** This function
            // runs with the world write lock held, and the wire mutex is the
            // one the ticker holds while it is inside `write_all` -- so the
            // old form made one peer with a full TCP receive window freeze
            // the world for every player for up to `conn::WRITE_TIMEOUT`.
            // `docs/port-tracker.md` §26.3 found this site; §26.2 found the
            // other one and said "the only place", which is what stopped
            // anyone looking here for a month.
            writers.post(client_id, due, payload);
        }
        Ok(())
    }

    /// Whether `object_id` is one this server issued and no longer holds:
    /// `N=<our node>:<serial>` with a serial this process has minted
    /// (the serial is process-global, so it is compared with the counter,
    /// not with this connection's `last_object_id`, which another
    /// connection's ids can exceed), or a gone world-minted `w<seq>`.
    /// Ids of another node, or past the counter, are not ours.
    fn was_ours(&self, w: &Registry, object_id: &str) -> bool {
        if w.world_object_gone(object_id) {
            return true;
        }
        let Some((node, serial)) = object_id
            .strip_prefix("N=")
            .and_then(|r| r.split_once(':'))
            .and_then(|(n, s)| Some((n.parse::<i64>().ok()?, s.parse::<i64>().ok()?)))
        else {
            return false;
        };
        node == self.handshake.node_id()
            && serial >= 1
            && serial
                <= self
                    .last_object_id
                    .max(NEXT_OBJECT_ID.load(Ordering::Relaxed) - 1)
    }

    fn mint_object(&mut self, bound: &str, bind_params: PyValue) -> PyValue {
        let serial = (self.serial)();
        self.last_object_id = serial;
        let object_id = format!("N={}:{}", self.handshake.node_id(), serial);
        // Capped, never evicted (V2 F7): an evicted id is a reference the
        // client still holds and calls. Past the cap the reference is still
        // handed out, so the call that asked is answered, and a later call on
        // it is answered as a call on an unknown object.
        if self.bound_objects.len() < BOUND_CAP {
            self.bound_objects
                .insert(object_id.clone(), (bound.to_owned(), bind_params));
            crate::outln!("[svc] handed out object {object_id} for {bound}");
        } else {
            crate::outln!(
                "[svc] bound-object cap {BOUND_CAP} reached: {object_id} for {bound} NOT registered"
            );
            self.note_unanswered(format!("bound-object cap reached: {bound}"));
        }
        eve_proto::object_reference(&object_id, crate::pyutil::filetime_now())
    }

    /// Answer a plain call whose return value is an **object**.
    ///
    /// The difference from `handle_bind` is only in the shape of the reply:
    /// a bind answers `(boundObject, retval)`, while this answers with the
    /// reference alone, because that is what the caller assigns. Everything
    /// else -- minting the id, registering it so later method calls can be
    /// routed -- is the same, which is why this is a short function and not a
    /// second implementation of the bound-object protocol.
    ///
    /// `bound` is the synthetic service name the object's methods live under;
    /// see `Registry::OBJECT_RETURNING` for why these exist at all.
    fn handle_object_returning_call(
        &mut self,
        w: &mut Registry,
        packet: &MachoPacket,
        req: &Request<'_>,
        bound: &str,
    ) -> Result<Reply, DispatchError> {
        // **Dispatched first, and the answer thrown away.**
        //
        // For a long time this function only minted, because every call in
        // `OBJECT_RETURNING` was a pure handle-getter with no registry arm at
        // all -- `skillMgr2.GetMySkillHandler` and `invbroker`'s two
        // inventory calls create nothing server-side, so there was nothing to
        // run and the question never came up.
        //
        // `fleetObjectHandler.CreateFleet` is the first that **has a side
        // effect**: it has to create a fleet on the way through, and the
        // client's next statement is a method call on the object it got back
        // (`fleetSvc.py:395-399` **[V]**), so the answer must still be a
        // reference and not the new fleet's id.
        //
        // Hence dispatch, discard, then mint. `NoSuchMethod` is swallowed
        // because it is the *expected* answer for the three handle-getters;
        // every other error is reported, because a mutator that failed is
        // worth a log line even when its return value is unused.
        match w.dispatch_as(self.caller(), req) {
            Ok(_) => {}
            Err(ServiceError::NoSuchMethod { .. }) => {}
            Err(e) => {
                let what = Registry::describe(req);
                self.unhandled(what, &format!(" (object-returning): {e}"));
            }
        }

        let reference = self.mint_object(bound, PyValue::None);
        Ok(Reply {
            payloads: vec![Self::call_rsp(packet, &reference)?],
            pushes: Vec::new(),
            close: false,
        })
    }

    /// Answer a call on an object we previously handed out.
    ///
    /// Once the client holds a reference it stops addressing calls by service
    /// name: `MachoObjectConnection` fixes the destination to the **node** and
    /// identifies the object only by its `objectID` string inside the payload
    /// (`docs/reference/bound-objects.md` §1.6). So the service has to be recovered from
    /// the id we minted, which is what `bound_objects` is for.
    ///
    /// The call is then dispatched exactly like a service call, because we
    /// keep one object per service -- see the note on `handle_bind`. The
    /// method name always starts with an uppercase letter: lowercase
    /// attribute reads are served on the client from the
    /// `__publicattributes__` shipped with the reference and never reach us.
    fn handle_bound_call(
        &mut self,
        w: &mut Registry,
        packet: &MachoPacket,
        object_id: &str,
        method: &str,
        args: &[PyValue],
        kwargs: &[(PyValue, PyValue)],
    ) -> Result<Reply, DispatchError> {
        // This connection's own objects first, then the world's: a reference
        // a push handed to this pilot (`OnTradeInitiate`) was minted by the
        // world (`Registry::mint_world_object`) and is on no connection's list.
        let Some((service, bind_params)) = self
            .bound_objects
            .get(object_id)
            .cloned()
            .or_else(|| w.world_object(object_id))
        else {
            // An id we never issued. Worth saying loudly: it means either the
            // client kept a reference across something that should have
            // invalidated it, or we restarted under a live client.
            // UNMACHO: an id of ours that has been released (or dropped by a
            // session change) is answered `UnMachoDestination`, which makes
            // the client's `MonikeredCall` unbind and bind again
            // (`moniker.py:162-189`). [A] a reference the client got as a
            // plain return value has no retry, so the error reaches its
            // window; falsified by an "UnMachoDestination" box in a client
            // run, and then only `handle_bind`'s objects should qualify.
            if self.was_ours(w, object_id) {
                crate::outln!(
                    "[svc] call on released object {object_id}: {method} -> UnMachoDestination"
                );
                return Self::error_reply(
                    packet,
                    &eve_proto::user_error("UnMachoDestination", Vec::new()),
                );
            }
            // An id we never issued, so there is no service to route to and
            // not even a shape to guess at. Answer `None` rather than hang.
            let what = format!("call on unknown object {object_id}: {method}");
            let result = self.unhandled(what, "");
            let rsp = MachoPacket::respond_to(
                packet,
                MachoNetMsg::CallRsp,
                vec![eve_proto::response_payload(&result)],
            );
            return Ok(Reply {
                payloads: vec![rsp.to_payload().map_err(DispatchError::Proto)?],
                pushes: Vec::new(),
                close: false,
            });
        };

        // A method on an object can itself return an object: the inventory
        // calls arrive this way once `invbroker` is already bound.
        let result = if let Some(bound) = Registry::object_returning(&service, method) {
            // The new object's identity is this call's arguments, not the
            // parent object's: `GetInventory(10004, None)` and
            // `GetInventoryFromId(capsuleID, 0)` come off the same bound
            // `invbroker` and must not be minted alike.
            self.mint_object(bound, PyValue::Tuple(args.to_vec()))
        } else {
            let req = Request {
                service: &service,
                method,
                args,
                kwargs,
                bind_params: Some(&bind_params),
                actor: self.session.character_id,
            };
            // A raise is the same as a service call's: an `ErrorResponse`, so
            // the client's `except UserError` round a bound call --
            // `invControllers._AddItem:323` catching
            // `NotEnoughCargoSpace`, `clientDogmaLocation.py:857`
            // swallowing `EffectAlreadyActive2` -- has something to
            // catch. `None` read as success to every one of them.
            let why = format!(" on {object_id} {bind_params:?}: ");
            match self.answer_call(packet, &req, w.dispatch_as(self.caller(), &req), &why)? {
                Answered::Value(v) => v,
                Answered::Reply(reply) => return Ok(reply),
            }
        };

        // Undock reaches us as a fused bind today, but the client re-binds on
        // its own schedule -- if it still held the station `ship` object it
        // would arrive here instead. Hook both routes rather than depend on
        // which one a given run takes.
        let payloads =
            self.with_session_change(w, &service, method, Self::call_rsp(packet, &result)?)?;
        Ok(Reply {
            payloads,
            pushes: Vec::new(),
            close: false,
        })
    }

    /// Answer `charUnboundMgr.SelectCharacterID`: change the session.
    ///
    /// **The answer is not the point.** `PerformSessionChange` discards the
    /// return value; what makes the client enter the game is the
    /// `SessionChangeNotification` that comes with it. Answering `None` and
    /// sending no notification leaves the client sitting on the select screen
    /// having successfully selected nothing.
    ///
    /// Both go out in one reply, notification first. That is close to what
    /// the real server does -- it sends the change from *inside*
    /// `SelectCharacterID`, before responding -- and the ordering is safe
    /// because the two are handled by different tasklets on the client: a
    /// `CallRsp` is routed straight to the waiting tasklet's channel and
    /// never takes the session lock, while the notification takes the session
    /// write lock and runs the handler chain.
    ///
    /// The consequence to keep in mind is that **the client will call us
    /// again while this is in flight**, from those handlers, and some of
    /// those calls block. `conn::serve` copes without any change: it writes
    /// the replies and loops straight back to reading, so the nested
    /// `CallReq`s are answered from the same loop. What we must not do is
    /// send a notification that the handlers are themselves waiting on --
    /// every other even-numbered command takes a session *read* lock and
    /// would sleep until the change completes.
    fn handle_select_character(
        &mut self,
        w: &mut Registry,
        packet: &MachoPacket,
        args: &[PyValue],
    ) -> Result<Reply, DispatchError> {
        // Both failures below mean we cannot build a session change. Answer
        // the call anyway. `PerformSessionChange` discards the return value,
        // so the client simply stays on the character screen -- a visible,
        // reportable failure -- instead of freezing on the click with nothing
        // in either log.
        let answer_nothing = |me: &mut Self, what: String| -> Result<Reply, DispatchError> {
            let result = me.unhandled(what, "");
            let rsp = MachoPacket::respond_to(
                packet,
                MachoNetMsg::CallRsp,
                vec![eve_proto::response_payload(&result)],
            );
            Ok(Reply {
                payloads: vec![rsp.to_payload().map_err(DispatchError::Proto)?],
                pushes: Vec::new(),
                close: false,
            })
        };
        let character_id = match args.first() {
            Some(PyValue::Int(id)) => *id,
            other => {
                let what = format!("charUnboundMgr.SelectCharacterID: charID {other:?}");
                return answer_nothing(self, what);
            }
        };
        // A CEO from before CORP-W lacks the wallet roles: give them before
        // the login change reads the session role keys (CORP-T).
        if w.character(character_id)
            .is_some_and(|c| w.may_play(self.caller(), c))
        {
            w.heal_ceo_roles_at_login(character_id);
        }
        // Only a character this session may play (account-spec §4.6); any
        // other takes the same no-character answer.
        let Some(change) = w.character_change_as(self.caller(), character_id) else {
            let what = format!("charUnboundMgr.SelectCharacterID: no character {character_id}");
            return answer_nothing(self, what);
        };

        let notification = MachoPacket::session_change(
            MachoAddress::node(self.handshake.node_id()),
            MachoAddress::client(self.session.client_id),
            self.session.session_id,
            change,
        );
        let rsp = MachoPacket::respond_to(
            packet,
            MachoNetMsg::CallRsp,
            vec![eve_proto::response_payload(&PyValue::None)],
        );
        // **The one writer of character identity.** Everything downstream
        // reads `Request::actor`, which is this field; before this line the
        // server built the change, sent it, logged it and forgot which
        // character was playing, so every service arm went back to reading
        // "the first character". Written only on success, and only after
        // `character_change` has confirmed the id names a real character.
        self.session.character_id = Some(character_id);
        // The other half of the same write. `w` is the world lock this
        // payload already holds, so this is not a second acquisition.
        // NET-TAKEOVER: one session per character. The old connection's
        // logoff runs after this one is in the table, so
        // `session_closed`'s last-session guard sees a character still
        // online and skips the logoff hooks (presence, fleet, trade, chat).
        // **A bot's character is exempt.** Bots reconnect on any error, so a
        // human and a bot on one character would evict each other in a loop;
        // the bot character keeps taking several sessions, as before.
        let me = self.session.client_id;
        if !w.character(character_id).is_some_and(|c| c.is_bot) {
            self.evictions.extend(
                w.sessions()
                    .of_character(character_id)
                    .iter()
                    .filter(|e| e.client_id != me)
                    .map(|e| (e.client_id, CHARACTER_ELSEWHERE)),
            );
        }
        w.session_selected_character(self.session.client_id, character_id);
        crate::outln!("[svc] session change: character {character_id} entering the game");
        // Plan §1.5: the no-bundle warning again on every login.
        if !w.content().is_bundle() {
            crate::outln!("{}", crate::content::NO_BUNDLE);
        }
        Ok(Reply {
            payloads: vec![
                notification.to_payload().map_err(DispatchError::Proto)?,
                rsp.to_payload().map_err(DispatchError::Proto)?,
            ],
            pushes: Vec::new(),
            close: false,
        })
    }

    /// Answer `userSvc.UserLogOffCharacter` (Escape > Log Off Character).
    ///
    /// **The return value is the permission.** `gameui.LogOffCharacter` is
    /// `if PerformSessionChange('logoff', userSvc.UserLogOffCharacter):
    /// DoCharacterLogoff() else: 'Character logoff denied'`, and
    /// `PerformSessionChange` returns the call's value **[V]**
    /// (`gameui.py:473-481`, `sessions.py:471`). We answered `None`, so the
    /// client refused its own log-off and sat in the session change. The
    /// answer is `True` and nothing else: a session change ahead of it raises
    /// in crimewatch and dogma, and the client's own `DoCharacterLogoff`
    /// clears its session. `Registry::character_log_off` has the server half,
    /// and this connection is no longer anybody's.
    fn handle_log_off(
        &mut self,
        w: &mut Registry,
        packet: &MachoPacket,
    ) -> Result<Reply, DispatchError> {
        if w.character_log_off(self.session.client_id) {
            crate::outln!(
                "[svc] character {:?} logging off to the select screen",
                self.session.character_id
            );
        }
        self.session.character_id = None;
        let rsp = MachoPacket::respond_to(
            packet,
            MachoNetMsg::CallRsp,
            vec![eve_proto::response_payload(&PyValue::Bool(true))],
        );
        Ok(Reply {
            payloads: vec![rsp.to_payload().map_err(DispatchError::Proto)?],
            pushes: Vec::new(),
            close: false,
        })
    }

    /// Answer a `MachoBindObject`: hand back a reference to the object, and
    /// the result of the first call on it.
    ///
    /// **Bind and first call are one round trip.** `Moniker.Bind` sends
    /// `MachoBindObject(bindParams, call)` where `call` is either `None` or
    /// the `(methodName, args, keywords)` of the call that triggered the
    /// bind, and unpacks a 2-tuple `(boundObject, retval)` -- `retval` being
    /// `None` when `call` was. So this is the one place where answering with
    /// the right shape is not enough: the *call inside the call* has to be
    /// dispatched too, or the client gets a bound object and no answer.
    ///
    /// The fused call is dispatched against the same service name, so a
    /// bound method is an ordinary arm in `service.rs` and gets the same
    /// `UNANSWERED` diagnostics as everything else. That is a simplification
    /// we are entitled to only because we are a single node with one object
    /// per service: a real cluster binds a distinct object per `bindParams`.
    /// When that stops being true, this is the function that has to grow a
    /// registry keyed by `(service, bindParams)`.
    ///
    /// We do not check the resolved node. `ServiceCallGPCS.CallUp` would
    /// raise `WrongMachoNode` if `MachoResolveObject` disagreed with the node
    /// handling the call, but we answer as exactly one node, so the question
    /// cannot arise.
    fn handle_bind(
        &mut self,
        w: &mut Registry,
        packet: &MachoPacket,
        service: &str,
        args: &[PyValue],
    ) -> Result<Reply, DispatchError> {
        // args = (bindParams, call)
        let call = args.get(1).unwrap_or(&PyValue::None);
        // The fused call's method name, hoisted out of the match so the reply
        // can be checked against `session_change_for`. `ship.Undock` arrives
        // this way -- as a bind with a call fused onto it -- so without this
        // the undock session change would never be sent.
        let mut fused_method = String::new();
        let retval = match call {
            PyValue::None => PyValue::None,
            PyValue::Tuple(parts) if !parts.is_empty() => {
                let method = match &parts[0] {
                    PyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
                    PyValue::Unicode(s) => Some(s.clone()),
                    other => {
                        let what = format!("{service}.MachoBindObject: method name {other:?}");
                        self.unhandled(what, "");
                        None
                    }
                };
                // No usable method name means there is no fused call to make,
                // but the bind itself is still answerable: fall through and
                // hand out the object with a `None` retval, rather than
                // dispatching a method called "" and logging a second,
                // meaningless miss for it.
                if let Some(name) = &method {
                    fused_method = name.clone();
                }
                let Some(method) = method else {
                    let reference =
                        self.mint_object(service, args.first().cloned().unwrap_or(PyValue::None));
                    let rsp = MachoPacket::respond_to(
                        packet,
                        MachoNetMsg::CallRsp,
                        vec![eve_proto::response_payload(&PyValue::Tuple(vec![
                            reference,
                            PyValue::None,
                        ]))],
                    );
                    return Ok(Reply {
                        payloads: vec![rsp.to_payload().map_err(DispatchError::Proto)?],
                        pushes: Vec::new(),
                        close: false,
                    });
                };
                let inner_args = match parts.get(1) {
                    Some(PyValue::Tuple(a)) | Some(PyValue::List(a)) => a.clone(),
                    _ => Vec::new(),
                };
                let kwargs = match parts.get(2) {
                    Some(PyValue::Dict(d)) => d.clone(),
                    _ => Vec::new(),
                };
                let req = Request {
                    service,
                    method: &method,
                    args: &inner_args,
                    kwargs: &kwargs,
                    // **The bind's params, not `None`.** A fused call is a
                    // call on the object being bound, so it has to see what
                    // the object is: `Moniker('fleetObjectHandler', fleetID)
                    // .AcceptInvite(shipTypeID)` is one bind with one fused
                    // call (`fleetSvc.OnFleetInvite:1347` **[V]**), and with
                    // no bind params the arm cannot tell which fleet is being
                    // joined -- it falls through to "the fleet this character
                    // is in", which for a pilot accepting an invite is none.
                    //
                    // This was `None` while no fused call needed them:
                    // `ship.Undock` and `dogmaIM.GetAllInfo` do not, and
                    // `invbroker`'s are object-returning and never reach a
                    // dispatch at all. The first *subsequent* call on any
                    // bound object has always had them
                    // (`handle_bound_call` reads them back out of
                    // `bound_objects`), so the symptom was that an object
                    // worked for every call except the one fused onto its
                    // bind.
                    bind_params: args.first(),
                    actor: self.session.character_id,
                };
                // The fused call can itself return an object -- the
                // inventory calls do -- in which case there is nothing for
                // the registry to compute.
                if let Some(bound) = Registry::object_returning(service, &method) {
                    // **The new object's identity is the fused call's
                    // arguments, not the bind's.** `invbroker` is bound once
                    // on `(stationID, groupStation)` and then hands out
                    // several different inventories from it. Minting them all
                    // with the outer bind params made them indistinguishable,
                    // so `GetSelfInvItem` answered "the station" even for the
                    // ship's inventory.
                    self.mint_object(bound, PyValue::Tuple(inner_args.clone()))
                } else {
                    match w.dispatch_as(self.caller(), &req) {
                        Ok(v) => v,
                        // **A raise is the bind's answer**, exactly as for a
                        // service call: `Moniker.Bind` re-raises it from
                        // `MachoBindObject`, so the client's `try/except
                        // UserError` round the fused method sees it. Answering
                        // `None` swallowed every refusal on this route
                        // (`docs/lanes/bots.md`, P8a note).
                        Err(ServiceError::Raise(exception)) => {
                            return self.raised(packet, &req, &exception);
                        }
                        Err(e) => {
                            let what = Registry::describe(&req);
                            self.unhandled(what, &format!(" (bound object): {e}"))
                        }
                    }
                }
            }
            other => {
                // Neither `None` nor a `(method, args, kwargs)` tuple. Nothing
                // to dispatch, but the bind still gets its object.
                let what = format!("{service}.MachoBindObject: call {other:?}");
                self.unhandled(what, "")
            }
        };

        let reference = self.mint_object(service, args.first().cloned().unwrap_or(PyValue::None));

        let rsp = MachoPacket::respond_to(
            packet,
            MachoNetMsg::CallRsp,
            vec![eve_proto::response_payload(&PyValue::Tuple(vec![
                reference, retval,
            ]))],
        );
        // **The method-keyed drain point.** A bind's arm sees
        // `MachoBindObject`, not the method the client actually wanted, so
        // anything that must follow the *fused* call cannot be queued from an
        // arm -- the arm for that method never runs. Called here, beside
        // `with_session_change`, which is the same shape and the precedent.
        self.pending_for_bound_call(w, service, &fused_method);

        // This is the route `ship.Undock` actually takes.
        let payloads = self.with_session_change(
            w,
            service,
            &fused_method,
            rsp.to_payload().map_err(DispatchError::Proto)?,
        )?;
        Ok(Reply {
            payloads,
            pushes: Vec::new(),
            close: false,
        })
    }

    fn handle_ping(&mut self, packet: &MachoPacket) -> Result<Reply, DispatchError> {
        /// One client ping, as it would be annotated crossing a proxy to a
        /// server node and back. Order is load-bearing; see above.
        const HOPS: [&str; 7] = [
            "proxy::handle_message",  // the proxy takes the request
            "proxy::writing",         // ...and forwards it to the server
            "server::handle_message", // the server takes it
            "server::turnaround",     // ...answers it
            "server::writing",        // ...and writes the answer back
            "proxy::handle_message",  // the proxy takes the answer
            "proxy::writing",         // ...and writes it to the client
        ];

        // `params` for PingReq is `['times']`, so the list is body[0]. A ping
        // with no times is still a ping; start a list rather than refuse.
        let mut times = match packet.body.first() {
            Some(PyValue::List(items)) => items.clone(),
            Some(PyValue::Tuple(items)) => items.clone(),
            _ => Vec::new(),
        };
        let now = crate::pyutil::filetime_now();
        for label in HOPS {
            times.push(PyValue::Tuple(vec![
                PyValue::Int(now),
                PyValue::Int(now),
                // Unicode, not bytes: the client's own entry arrives under
                // tag 0x2E (UTF-8), from the same `macho.mode + '::...'`
                // expression, so matching it keeps the list uniform.
                PyValue::Unicode(label.to_owned()),
            ]));
        }

        let rsp = MachoPacket::respond_to(packet, MachoNetMsg::PingRsp, vec![PyValue::List(times)]);
        Ok(Reply {
            payloads: vec![rsp.to_payload().map_err(DispatchError::Proto)?],
            pushes: Vec::new(),
            close: false,
        })
    }

    pub fn handshake(&self) -> &Handshake {
        &self.handshake
    }

    /// Payloads received after the handshake completed.
    pub fn post_handshake_payloads(&self) -> usize {
        self.post_handshake_payloads
    }

    /// Drain the world's pending notifications into frames.
    ///
    /// Each one is a thing that happened which the call that caused it does not
    /// return: five subsystems' worth of mutators discard their return value
    /// entirely, so this is how the client finds out at all.
    fn pending_pushes(
        &mut self,
        w: &mut Registry,
    ) -> Result<Vec<(Duration, Vec<u8>)>, DispatchError> {
        let mut out = Vec::new();
        let took = Instant::now();
        let pending = w.take_pending();
        self.tail.take = took.elapsed();
        self.tail.pushes = pending.len() as u32;
        let fan = Instant::now();
        for (after, n) in pending {
            // **No match, and that is the point.** Each push knows its own
            // event name, its arguments and who it is for, because the module
            // that has the client evidence for a shape is the module that
            // should hold it -- `crate::notify` has the history, and
            // `docs/lanes/README.md` rule 2 has the reason. This loop used to
            // be 170 lines and every new push in the project edited it.
            let audience = n.audience();
            let (event, args) = n.to_push();
            let Some(audience) = audience else {
                // The acting session: straight into this reply, where it
                // goes out after the response in the same flush.
                let frame = self.notify(event, args)?;
                self.tail.frames_inline += 1;
                self.tail.bytes += frame.len() as u64;
                out.push((after, frame));
                continue;
            };
            // **Pickled once for the whole audience** (lane perf-fight):
            // the body names no recipient, so each frame below is only its
            // address -- the bytes `notify` would give, without marshalling
            // the arguments again for every pilot on the grid, under the
            // world lock.
            // With `--coalesce-destiny`, a mergeable Destiny push is also
            // classified here, so `deliver_elsewhere` queues a part per target
            // instead of encoding a frame per target (PA-10).
            let part = crate::push::coalesce_destiny()
                .then(|| crate::push::destiny_part(event, &args))
                .flatten();
            let body = MachoPacket::notification_body(args);
            let part = part.map(|p| crate::push::PartSrc::with_body(p, &body));
            for character_id in audience {
                if Some(character_id) == self.session.character_id {
                    // This connection, inline, so the frame rides out with
                    // the answer to the call that caused it.
                    let frame = self.notify_body_to(self.session.client_id, event, &body)?;
                    self.tail.frames_inline += 1;
                    self.tail.bytes += frame.len() as u64;
                    out.push((after, frame));
                }
                // **And every other connection of that character, including
                // another window of our own.** The `if` above is not an
                // `else`, and that is the fix: `notify` addresses *this*
                // connection by construction, so a character logged in twice
                // used to get an addressed push on the window that acted and
                // nothing on the other -- while the identical push reached
                // both windows of anybody else. In space that is a window
                // with no ship in it.
                self.deliver_elsewhere(w, character_id, event, &body, part.as_ref(), after)?;
            }
        }
        self.tail.fanout = fan.elapsed();
        Ok(out)
    }

    /// Post a notification to **another character's** connections.
    ///
    /// Two lookups, each owned by the thing that knows about it:
    /// `session_table::of_character` turns a character into client ids -- a
    /// `Vec`, because a character may be logged in twice -- and
    /// `push::Writers::wire_for` turns a client id into somewhere to write.
    /// Every live session of that character gets the frame, which is what the
    /// real server's scattercast does and the only answer that is not a
    /// guess about which of two clients the player is looking at.
    ///
    /// **`post`, never `send`.** This runs with the world write lock held
    /// (`on_payload` takes it once for the whole payload), and
    /// `docs/reference/world-state.md` §4 rule 4 forbids a socket write under a lock:
    /// one blocked peer would stall the world for everybody. `Wire::post` is
    /// an in-memory enqueue, and the ticker -- which deliberately never takes
    /// the world lock -- does the writing within one `push::TICK`. That also
    /// fixes the lock order at world-then-wire everywhere, so there is no
    /// cycle to deadlock on.
    ///
    /// A frame addressed to another connection must carry **that**
    /// connection's `client_id`, not ours; see [`Self::notify_to`].
    ///
    /// **Deliberately narrower than the real server, in one place.** A push
    /// whose `audience()` is `None` -- "tell the caller" -- still reaches only
    /// the connection that called. CCP's scattercast is addressed to a
    /// *character*, so both of a player's windows would learn; ours tells the
    /// one that acted. That is visible for a GM spawn made in one window
    /// while the other has the inventory open, and it is left alone here
    /// because widening it changes every unaddressed push in the project at
    /// once. The addressed case -- this function -- is now consistent either
    /// way.
    fn deliver_elsewhere(
        &mut self,
        w: &Registry,
        character_id: i64,
        event: &str,
        body: &PyValue,
        part: Option<&crate::push::PartSrc>,
        after: Duration,
    ) -> Result<(), DispatchError> {
        let Some(writers) = self.writers.clone() else {
            // No registry: a unit test or capture mode. Say so rather than
            // dropping it silently -- a missing notification with no trace is
            // the failure this whole section exists to avoid.
            self.note_unanswered(format!(
                "{event} for character {character_id}: no writer registry"
            ));
            return Ok(());
        };
        // **Elsewhere means elsewhere**: this connection is excluded, because
        // when the audience names our own character the caller has already
        // served us inline through `notify`. Without the filter the acting
        // window would get the frame twice, which for a `SetState` is a
        // second ballpark bootstrap.
        let targets: Vec<i64> = w
            .sessions()
            .of_character(character_id)
            .iter()
            .map(|e| e.client_id)
            .filter(|id| *id != self.session.client_id)
            .collect();
        if targets.is_empty() {
            // Offline. Not an error and not worth a log line: a fleet invite
            // to a pilot who logged off is simply not delivered, and the
            // fleet state is already correct without it.
            return Ok(());
        }
        let due = Instant::now() + after;
        for client_id in targets {
            if let Some(src) = part {
                // PA-10: the outbox may merge it into its tail; the writer
                // encodes it. The acting connection's inline frame is still
                // encoded bytes, above.
                let node = self.handshake.node_id();
                self.tail.frames_posted += 1;
                self.tail.bytes += src.est() as u64;
                writers.post_part(client_id, due, src.for_client(node, client_id));
                continue;
            }
            let payload = self.notify_body_to(client_id, event, body)?;
            self.tail.frames_posted += 1;
            self.tail.bytes += payload.len() as u64;
            // **`Writers::post`, not `wire_for(..).lock()`** -- the inversion
            // §26.2 designed this away: we hold the world lock here, and the
            // wire mutex is the one a blocked `write_all` sits on. A `false`
            // return is the connection having gone, which is the same normal
            // answer `wire_for` gave by returning `None`, and a poisoned
            // outbox is skipped the way the ticker skips a poisoned wire.
            writers.post(client_id, due, payload);
        }
        Ok(())
    }

    /// A client-bound `Notification`, addressed the way the real server
    /// addresses a per-client one: `Scattercast('clientID', [clientID], event,
    /// *args)`. The `*` on the idtype is added by
    /// `ScattercastWithoutTheStars` itself, which is why it is literal here.
    ///
    /// The event name must begin with `On`. `ScatterEventWithoutTheStars` logs
    /// two errors and a traceback for anything else, and the two other prefixes
    /// the client special-cases are not what we want: `Do*` runs its handlers
    /// **synchronously inside `NotifyUp`** with `block_trap` set, so a handler
    /// that calls back to us raises, and `Process*` chains them.
    ///
    /// **`DoDestinyUpdate` is the one deliberate exception**, and it is an
    /// exception on evidence rather than by necessity: its only registered
    /// handler is `michelle.DoDestinyUpdate`, which logs, expands
    /// `PackagedAction` entries, counts timestamps and appends to
    /// `Park.history` -- nothing that can block. The native parse, the prime
    /// wave and the scene work all happen later, on the client's own tick,
    /// out of `DoPreTick`. The rule stands for everything else; see
    /// `crate::destiny`'s header for the full argument.
    fn notify(&self, event: &str, args: Vec<PyValue>) -> Result<Vec<u8>, DispatchError> {
        // The connection, not the account. See `Session::client_id`.
        self.notify_to(self.session.client_id, event, args)
    }

    /// The same, addressed to a named connection.
    ///
    /// Split out from [`Self::notify`] because a fleet invite goes to a
    /// connection that is not this one, and the `*clientID` narrowcast has to
    /// name **that** connection. It was `self.session.client_id`
    /// unconditionally, which is why a push could only ever reach its own
    /// caller.
    fn notify_to(
        &self,
        client_id: i64,
        event: &str,
        args: Vec<PyValue>,
    ) -> Result<Vec<u8>, DispatchError> {
        MachoPacket::notification(
            MachoAddress::node(self.handshake.node_id()),
            event,
            "*clientID",
            vec![PyValue::Int(client_id)],
            args,
        )
        .to_payload()
        .map_err(DispatchError::Proto)
    }

    /// [`Self::notify_to`] with the arguments already pickled by
    /// `MachoPacket::notification_body`: the same bytes, for a push that
    /// goes to more than one connection.
    fn notify_body_to(
        &self,
        client_id: i64,
        event: &str,
        body: &PyValue,
    ) -> Result<Vec<u8>, DispatchError> {
        MachoPacket::notification_pickled(
            MachoAddress::node(self.handshake.node_id()),
            event,
            "*clientID",
            vec![PyValue::Int(client_id)],
            body.clone(),
        )
        .to_payload()
        .map_err(DispatchError::Proto)
    }

    /// Decode one payload. **An associated function with no `self` and no
    /// world**, so that it cannot take the world lock is a property of its
    /// signature (V2 F1): `on_payload` calls it before the one acquisition.
    ///
    /// Before login a payload that is not marshal is a handshake failure and
    /// ends the connection, as it always did. After login every outcome is
    /// an [`Inbound`] to record, and a `CallReq`'s or `Notification`'s inner
    /// pickle is decoded here too (`CallPayload::from_payload`).
    fn decode(authenticated: bool, payload: &[u8]) -> Result<Inbound, DispatchError> {
        if !authenticated {
            return loads(payload)
                .map(Inbound::Handshake)
                .map_err(DispatchError::Marshal);
        }
        let value = match loads(payload) {
            Ok(v) => v,
            Err(WireError::UnsupportedTag { at, tag }) => {
                return Ok(Inbound::UnknownTag { at, tag });
            }
            Err(e) => return Ok(Inbound::Undecodable(e)),
        };
        let packet = match MachoPacket::from_value(&value) {
            Ok(p) => p,
            Err(e) => return Ok(Inbound::Unparsed(e)),
        };
        let call = match packet.command {
            MachoNetMsg::CallReq | MachoNetMsg::Notification => {
                packet.body.first().map(CallPayload::from_payload)
            }
            _ => None,
        };
        Ok(Inbound::Packet(Box::new(Decoded { packet, call })))
    }

    fn observe(&mut self, w: &mut Registry, inbound: Inbound) -> Result<Reply, DispatchError> {
        self.post_handshake_payloads += 1;
        match inbound {
            Inbound::Packet(decoded) => self.handle_packet(w, *decoded),
            Inbound::UnknownTag { at, tag } => {
                if !self.unknown_tags.iter().any(|(_, t)| *t == tag) {
                    self.unknown_tags.push((at, tag));
                }
                Ok(Reply::nothing())
            }
            Inbound::Undecodable(e) => {
                self.note_unanswered(format!("undecodable payload: {e}"));
                Ok(Reply::nothing())
            }
            // Not a packet we model. Log rather than fail: this is the path
            // the first real client connection lit up, and we want the
            // evidence, not a disconnect. No header parsed, so there is no
            // call id to answer.
            Inbound::Unparsed(e) => {
                self.note_unanswered(format!("unparsed packet: {e}"));
                Ok(Reply::nothing())
            }
            // `decode` only builds this before login, and the phase cannot
            // go backwards.
            Inbound::Handshake(_) => {
                self.note_unanswered("a handshake value after login".to_owned());
                Ok(Reply::nothing())
            }
        }
    }
}

impl Dispatch for GpsSession {
    fn greeting(&mut self) -> Vec<Vec<u8>> {
        vec![dumps(&self.handshake.greeting())]
    }

    /// Handle one payload, then drain whatever the world now wants pushed.
    ///
    /// **The drain is here and not in the arms on purpose.** An arm sees a
    /// `&mut Registry` and nothing else: it can record that something happened
    /// which the client has to be *told*, and it cannot send anything. Turning
    /// that record into a frame needs the session and the node id, which live
    /// out here; and performing the write needs a socket, which lives further
    /// out still, in `conn::serve`. `docs/reference/world-state.md` §4 rule 4 -- never
    /// hold a lock across a socket write -- then holds by construction once
    /// `Registry` goes behind an `RwLock`, rather than by everyone remembering.
    fn on_payload(&mut self, payload: &[u8]) -> Result<Reply, DispatchError> {
        // **The one acquisition.** See the `world` field's comment for why it
        // is here and not in the arms.
        //
        // Poisoning is recovered rather than propagated, and the choice is
        // deliberate (`docs/reference/world-state.md` §4 rule 5): with one world behind
        // one lock, propagating a poison would let a single panic in a
        // cosmetic arm end the universe for every connected player, including
        // the ones who were not in that arm. Recovering risks serving state a
        // panicking thread left half-written. We take that risk because the
        // mutating arms validate before they mutate, so the critical sections
        // have no fallible work in them -- and because a dead world is
        // unrecoverable while a wrong row is a bug report.
        //
        // **Decoded first, with no lock** (V2 F1): inflate and decode can
        // take a while for a large payload, and under the lock that while
        // was every player's.
        self.post_lock = None;
        self.evictions.clear();
        let inbound = Self::decode(
            self.handshake.phase() == crate::handshake::Phase::Authenticated,
            payload,
        )?;
        // World accounts (ACC2): H6's login hashes a password, so it runs
        // here, before the turn's lock, and takes the lock only briefly.
        if let Inbound::Handshake(v) = &inbound {
            self.handshake.pre_login(v, &self.world);
        }
        let world = Arc::clone(&self.world);
        // CP15: `[turns]` names the slowest call; wait and hold are timed
        // apart (V3 F3).
        let label = inbound.label();
        let asked = Instant::now();
        // PA-12a: a read-eligible call is served under `world.read()` when
        // nothing is queued for the turn to drain; otherwise it falls through
        // to the one write acquisition below, from the same session state.
        let mut fallback = false;
        if self.read_turns {
            match self.try_read_turn(&inbound, label.as_deref(), asked)? {
                ReadTurn::Served(reply) => return Ok(reply),
                ReadTurn::Fallback => fallback = true,
                ReadTurn::NotRead => {}
            }
        }
        // A recovered poison **taints** the world (review M5, ruling 10):
        // some arm panicked mid-turn, the claim above is unproven (IP19),
        // and the store must not save what it left over the good file
        // (IP11). Sticky, and logged once.
        self.wait_for_clock();
        let waiting = crate::clock_probe::Waiter::enter();
        let mut w = match world.write() {
            Ok(w) => w,
            Err(poisoned) => {
                let mut w = poisoned.into_inner();
                if !w.tainted() {
                    crate::errln!(
                        "[world] TAINTED: a panic poisoned the world lock; serving on, \
                         but the world may be half-written and will not be saved over \
                         the good file"
                    );
                }
                w.taint();
                w
            }
        };
        drop(waiting);
        let acquired = Instant::now();
        // The turn trace (TT-A): reset what an earlier early return left, then
        // note what was queued before this turn did anything.
        crate::logline::take_thread_stats();
        let sched0 = crate::sched_stat::sample();
        let cancel0 = crate::clock_probe::cancel_ns_here();
        let left = w.pending_len();
        self.tail = TailCounters::default();
        // **Captured before the arm runs**, which is the difference between
        // "the turn after login" and "the login turn itself": the handshake
        // arm sets `session_sent` inside `handle_inbound`, so reading it
        // afterwards fires on the login flush and makes this variant
        // identical to putting the frame in `payloads`.
        let mut reply = self.handle_inbound(&mut w, inbound)?;
        let arm = acquired.elapsed();
        let queued = w.pending_len();
        // **`global_config` is built and NOT sent, and the client is why.**
        // §41 derived that `machoNet.InitClientGlobalConfig` was callable
        // because `ServiceCallGPCS.CallUp` gates on a `__restrictedcalls__`
        // deny list. The client disagreed on the first run that sent it:
        //
        //   exceptions.RuntimeError: In machoNet, method
        //   InitClientGlobalConfig is not exported
        //   ... ObjectCallGPCS.py(214) CallUp
        //
        // The call goes through `ObjectCallGPCS`, not `ServiceCallGPCS`, and
        // that layer *does* enforce `__exportedcalls__` -- which
        // `InitClientGlobalConfig` is not in. So the route is closed, not
        // merely unfinished. `docs/port-tracker.md` §41.10.
        reply.pushes.extend(self.pending_pushes(&mut w)?);
        // GW-OUT: notices for gateway clients leave with the pushes, but are
        // published only after the guard drops. Empty costs nothing.
        let notices = crate::gateway_out::take(&mut w);
        // **A turn does not save** (CP52, owner decision O1). It used to
        // write the whole world with an fsync, under this lock, after every
        // dirty payload: at 50 mining bots that was most turns, and the
        // biggest single cost on the lock. Now a mutation only marks the
        // world dirty, and the clock saves it every `push::SAVE_EVERY`,
        // rendering under the lock and writing on the saver thread
        // (`crate::store::Saver`). RAM may lead disk by up to `SAVE_EVERY`;
        // that is the decided revision of `docs/reference/world-state.md` §4 rule 3,
        // and shutdown writes the rest ([`flush_world`]).
        // Recorded under the lock this turn holds: no new mutex (CP15).
        let (turn_wait, turn_hold) = (
            acquired.saturating_duration_since(asked),
            acquired.elapsed(),
        );
        let log = crate::logline::take_thread_stats();
        let sched = sched0.and_then(|a| {
            crate::sched_stat::sample().map(|b| {
                let (cpu, runq) = b.since(a);
                crate::live_status::Sched { cpu, runq }
            })
        });
        let tail = crate::live_status::TurnTail {
            left: left as u32,
            queued: queued as u32,
            arm,
            take: self.tail.take,
            fanout: self.tail.fanout,
            pushes: self.tail.pushes,
            frames_inline: self.tail.frames_inline,
            frames_posted: self.tail.frames_posted,
            bytes: self.tail.bytes,
            sc: self.tail.sc,
            log: Duration::from_nanos(log.ns),
            log_lines: log.lines,
            sched,
            cancel: Duration::from_nanos(
                crate::clock_probe::cancel_ns_here().saturating_sub(cancel0),
            ),
        };
        crate::live_status::prof::payload_turn(
            label.as_deref(),
            turn_wait,
            turn_hold,
            if fallback {
                crate::live_status::prof::TurnKind::Fallback
            } else {
                crate::live_status::prof::TurnKind::Write
            },
        );
        crate::live_status::record_turn(
            &w,
            crate::live_status::Turn {
                wait: turn_wait,
                hold: turn_hold,
                save: None,
                label: label.as_deref(),
                tail: Some(&tail),
                read: false,
                fallback,
            },
        );
        drop(w);
        crate::gateway_out::flush(notices);
        self.run_evictions();
        self.finish_post_lock(&mut reply)?;
        Ok(reply)
    }

    fn state(&self) -> String {
        self.describe_state()
    }

    /// [`Self::with_preauth_timeout`]'s leash until the handshake is done,
    /// then `conn::serve`'s own default (V2 F5).
    fn idle_timeout(&self) -> Option<Duration> {
        (self.handshake.phase() != crate::handshake::Phase::Authenticated)
            .then_some(self.preauth_timeout)
    }

    /// [`crate::conn::PREAUTH_MAX_FRAME`] until the handshake is done.
    fn max_frame(&self) -> Option<usize> {
        (self.handshake.phase() != crate::handshake::Phase::Authenticated)
            .then_some(crate::conn::PREAUTH_MAX_FRAME)
    }

    /// A `PingReq` with an empty `times` list, addressed like our other
    /// notifications. The client appends its turnaround and answers
    /// (`machoNet.HandleMessage`), so the list must be a list (NET-PING).
    fn keepalive_ping(&mut self) -> Option<Vec<u8>> {
        if self.handshake.phase() != crate::handshake::Phase::Authenticated {
            return None;
        }
        MachoPacket::new(
            MachoNetMsg::PingReq,
            MachoAddress::node(self.handshake.node_id()),
            MachoAddress::client(self.session.client_id),
            vec![PyValue::List(Vec::new())],
        )
        .to_payload()
        .ok()
    }
}

impl GpsSession {
    fn handle_inbound(
        &mut self,
        w: &mut Registry,
        inbound: Inbound,
    ) -> Result<Reply, DispatchError> {
        let value = match inbound {
            Inbound::Handshake(value) => value,
            other => return self.observe(w, other),
        };
        let outcome = self
            .handshake
            .on_payload(&value)
            .map_err(DispatchError::Handshake)?;
        let mut payloads: Vec<Vec<u8>> = outcome.replies.iter().map(dumps).collect();

        // H10 completes the handshake; the session follows immediately, in
        // the same flush. The client cannot do anything useful until it has
        // one, and sending it unprompted is what the real proxy does.
        if self.handshake.phase() == crate::handshake::Phase::Authenticated && !self.session_sent {
            self.session_sent = true;
            // BA-4: the client's own build rides on the session, so a
            // service can tell which accepted build it is talking to.
            self.session.client_build = self.handshake.identity().boot_build;
            // IP27-a (`account-spec.md` §5): the account resolved in H6
            // becomes this session's `userid` and role, and the session
            // table learns the `userid`, **before** the session is sent:
            // `basesession.py:1600` forbids a later `userid` change. The
            // catch-all resolves to the template's own id and mask.
            if let Some(r) = self.handshake.resolved() {
                self.session.adopt(r);
                w.session_opened(crate::session_table::SessionEntry {
                    client_id: self.session.client_id,
                    session_id: self.session.session_id,
                    user_id: r.user_id,
                    character_id: None,
                });
                // NET-TAKEOVER: one client per account. Off in the catch-all,
                // whose one shared user id is every bot and player at once;
                // the character rule covers that mode.
                if self.handshake.accounts_are_distinct() {
                    let me = self.session.client_id;
                    self.evictions.extend(
                        w.sessions()
                            .of_user(r.user_id)
                            .iter()
                            .filter(|e| e.client_id != me)
                            .map(|e| (e.client_id, ACCOUNT_ELSEWHERE)),
                    );
                }
            }
            payloads.push(self.session_init_packet()?);
            // **`global_config_packet` is NOT sent here yet, and the reason is
            // measured rather than cautious.** Adding it to this flush failed
            // **38** tests: every `loopback.rs` fixture reads the post-login
            // frames in sequence, so one more frame shifts all of them. None
            // of those failures is a defect -- they are fixtures encoding the
            // sequence -- but updating 38 of them is a change that should be
            // made deliberately, with each one re-read, rather than folded
            // into the first send.
            //
            // It also surfaces a question this flush cannot answer: *where*
            // in the login sequence the real server sends it. Here is a
            // guess; the first payload turn after login is another. See
            // `docs/port-tracker.md` §41.7.
        }
        Ok(Reply {
            payloads,
            pushes: Vec::new(),
            close: outcome.close,
        })
    }

    fn describe_state(&self) -> String {
        let mut s = format!("{:?}", self.handshake.phase());
        if let Some(name) = self.handshake.identity().user_name.as_ref() {
            // Client-supplied: escaped and capped (SEC-FIX 12).
            s.push_str(&format!(" user={}", crate::handshake::loggable_bytes(name)));
        }
        s.push_str(&format!(" client={}", self.session.client_id));
        match self.session.character_id {
            Some(id) => s.push_str(&format!(" char={id}")),
            // Worth saying out loud rather than omitting: a connection that
            // reached the end of its life without selecting a character
            // answered every service call from its no-character branch, and
            // that is the first thing to check when a client shows an empty
            // hangar.
            None => s.push_str(" char=none"),
        }
        if self.last_object_id > 0 {
            // The last bound-object serial this connection took out of the
            // process-global counter, so two connections in one log can be
            // told apart by the ids they handed out.
            s.push_str(&format!(
                " last-object=N={}:{}",
                self.handshake.node_id(),
                self.last_object_id
            ));
        }
        if self.post_handshake_payloads > 0 {
            s.push_str(&format!(
                " post-handshake={} unknown-tags={:?}",
                self.post_handshake_payloads, self.unknown_tags
            ));
        }
        if !self.unanswered.is_empty() {
            // `name x3` rather than three copies of `name`. The multiplicity
            // is the interesting part: one occurrence is a gap, a hundred is
            // a live lock.
            let listed: Vec<String> = self
                .unanswered
                .iter()
                .map(|(what, n)| {
                    if *n == 1 {
                        what.clone()
                    } else {
                        format!("{what} x{n}")
                    }
                })
                .collect();
            s.push_str(&format!(" unanswered=[{}]", listed.join(", ")));
        }
        s
    }
}

/// **Shutdown's save** (CP52): the world's last dirty state, rendered under
/// the world lock, then [`crate::store::Store::close`], which writes it and
/// everything queued before it and releases the world's lock file.
///
/// Payload turns no longer save, so without this a clean shutdown would
/// lose up to `push::SAVE_EVERY` of play. A poisoned lock is recovered and
/// the world tainted, as a turn does (IP9), so the save goes to
/// `<file>.tainted` and never over the good file. A kill skips this, and
/// then the disk is at most `SAVE_EVERY` behind.
pub fn flush_world(world: &RwLock<Registry>, store: &dyn crate::store::Store) {
    {
        let asked = Instant::now();
        let waiting = crate::clock_probe::Waiter::enter();
        let mut w = world.write().unwrap_or_else(|e| {
            let mut w = e.into_inner();
            w.taint();
            w
        });
        drop(waiting);
        let acquired = Instant::now();
        crate::chat_dir::drain_dirty(&mut w);
        if w.take_dirty()
            && let Err(e) = store.save_owned(w.to_snapshot()).map(|_| ())
        {
            crate::errln!("[world] shutdown save failed: {e}");
        }
        crate::clock_probe::note_other(
            "flush_world",
            acquired.saturating_duration_since(asked),
            acquired.elapsed(),
        );
    }
    store.close();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store as _;
    use std::sync::mpsc;

    fn world() -> Arc<RwLock<Registry>> {
        Arc::new(RwLock::new(Registry::new(
            crate::model::SandboxConfig::default(),
        )))
    }

    fn session(world: &Arc<RwLock<Registry>>) -> GpsSession {
        GpsSession::new(
            Handshake::new(crate::handshake::Config::default(), [7u8; 64]),
            Session::default(),
            Arc::clone(world),
        )
    }

    /// A `CallReq` whose inner pickle is a well-formed service call.
    fn call_req_payload() -> Vec<u8> {
        let call = CallPayload::Service {
            method: "GetTime".to_owned(),
            args: vec![],
            kwargs: vec![],
        };
        MachoPacket::new(
            MachoNetMsg::CallReq,
            MachoAddress::client(1),
            MachoAddress::node(1),
            vec![call.to_payload()],
        )
        .to_payload()
        .expect("a CallReq payload")
    }

    /// Hold the world's write lock on another thread until the returned
    /// sender is dropped or sent to.
    fn hold_write_lock(world: &Arc<RwLock<Registry>>) -> mpsc::Sender<()> {
        let (release, wait) = mpsc::channel::<()>();
        let (held, is_held) = mpsc::channel::<()>();
        let w = Arc::clone(world);
        std::thread::spawn(move || {
            let _guard = w.write().expect("unpoisoned");
            held.send(()).expect("test alive");
            let _ = wait.recv();
        });
        is_held.recv().expect("holder started");
        release
    }

    #[test]
    fn loads_runs_with_the_world_lock_free() {
        let world = world();
        let release = hold_write_lock(&world);

        // The inner call pickle decodes too, with the lock held elsewhere.
        match GpsSession::decode(true, &call_req_payload()).expect("decodes") {
            Inbound::Packet(d) => assert!(
                matches!(d.call, Some(Ok(CallPayload::Service { .. }))),
                "the inner call is decoded before the lock: {:?}",
                d.call
            ),
            other => panic!("expected a packet, got {other:?}"),
        }

        // And through `on_payload`: a payload that fails to decode returns
        // without waiting for the lock. Before the split this blocked until
        // the holder let go.
        let mut gps = session(&world);
        let (done, result) = mpsc::channel();
        std::thread::spawn(move || {
            let r = gps.on_payload(&[0xFF, 0x00]);
            let _ = done.send(r.is_err());
        });
        let refused = result
            .recv_timeout(Duration::from_secs(10))
            .expect("on_payload decoded without waiting for the world lock");
        assert!(refused, "a non-marshal handshake payload is refused");
        assert!(
            world.try_read().is_err(),
            "the holder still had the lock when on_payload returned"
        );
        drop(release);
    }

    /// BA-4: the session carries the client's exact build once logged in,
    /// for every build the handshake accepts.
    #[test]
    fn the_session_carries_the_clients_build() {
        use crate::pyutil::pydict;
        use eve_marshal::build as reg;
        let b = |s: &str| PyValue::Bytes(s.as_bytes().to_vec());
        for build in reg::accepted() {
            let world = world();
            let mut gps = session(&world);
            gps.greeting();
            assert_eq!(gps.session().client_build, None);
            let line = reg::reference().line;
            let h6 = PyValue::Tuple(vec![
                PyValue::Bytes(vec![0x11; crate::handshake::CHALLENGE_LEN]),
                pydict(vec![
                    ("boot_build", PyValue::Int(i64::from(build))),
                    ("user_name", b("EVE-SSO-CONNECTION")),
                    ("user_sso_token", b("invented")),
                ]),
            ]);
            let h9 = PyValue::Tuple(vec![
                PyValue::Bytes(
                    crate::wire::crypto_hash(&[PyValue::Bytes(vec![7u8; 64])]).into_bytes(),
                ),
                b(""),
                PyValue::Dict(vec![]),
            ]);
            for v in [
                PyValue::Tuple(vec![
                    PyValue::Int(i64::from(line.secret)),
                    PyValue::Int(i64::from(line.macho)),
                    PyValue::Int(0),
                    PyValue::Real(line.version),
                    PyValue::Int(i64::from(build)),
                    b(line.codename_at_region),
                ]),
                PyValue::Tuple(vec![PyValue::None, b("VK"), b("x")]),
                PyValue::Tuple(vec![b("placebo"), PyValue::Dict(vec![])]),
                h6,
                h9,
            ] {
                gps.on_payload(&dumps(&v)).expect("handshake step");
            }
            assert_eq!(gps.session().client_build, Some(i64::from(build)));
        }
    }

    #[test]
    fn unanswered_folds_past_the_key_cap() {
        let world = world();
        let mut gps = session(&world);
        let total = 10_000;
        for i in 0..total {
            gps.note_unanswered(format!("distinct {i}"));
        }
        assert_eq!(gps.unanswered.len(), UNANSWERED_KEYS + 1);
        assert_eq!(
            gps.unanswered.get(UNANSWERED_OTHER),
            Some(&((total - UNANSWERED_KEYS) as u64))
        );
        assert_eq!(gps.unanswered.values().sum::<u64>(), total as u64);
        // A key kept before the cap still counts under its own name.
        gps.note_unanswered("distinct 0".to_owned());
        assert_eq!(gps.unanswered.get("distinct 0"), Some(&2));
    }

    /// **A world-minted object resolves on a connection that never bound
    /// it** (GAP-TRADE): the trade session is minted by the world during
    /// Alice's call and Bob's connection calls it by the id in the push.
    #[test]
    fn a_world_minted_object_resolves_on_another_connection() {
        use crate::model::Character;
        use crate::session_table::SessionEntry;
        const ALICE: i64 = 90_000_811;
        const BOB: i64 = 90_000_812;
        let pilot = |id: i64, ship: i64| Character {
            character_id: id,
            ship_id: ship,
            station_id: Some(60_000_811),
            ..Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![pilot(ALICE, 100_000_811), pilot(BOB, 100_000_812)],
            ..crate::model::SandboxConfig::default()
        });
        for (n, who) in [ALICE, BOB].into_iter().enumerate() {
            reg.session_opened(SessionEntry {
                client_id: n as i64 + 1,
                session_id: n as i64 + 1,
                user_id: who,
                character_id: Some(who),
            });
        }
        crate::trade::initiate(&mut reg, ALICE, BOB).expect("a trade");
        let id = reg.trades.of(BOB).expect("open").object_id.clone();
        let world = Arc::new(RwLock::new(reg));

        let mut bob = logged_in(&world);
        bob.session.character_id = Some(BOB);
        bob.bound_objects.clear();
        bob.unanswered.clear();
        assert!(bob.bound_objects.is_empty(), "Bob never bound anything");
        let reply = bob
            .on_payload(&node_call(&id, "Abort", vec![]))
            .expect("routed");
        assert_eq!(reply.payloads.len(), 1);
        let unknown = |g: &GpsSession| {
            g.unanswered
                .keys()
                .filter(|k| k.starts_with("call on unknown object"))
                .count()
        };
        assert_eq!(unknown(&bob), 0, "{:?}", bob.unanswered);
        // It did the call: Abort ended the trade.
        assert!(world.read().expect("lock").trades.is_empty());
        // An id nobody minted is still the unknown-object answer.
        bob.on_payload(&node_call("N=1:w999", "Abort", vec![]))
            .expect("answered");
        assert_eq!(unknown(&bob), 1);
    }

    #[test]
    fn bound_objects_stop_growing_at_the_cap() {
        let world = world();
        let mut gps = session(&world);
        for _ in 0..BOUND_CAP + 3 {
            gps.mint_object("invbroker", PyValue::None);
        }
        assert_eq!(gps.bound_objects.len(), BOUND_CAP);
        assert!(
            gps.unanswered
                .keys()
                .any(|k| k.starts_with("bound-object cap reached")),
            "hitting the cap is recorded"
        );
    }

    #[test]
    fn a_recovered_poison_taints_the_world() {
        let world = world();
        let w = Arc::clone(&world);
        let _ = std::thread::spawn(move || {
            let _guard = w.write().expect("unpoisoned");
            panic!("dispatch::tests: an arm panicked holding the world, deliberately");
        })
        .join();
        assert!(world.is_poisoned());
        assert!(!world.read().unwrap_or_else(|e| e.into_inner()).tainted());

        // Any marshal payload reaches the lock; what the handshake makes of
        // it does not matter here.
        let mut gps = session(&world);
        let _ = gps.on_payload(&dumps(&PyValue::Int(0)));

        assert!(
            world.read().unwrap_or_else(|e| e.into_inner()).tainted(),
            "recovering the poisoned lock must taint the world"
        );
    }

    /// A store that counts saves and closes.
    #[derive(Default)]
    struct Counted {
        saves: std::sync::Mutex<Vec<crate::store::Snapshot>>,
        closed: std::sync::atomic::AtomicBool,
    }

    impl crate::store::Store for Counted {
        fn load(&self) -> std::io::Result<Option<crate::store::Snapshot>> {
            Ok(None)
        }
        fn save(&self, snapshot: &crate::store::Snapshot) -> std::io::Result<()> {
            self.saves.lock().expect("saves").push(snapshot.clone());
            Ok(())
        }
        fn close(&self) {
            self.closed.store(true, Ordering::Relaxed);
        }
    }

    /// A clean world record, then a durable change the turn would have saved.
    fn dirty_world() -> (Arc<RwLock<Registry>>, crate::delta::World) {
        let world = world();
        world.write().expect("w").take_dirty();
        let minted = crate::delta::World::mint(crate::pyutil::filetime_now());
        world.write().expect("w").deltas_mut().set_world(minted);
        (world, minted)
    }

    /// Shutdown saves the dirty world once and closes the store; a clean
    /// world is not saved again, and the store is still closed.
    #[test]
    fn shutdown_flushes_the_last_snapshot_and_closes() {
        let (world, minted) = dirty_world();
        let store = Counted::default();
        flush_world(&world, &store);
        {
            let saves = store.saves.lock().expect("saves");
            assert_eq!(saves.len(), 1);
            assert_eq!(saves[0].deltas.world(), Some(&minted));
        }
        assert!(store.closed.load(Ordering::Relaxed));
        let again = Counted::default();
        flush_world(&world, &again);
        assert!(again.saves.lock().expect("saves").is_empty(), "clean");
        assert!(again.closed.load(Ordering::Relaxed));
    }

    /// Shutdown through the real saver: what it flushes is on disk when it
    /// returns, and the world file's lock is free only then.
    #[test]
    fn shutdown_writes_through_the_saver_and_frees_the_lock() {
        let dir = std::env::temp_dir().join(format!("eve-cp52-flush-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("world.dat");
        let lock = crate::account_book::lock_world(&path).expect("locks");
        let saver = crate::store::Saver::spawn(crate::store::FileStore::new(&path), Some(lock))
            .expect("spawns");
        let (world, minted) = dirty_world();
        assert!(crate::account_book::lock_world(&path).is_err(), "held");
        flush_world(&world, &saver);
        let back = crate::store::FileStore::new(&path)
            .load()
            .expect("loads")
            .expect("a world");
        assert_eq!(back.deltas.world(), Some(&minted));
        assert!(crate::account_book::lock_world(&path).is_ok(), "released");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A killed server reloads at most `SAVE_EVERY` old** (CP52), and a
    /// clock-only change is what it reloads: no payload, no shutdown. The
    /// world changes before the clock starts; within `SAVE_EVERY` (plus a
    /// tick's slack for the saver's write) the file on disk carries it, read
    /// by a fresh store as a restarted server would after a kill.
    #[test]
    fn a_killed_server_reloads_at_most_save_every_old() {
        let dir = std::env::temp_dir().join(format!("eve-cp52-kill-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("world.dat");
        let saver =
            crate::store::Saver::spawn(crate::store::FileStore::new(&path), None).expect("spawns");
        let (world, minted) = dirty_world();
        let changed = Instant::now();
        crate::push::spawn_world_ticker(
            Arc::clone(&world),
            Arc::new(crate::push::Writers::default()),
            1,
            Arc::new(saver),
            None,
        )
        .expect("spawn world ticker");
        let bound = crate::push::SAVE_EVERY + crate::push::TICK * 20;
        let on_disk = loop {
            let back = crate::store::FileStore::new(&path).load().ok().flatten();
            if back
                .as_ref()
                .is_some_and(|b| b.deltas.world() == Some(&minted))
            {
                break changed.elapsed();
            }
            assert!(changed.elapsed() < bound, "not on disk within {bound:?}");
            std::thread::sleep(crate::push::TICK);
        };
        assert!(on_disk <= bound, "{on_disk:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_preauth_timeout_applies_until_login() {
        let world = world();
        let gps = session(&world).with_preauth_timeout(Duration::from_millis(5));
        assert_eq!(gps.idle_timeout(), Some(Duration::from_millis(5)));
        assert_eq!(session(&world).idle_timeout(), Some(PREAUTH_TIMEOUT));
    }

    // ---- the turn trace (TT-A) ----

    /// A session that has finished the handshake (the sequence
    /// `the_session_carries_the_clients_build` walks).
    fn logged_in(world: &Arc<RwLock<Registry>>) -> GpsSession {
        logged_in_with(session(world), "invented")
    }

    /// [`logged_in`] for a session built by the caller (its handshake config,
    /// its writers, its client id) and logging in with `token`.
    fn logged_in_with(mut gps: GpsSession, token: &str) -> GpsSession {
        use crate::pyutil::pydict;
        use eve_marshal::build as reg;
        let b = |s: &str| PyValue::Bytes(s.as_bytes().to_vec());
        let build = reg::accepted()[0];
        gps.greeting();
        let line = reg::reference().line;
        let h6 = PyValue::Tuple(vec![
            PyValue::Bytes(vec![0x11; crate::handshake::CHALLENGE_LEN]),
            pydict(vec![
                ("boot_build", PyValue::Int(i64::from(build))),
                ("user_name", b("EVE-SSO-CONNECTION")),
                ("user_sso_token", b(token)),
            ]),
        ]);
        let h9 = PyValue::Tuple(vec![
            PyValue::Bytes(crate::wire::crypto_hash(&[PyValue::Bytes(vec![7u8; 64])]).into_bytes()),
            b(""),
            PyValue::Dict(vec![]),
        ]);
        for v in [
            PyValue::Tuple(vec![
                PyValue::Int(i64::from(line.secret)),
                PyValue::Int(i64::from(line.macho)),
                PyValue::Int(0),
                PyValue::Real(line.version),
                PyValue::Int(i64::from(build)),
                b(line.codename_at_region),
            ]),
            PyValue::Tuple(vec![PyValue::None, b("VK"), b("x")]),
            PyValue::Tuple(vec![b("placebo"), PyValue::Dict(vec![])]),
            h6,
            h9,
        ] {
            gps.on_payload(&dumps(&v)).expect("handshake step");
        }
        gps
    }

    /// A `CallReq` to a service by name, as `ServiceCallGPCS` builds it.
    fn service_call(service: &str, method: &str, args: Vec<PyValue>) -> Vec<u8> {
        let call = CallPayload::Service {
            method: method.to_owned(),
            args,
            kwargs: vec![],
        };
        MachoPacket::new(
            MachoNetMsg::CallReq,
            MachoAddress::Client {
                client_id: 1,
                call_id: Some(1),
                service: None,
            },
            MachoAddress::Any {
                service: Some(service.to_owned()),
                call_id: Some(1),
            },
            vec![call.to_payload()],
        )
        .to_payload()
        .expect("a CallReq payload")
    }

    fn probe_push() -> crate::notify::InvalidateLocations {
        crate::notify::InvalidateLocations {
            location_ids: vec![60_000_001],
        }
    }

    /// The window's worst turn, taking the window.
    fn worst_turn(world: &Arc<RwLock<Registry>>) -> crate::live_status::SlowTurn {
        let mut w = world.write().expect("unpoisoned");
        crate::live_status::take_turns(&mut w)
            .tail
            .worst
            .expect("a traced turn")
    }

    /// TT-B C1: the relay to eve-bots waits on a TCP peer; the world lock is
    /// free while it does, and the reply is the one the inline path builds.
    #[test]
    fn a_bots_relay_does_not_hold_the_world() {
        use std::io::{BufRead, BufReader, Write};
        let link = crate::bot_control::Link::open(None).expect("link");
        let mut peer = std::net::TcpStream::connect(link.addr()).expect("connects");
        peer.write_all(format!("HELLO {}\n", link.token()).as_bytes())
            .unwrap();
        let mut r = BufReader::new(peer.try_clone().unwrap());
        let mut first = String::new();
        r.read_line(&mut first).unwrap();
        assert_eq!(first.trim(), "OK");
        std::thread::spawn(move || {
            let mut line = String::new();
            while r.read_line(&mut line).unwrap_or(0) > 0 {
                let rest = line.trim_end().strip_prefix("CMD ").unwrap().to_owned();
                let (id, _) = rest.split_once(' ').unwrap();
                std::thread::sleep(Duration::from_millis(300));
                peer.write_all(format!("REPLY {id} ok\n").as_bytes())
                    .unwrap();
                line.clear();
            }
        });
        let world = world();
        {
            let mut w = world.write().expect("unpoisoned");
            crate::bot_control::attach(&mut w, true, Some(link));
        }
        let mut gps = logged_in(&world);
        gps.session.role = crate::roles::ROLE_GML;
        let call = service_call(
            "slash",
            "SlashCmd",
            vec![PyValue::Bytes(b"/bots list ".to_vec())],
        );
        let t = std::thread::spawn({
            let call = call.clone();
            move || gps.on_payload(&call)
        });
        std::thread::sleep(Duration::from_millis(50));
        let asked = Instant::now();
        drop(world.write().expect("unpoisoned"));
        let waited = asked.elapsed();
        assert!(waited < Duration::from_millis(100), "{waited:?}");
        let reply = t.join().unwrap().expect("answered");
        // The same bytes the inline path gives for the text "ok".
        let packet = MachoPacket::from_value(&loads(&call).unwrap()).unwrap();
        let Err(ServiceError::Raise(e)) =
            crate::service::gm::visible_reply("/bots list", crate::pyutil::bstr("ok"))
        else {
            panic!("a /bots answer raises CustomInfo");
        };
        let want = GpsSession::error_reply(&packet, &e).unwrap().payloads;
        assert_eq!(reply.payloads, want);
    }

    #[test]
    fn a_leftover_push_is_counted_as_left() {
        let world = world();
        let mut gps = logged_in(&world);
        {
            let mut w = world.write().expect("unpoisoned");
            crate::live_status::take_turns(&mut w);
            for _ in 0..3 {
                w.push(probe_push());
            }
        }
        gps.on_payload(&service_call("machoNet", "GetTime", vec![]))
            .expect("a call");
        let tail = worst_turn(&world).tail;
        assert_eq!(tail.left, 3, "{tail:?}");
        assert!(tail.queued >= 3, "{tail:?}");
        assert_eq!(tail.pushes, 3);
        assert_eq!(tail.frames_inline, 3);
        assert!(tail.bytes > 0);
    }

    #[test]
    fn a_bind_counts_its_log_line() {
        let world = world();
        let mut gps = logged_in(&world);
        {
            let mut w = world.write().expect("unpoisoned");
            crate::live_status::take_turns(&mut w);
        }
        let bind_params = PyValue::Tuple(vec![PyValue::Int(90_000_001), PyValue::Int(1)]);
        let fused = PyValue::Tuple(vec![
            PyValue::Bytes(b"GetCharacterLoadSlot".to_vec()),
            PyValue::Tuple(vec![PyValue::Int(90_000_001)]),
            PyValue::Dict(vec![]),
        ]);
        gps.on_payload(&service_call(
            "populationCap",
            "MachoBindObject",
            vec![bind_params, fused],
        ))
        .expect("a bind");
        // The logger is uninitialised in a unit test, so this is a direct
        // `println!`, timed all the same.
        let tail = worst_turn(&world).tail;
        assert!(tail.log_lines >= 1, "{tail:?}");
        assert!(tail.log > Duration::ZERO, "{tail:?}");
        assert!(tail.log <= Duration::from_secs(5));
    }

    #[test]
    fn frames_posted_counts_deliver_elsewhere() {
        let world = world();
        let mut gps = logged_in(&world).with_writers(Arc::new(crate::push::Writers::default()));
        {
            let mut w = world.write().expect("unpoisoned");
            crate::live_status::take_turns(&mut w);
            w.session_opened(crate::session_table::SessionEntry {
                client_id: 777_001,
                session_id: 5,
                user_id: 9,
                character_id: Some(2002),
            });
            w.push(crate::notify::Addressed {
                to: Some(vec![2002]),
                inner: Box::new(probe_push()),
            });
        }
        gps.on_payload(&service_call("machoNet", "GetTime", vec![]))
            .expect("a call");
        let tail = worst_turn(&world).tail;
        assert_eq!(tail.frames_posted, 1, "{tail:?}");
        assert_eq!(tail.frames_inline, 0, "{tail:?}");
        assert!(tail.bytes > 0);
    }

    /// The trace is bookkeeping around the turn: the frames a turn returns
    /// are the bytes `notify` gives for the same pushes, in order.
    #[test]
    fn the_trace_does_not_change_the_reply() {
        let world = world();
        let mut gps = logged_in(&world);
        {
            let mut w = world.write().expect("unpoisoned");
            // The login's own turns are traced too; on a loaded machine one of
            // them could be the window's worst and carry no frames. Start the
            // window here so the worst turn is the one under test.
            let _ = crate::live_status::take_turns(&mut w);
            for _ in 0..3 {
                w.push(probe_push());
            }
        }
        let reply = gps
            .on_payload(&service_call("machoNet", "GetTime", vec![]))
            .expect("a call");
        let (event, args) = crate::notify::Push::to_push(&probe_push());
        let expected = gps.notify(event, args).expect("a frame");
        assert_eq!(reply.pushes.len(), 3);
        for (after, frame) in &reply.pushes {
            assert_eq!(*after, Duration::ZERO);
            assert_eq!(frame, &expected);
        }
        let tail = worst_turn(&world).tail;
        assert_eq!(tail.bytes, 3 * expected.len() as u64);
    }

    /// A Destiny batch of fixed entries (PA-10).
    #[derive(Debug)]
    struct DestinyProbe(i64);

    impl crate::notify::Push for DestinyProbe {
        fn to_push(&self) -> (&'static str, Vec<PyValue>) {
            let entry = PyValue::Tuple(vec![
                PyValue::Int(77),
                PyValue::Tuple(vec![
                    PyValue::Bytes(b"Orbit".to_vec()),
                    PyValue::Tuple(vec![PyValue::Int(self.0)]),
                ]),
            ]);
            (
                "DoDestinyUpdate",
                vec![PyValue::List(vec![entry]), PyValue::Int(0)],
            )
        }

        fn grid(&self) -> crate::grid::GridRole {
            // A role is required of every Destiny push; this one reaches
            // whoever it is addressed to.
            crate::grid::GridRole::Resets(Vec::new())
        }
    }

    /// PA-10 T9: with coalescing on, another connection is queued parts (two
    /// same-stamp pushes, one outbox entry) and the acting connection's
    /// inline frames are the bytes they are with it off.
    #[test]
    fn deliver_elsewhere_posts_parts_and_leaves_the_acting_frames_alone() {
        use std::net::{TcpListener, TcpStream};
        let run = |on: bool| {
            crate::push::force_coalesce_for_test(on);
            let world = world();
            let writers = Arc::new(crate::push::Writers::default());
            let mut gps = logged_in(&world).with_writers(Arc::clone(&writers));
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let ours = TcpStream::connect(listener.local_addr().expect("addr")).expect("connect");
            let (_peer, _) = listener.accept().expect("accept");
            let wire = Arc::new(std::sync::Mutex::new(crate::push::Wire::new(ours, None)));
            writers.register(777_001, &wire);
            gps.session.character_id = Some(2001);
            let acting = gps.session.character_id;
            {
                let mut w = world.write().expect("unpoisoned");
                w.session_opened(crate::session_table::SessionEntry {
                    client_id: 777_001,
                    session_id: 5,
                    user_id: 9,
                    character_id: Some(2002),
                });
                for n in 1..=2 {
                    let mut to = vec![2002];
                    to.extend(acting);
                    w.push(crate::notify::Addressed {
                        to: Some(to),
                        inner: Box::new(DestinyProbe(n)),
                    });
                }
            }
            let reply = gps
                .on_payload(&service_call("machoNet", "GetTime", vec![]))
                .expect("a call");
            let queued = wire.lock().expect("wire").queued();
            crate::push::force_coalesce_for_test(false);
            (reply.pushes, queued)
        };
        let (off_pushes, off_queued) = run(false);
        let (on_pushes, on_queued) = run(true);
        assert_eq!(off_queued, 2, "one frame per push, as before");
        assert_eq!(on_queued, 1, "two same-stamp parts, one entry");
        assert_eq!(
            off_pushes.len(),
            2,
            "the acting connection has its inline frames"
        );
        assert_eq!(on_pushes, off_pushes, "the acting connection is untouched");
    }

    /// destiny-batch: N pilots each move in their own payload turn, the
    /// turns some ms apart (more than `MERGE_DUE_SLACK`), all in one Destiny
    /// stamp. A viewer on the grid is queued **one** `DoDestinyUpdate` with
    /// the N entries in turn order, not N one-entry frames (the owner's
    /// client logged 1300 of those for one tick on 2026-11-03).
    #[test]
    fn n_pilots_moving_in_one_stamp_give_a_viewer_one_update_in_order() {
        use std::net::{TcpListener, TcpStream};
        const N: i64 = 5;
        const VIEWER: i64 = 2002;
        crate::push::force_coalesce_for_test(true);
        let world = world();
        let writers = Arc::new(crate::push::Writers::default());
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let ours = TcpStream::connect(listener.local_addr().expect("addr")).expect("connect");
        let (_peer, _) = listener.accept().expect("accept");
        let wire = Arc::new(std::sync::Mutex::new(crate::push::Wire::new(ours, None)));
        writers.register(777_001, &wire);
        world
            .write()
            .expect("unpoisoned")
            .session_opened(crate::session_table::SessionEntry {
                client_id: 777_001,
                session_id: 5,
                user_id: 9,
                character_id: Some(VIEWER),
            });
        for n in 1..=N {
            let mut gps = logged_in(&world).with_writers(Arc::clone(&writers));
            gps.session.character_id = Some(3000 + n);
            world
                .write()
                .expect("unpoisoned")
                .push(crate::notify::Addressed {
                    to: Some(vec![VIEWER, 3000 + n]),
                    inner: Box::new(DestinyProbe(n)),
                });
            gps.on_payload(&service_call("machoNet", "GetTime", vec![]))
                .expect("a call");
            std::thread::sleep(crate::push::MERGE_DUE_SLACK * 2);
        }
        let parts = wire
            .lock()
            .expect("wire")
            .outbox()
            .lock()
            .expect("outbox")
            .queued_parts();
        crate::push::force_coalesce_for_test(false);
        assert_eq!(parts.len(), 1, "one frame for the stamp: {parts:?}");
        let order: Vec<i64> = parts[0]
            .as_ref()
            .expect("a destiny part")
            .iter()
            .map(|e| match e {
                PyValue::Tuple(t) => match &t[1] {
                    PyValue::Tuple(call) => match &call[1] {
                        PyValue::Tuple(a) => match a[0] {
                            PyValue::Int(n) => n,
                            _ => panic!("{e:?}"),
                        },
                        _ => panic!("{e:?}"),
                    },
                    _ => panic!("{e:?}"),
                },
                _ => panic!("{e:?}"),
            })
            .collect();
        assert_eq!(order, (1..=N).collect::<Vec<_>>(), "the turns' order");
    }

    // ---- read turns (PA-12a) ----

    const PILOT: i64 = 1_000_001;
    const SYSTEM: i64 = 30_000_001;

    thread_local! {
        static SERIAL: std::cell::Cell<i64> = const { std::cell::Cell::new(1) };
    }

    fn fixed_serial() -> i64 {
        SERIAL.with(std::cell::Cell::get)
    }

    /// A world with one pilot in a system holding one ore-archetype site.
    fn read_world() -> Arc<RwLock<Registry>> {
        read_world_with(None)
    }

    fn read_world_with(content: Option<Arc<crate::content::Content>>) -> Arc<RwLock<Registry>> {
        use crate::model::{Character, SandboxConfig};
        use crate::space_state::{Motion, Presence};
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![Character {
                character_id: PILOT,
                ship_id: 100_000_001,
                solar_system_id: Some(SYSTEM),
                ..Character::default()
            }],
            ..SandboxConfig::default()
        });
        if let Some(content) = content {
            reg = reg.with_content(content);
        }
        reg.roster_mut().arrive(Presence::arriving(
            PILOT,
            100_000_001,
            SYSTEM,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        crate::signal_tracker::add_site(
            &mut reg,
            crate::signal_tracker::Site {
                site_id: 7001,
                instance_id: 7002,
                dungeon_id: 999_304,
                solar_system_id: SYSTEM,
                position: [1.0e9, 2.0e9, 3.0e9],
                group_id: crate::signal_tracker::GROUP_ANOMALY,
                is_scannable: true,
                scan_strength_attribute: 9_001,
                scan_strength_value: 40.0,
                signature_radius: 100.0,
                target_id: "ABC-123".to_owned(),
                allowed_types: vec![],
            },
        );
        Arc::new(RwLock::new(reg))
    }

    /// Two logged-in sessions on the same world, with the pilot selected on
    /// both: `ro` serves reads under the read lock, `rw` is the oracle.
    fn read_pair(world: &Arc<RwLock<Registry>>) -> (GpsSession, GpsSession) {
        let mut ro = logged_in(world)
            .with_serial(fixed_serial)
            .with_read_turns(true);
        let mut rw = logged_in(world)
            .with_serial(fixed_serial)
            .with_read_turns(false);
        for gps in [&mut ro, &mut rw] {
            gps.on_payload(&service_call(
                "charUnboundMgr",
                "SelectCharacterID",
                vec![PyValue::Int(PILOT)],
            ))
            .expect("select the pilot");
            assert_eq!(gps.session.character_id, Some(PILOT));
        }
        (ro, rw)
    }

    fn node_call(object_id: &str, method: &str, args: Vec<PyValue>) -> Vec<u8> {
        let call = CallPayload::BoundObject {
            object_id: object_id.to_owned(),
            method: method.to_owned(),
            args,
            kwargs: vec![],
        };
        MachoPacket::new(
            MachoNetMsg::CallReq,
            MachoAddress::Client {
                client_id: 1,
                call_id: Some(2),
                service: None,
            },
            MachoAddress::Node {
                node_id: Some(1),
                service: None,
                call_id: Some(2),
            },
            vec![call.to_payload()],
        )
        .to_payload()
        .expect("a bound CallReq")
    }

    fn ping_payload() -> Vec<u8> {
        MachoPacket::new(
            MachoNetMsg::PingReq,
            MachoAddress::client(1),
            MachoAddress::node(1),
            vec![PyValue::List(vec![])],
        )
        .to_payload()
        .expect("a ping")
    }

    fn ping_rsp_payload() -> Vec<u8> {
        MachoPacket::new(
            MachoNetMsg::PingRsp,
            MachoAddress::client(1),
            MachoAddress::node(1),
            vec![PyValue::List(vec![])],
        )
        .to_payload()
        .expect("a ping response")
    }

    #[test]
    fn keepalive_ping_waits_for_login_and_its_answer_is_not_unanswered() {
        let w = world();
        assert!(session(&w).keepalive_ping().is_none(), "not before login");
        let mut gps = logged_in(&w);
        let ping = gps.keepalive_ping().expect("a ping after login");
        let packet = MachoPacket::from_value(&loads(&ping).expect("marshal")).expect("a packet");
        assert_eq!(packet.command, MachoNetMsg::PingReq);
        assert_eq!(packet.body, vec![PyValue::List(vec![])]);
        // The write path (a mutating session) and the read path agree: the
        // answer produces no reply and no unanswered note.
        let reply = gps.on_payload(&ping_rsp_payload()).expect("a PingRsp");
        assert!(reply.payloads.is_empty() && !reply.close);
        assert!(gps.unanswered.is_empty(), "{:?}", gps.unanswered);
    }

    fn release_payload(object_id: &str) -> Vec<u8> {
        let call = CallPayload::Release {
            objects: vec![(object_id.to_owned(), 0)],
        };
        MachoPacket::new(
            MachoNetMsg::Notification,
            MachoAddress::Node {
                node_id: None,
                service: None,
                call_id: None,
            },
            MachoAddress::node(1),
            vec![call.to_payload()],
        )
        .to_payload()
        .expect("a release")
    }

    /// Every request the read path may serve, then a few it may not, as
    /// `(bytes, eligible)`. The bound calls are on object ids minted by
    /// [`mint_bound`].
    fn read_requests(inv: &str) -> Vec<(Vec<u8>, bool)> {
        let svc = |s: &str, m: &str| (service_call(s, m, vec![]), true);
        vec![
            svc("config", "GetAverageMarketPrices"),
            svc("config", "GetAverageMarketPrices"),
            svc("dungeonInstanceCacheMgr", "GetOreAnomalyInstances"),
            svc("dungeonInstanceCacheMgr", "GetCombatAnomalyInstances"),
            svc("dungeonInstanceCacheMgr", "GetIceBeltInstances"),
            svc("dungeonInstanceCacheMgr", "GetOreAnomaliesCount"),
            svc("dungeonInstanceCacheMgr", "GetCombatAnomaliesCount"),
            svc("dungeonInstanceCacheMgr", "GetIceBeltsCount"),
            svc("dungeonInstanceCacheMgr", "GetSignatureInstances"),
            svc("skillMgr2", "GetMySkillHandler"),
            svc("invbroker", "GetInventory"),
            svc("invbroker", "GetInventoryFromId"),
            svc("machoNet", "MachoResolveObject"),
            (
                node_call(
                    inv,
                    "GetInventoryFromId",
                    vec![PyValue::Int(5), PyValue::Int(0)],
                ),
                true,
            ),
            (
                node_call(inv, "GetInventory", vec![PyValue::Int(6), PyValue::None]),
                true,
            ),
            (ping_payload(), true),
            (ping_rsp_payload(), true),
            // Not read-eligible: the same bytes either way.
            (service_call("config", "NoSuchThing", vec![]), false),
            (service_call("machoNet", "GetTime", vec![]), false),
            (node_call(inv, "NoSuchThing", vec![]), false),
            (node_call("N=1:99999", "GetInventory", vec![]), false),
        ]
    }

    /// Hand both sessions the same `invbroker` object (a bind never goes
    /// through the read path).
    fn mint_bound(ro: &mut GpsSession, rw: &mut GpsSession) -> String {
        SERIAL.with(|c| c.set(50));
        for gps in [&mut *ro, &mut *rw] {
            gps.mint_object(
                "invbroker",
                PyValue::Tuple(vec![PyValue::Int(7), PyValue::Int(1)]),
            );
        }
        ro.bound_objects.keys().next_back().expect("minted").clone()
    }

    fn assert_same_session_state(ro: &GpsSession, rw: &GpsSession, what: &str) {
        assert_eq!(ro.bound_objects, rw.bound_objects, "{what}");
        assert_eq!(ro.unanswered, rw.unanswered, "{what}");
        assert_eq!(
            ro.post_handshake_payloads, rw.post_handshake_payloads,
            "{what}"
        );
    }

    #[test]
    fn read_turns_answer_byte_for_byte_like_the_write_path() {
        crate::test_clock::freeze(Duration::from_secs(1_800_000_000));
        let world = read_world();
        let (mut ro, mut rw) = read_pair(&world);
        let inv = mint_bound(&mut ro, &mut rw);
        let mut eligible = 0;
        let mut serial = 100;
        let mut released = None;
        let mut requests = read_requests(&inv);
        for (i, (bytes, is_read)) in requests.drain(..).enumerate() {
            serial += 1;
            SERIAL.with(|c| c.set(serial));
            let a = ro.on_payload(&bytes).expect("ro");
            let b = rw.on_payload(&bytes).expect("rw");
            assert_eq!(a, b, "request {i}");
            assert_same_session_state(&ro, &rw, &format!("request {i}"));
            eligible += u64::from(is_read);
            assert_eq!(ro.read_turns_served(), eligible, "request {i}");
            assert_eq!(rw.read_turns_served(), 0);
            if i == 9 {
                // The handle the skill-handler call minted, for the release.
                released = ro
                    .bound_objects
                    .keys()
                    .find(|k| k.ends_with(&format!(":{serial}")))
                    .cloned();
            }
        }
        // A release notification for the minted id.
        let id = released.expect("the skill handler was minted");
        let n = release_payload(&id);
        assert_eq!(
            ro.on_payload(&n).expect("ro"),
            rw.on_payload(&n).expect("rw")
        );
        assert_same_session_state(&ro, &rw, "release");
        assert!(!ro.bound_objects.contains_key(&id));
        assert_eq!(ro.read_turns_served(), eligible + 1);
        // The agency rows really were non-trivial.
        let rows = service_call("dungeonInstanceCacheMgr", "GetOreAnomalyInstances", vec![]);
        let reply = ro.on_payload(&rows).expect("rows");
        assert!(
            reply.payloads[0].len() > 60,
            "{:?}",
            reply.payloads[0].len()
        );
        crate::test_clock::thaw();
    }

    /// PERF-BURST: the price dict is answered `Shared` and encoded after the
    /// guard drops; with a table of 300 invented rows the bytes still equal
    /// the write path's, and the read turn took no copy (the kept `Arc` is
    /// the only one left once the reply is built).
    #[test]
    fn a_shared_answer_is_encoded_like_the_owned_one() {
        crate::test_clock::freeze(Duration::from_secs(1_800_000_000));
        let mut c = crate::content::Content::fixture();
        let mut table = crate::econ_prices::Table::fixture();
        let proto = table.prices.values().next().expect("a row").clone();
        for i in 0..300 {
            let mut p = proto.clone();
            p.ref_price = 10.0 + i as f64;
            table.prices.insert(910_000 + i, p);
        }
        c.insert("econ_prices", table);
        let world = read_world_with(Some(Arc::new(c)));
        let (mut ro, mut rw) = read_pair(&world);
        let bytes = service_call("config", "GetAverageMarketPrices", vec![]);
        let a = ro.on_payload(&bytes).expect("ro");
        let b = rw.on_payload(&bytes).expect("rw");
        assert_eq!(a, b);
        assert!(a.payloads[0].len() > 5_000, "{}", a.payloads[0].len());
        assert_eq!(ro.read_turns_served(), 1);
        let w = world.read().expect("unpoisoned");
        let kept = crate::econ_prices::average_market_prices_shared(&w);
        assert_eq!(Arc::strong_count(&kept), 2, "the cache and this handle");
        crate::test_clock::thaw();
    }

    #[test]
    fn a_read_turn_leaves_the_world_as_it_found_it() {
        crate::test_clock::freeze(Duration::from_secs(1_800_000_000));
        let world = read_world();
        let (mut ro, mut rw) = read_pair(&world);
        let inv = mint_bound(&mut ro, &mut rw);
        let before = {
            let mut w = world.write().expect("unpoisoned");
            w.take_dirty();
            (w.to_snapshot(), w.deltas().revision())
        };
        let mut served = 0;
        for (i, (bytes, is_read)) in read_requests(&inv).into_iter().enumerate() {
            SERIAL.with(|c| c.set(100 + i as i64));
            ro.on_payload(&bytes).expect("ro");
            served += u64::from(is_read);
            assert_eq!(ro.read_turns_served(), served, "request {i}");
        }
        assert_eq!(ro.read_turns_served(), served);
        let mut w = world.write().expect("unpoisoned");
        assert!(!w.take_dirty(), "a read turn marked the world dirty");
        assert_eq!(w.deltas().revision(), before.1);
        assert!(w.to_snapshot() == before.0, "the snapshot moved");
        crate::test_clock::thaw();
    }

    #[test]
    fn queued_output_sends_a_read_back_to_the_write_path() {
        crate::test_clock::freeze(Duration::from_secs(1_800_000_000));
        let world = read_world();
        let (mut ro, mut rw) = read_pair(&world);
        let bytes = service_call("config", "GetAverageMarketPrices", vec![]);
        type Queue = fn(&mut Registry);
        let queues: [Queue; 2] = [
            |w| w.push(probe_push()),
            |w| w.queue_session_change(PILOT, vec![(PyValue::Int(1), PyValue::Int(2))]),
        ];
        for (n, queue) in queues.iter().enumerate() {
            let served = ro.read_turns_served();
            queue(&mut world.write().expect("unpoisoned"));
            let a = ro.on_payload(&bytes).expect("ro");
            queue(&mut world.write().expect("unpoisoned"));
            let b = rw.on_payload(&bytes).expect("rw");
            assert_eq!(a, b, "queue {n}");
            assert!(a.payloads.len() + a.pushes.len() > 1, "queue {n}: {a:?}");
            assert_eq!(ro.read_turns_served(), served, "queue {n}: a fallback");
            // And with nothing queued the very next one is a read turn.
            ro.on_payload(&bytes).expect("ro");
            assert_eq!(ro.read_turns_served(), served + 1, "queue {n}");
        }
        crate::test_clock::thaw();
    }

    #[test]
    fn read_turns_run_beside_a_reader_and_wait_for_a_writer() {
        let world = read_world();
        let mut ro = logged_in(&world).with_read_turns(true);
        let bytes = service_call("config", "GetAverageMarketPrices", vec![]);
        // A held reader does not stop a read turn.
        let reader = Arc::clone(&world);
        let (held, is_held) = mpsc::channel::<()>();
        let (release, wait) = mpsc::channel::<()>();
        let t = std::thread::spawn(move || {
            let _g = reader.read().expect("unpoisoned");
            held.send(()).expect("test alive");
            let _ = wait.recv();
        });
        is_held.recv().expect("reader started");
        let (done, result) = mpsc::channel();
        let h = std::thread::spawn(move || {
            ro.on_payload(&bytes).expect("a read turn");
            let _ = done.send(ro.read_turns_served());
            ro
        });
        let served = result
            .recv_timeout(Duration::from_secs(2))
            .expect("a read turn beside a held reader");
        assert_eq!(served, 1);
        let mut ro = h.join().expect("joined");
        drop(release);
        t.join().expect("reader done");
        // A held writer does.
        let bytes = service_call("config", "GetAverageMarketPrices", vec![]);
        let writer = hold_write_lock(&world);
        let (done, result) = mpsc::channel();
        let h = std::thread::spawn(move || {
            ro.on_payload(&bytes).expect("a read turn");
            let _ = done.send(());
        });
        assert!(
            result.recv_timeout(Duration::from_millis(300)).is_err(),
            "a read turn did not wait for the writer"
        );
        drop(writer);
        result
            .recv_timeout(Duration::from_secs(5))
            .expect("served after the writer let go");
        h.join().expect("joined");
    }

    #[test]
    fn read_arms_never_read_the_caller_field() {
        // The methods whose arms call `Registry::caller()`; it is
        // `Caller::Internal` outside `dispatch_as`. PA-12b converts
        // `GetCharacterSelectionData` and takes it off this list.
        let reads_caller = [
            "GetNumCharacters",
            "CreateCharacterWithDoll",
            "SlashCmd",
            "StartNewGameInstance",
            "GetCharacterSelectionData",
        ];
        for (methods, _) in crate::service::READS {
            for (service, method) in methods.iter() {
                assert!(
                    !reads_caller.contains(method),
                    "{service}.{method} reads the caller"
                );
            }
        }
    }

    #[test]
    fn reads_name_no_session_changing_method() {
        for (methods, _) in crate::service::READS {
            for (service, method) in methods.iter() {
                assert!(
                    ![
                        "Undock",
                        "CmdDock",
                        "CmdTurboDock",
                        "SelectCharacterID",
                        "UserLogOffCharacter",
                        "MachoBindObject"
                    ]
                    .contains(method),
                    "{service}.{method} changes the session"
                );
                assert!(!matches!(*service, "charUnboundMgr" | "userSvc"));
            }
        }
    }

    /// A source scan: the read path takes `world.read()` and never the write
    /// lock, and `record_turn` takes `&Registry`.
    #[test]
    fn the_read_turn_takes_no_write_lock() {
        let src = include_str!("dispatch.rs");
        let body = src
            .split("fn try_read_turn(")
            .nth(1)
            .and_then(|s| s.split("/// What every dispatched call").next())
            .expect("the function");
        assert!(body.contains("world.read()"));
        assert!(!body.contains(".write()"));
        assert!(!body.contains("&mut Registry"));
    }

    /// **PA-12a bench**: 8 sessions, each sending 200 warm
    /// `config.GetAverageMarketPrices` through `on_payload`, with read turns
    /// on and off. `STAGE_BENCH_CONTENT=DIR` for the real price table (the
    /// fixture answers a tiny dict). Release only.
    #[test]
    #[ignore]
    fn read_turn_bench() {
        let world = Arc::new(RwLock::new(crate::tick::stage_bench::world(500)));
        let payload = service_call("config", "GetAverageMarketPrices", vec![]);
        let size = {
            let mut gps = logged_in(&world);
            gps.on_payload(&payload).expect("warm").payloads[0].len()
        };
        for on in [false, true] {
            let t = Instant::now();
            let served: u64 = std::thread::scope(|s| {
                let hs: Vec<_> = (0..8)
                    .map(|_| {
                        s.spawn(|| {
                            let mut gps = logged_in(&world).with_read_turns(on);
                            for _ in 0..200 {
                                gps.on_payload(&payload).expect("a call");
                            }
                            gps.read_turns_served()
                        })
                    })
                    .collect();
                hs.into_iter().map(|h| h.join().expect("joined")).sum()
            });
            let stats = crate::live_status::peek_turns(&world.read().expect("unpoisoned"));
            let (n, hold) = if on {
                (stats.ro_turns, stats.ro_hold_total)
            } else {
                (stats.turns, stats.hold_total)
            };
            println!(
                "read_turn_bench: read_turns={on} reply {size} B, 8x200 calls in {:?} (served under read: {served}); mean hold {:.4} ms over {n} turns",
                t.elapsed(),
                hold.as_secs_f64() * 1e3 / n.max(1) as f64
            );
        }
    }

    /// 100k `GetTime` payloads through one session. Release only.
    #[test]
    #[ignore]
    fn turn_trace_bench() {
        let world = world();
        let mut gps = logged_in(&world);
        let payload = service_call("machoNet", "GetTime", vec![]);
        let t = Instant::now();
        for _ in 0..100_000 {
            gps.on_payload(&payload).expect("a call");
        }
        println!("turn_trace_bench: {:?}/turn", t.elapsed() / 100_000);
    }

    // ---- NET-TAKEOVER and UNMACHO ----

    fn tcp_pair() -> (std::net::TcpStream, std::net::TcpStream) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let ours = std::net::TcpStream::connect(l.local_addr().expect("addr")).expect("connect");
        let (peer, _) = l.accept().expect("accept");
        (ours, peer)
    }

    /// An accounts file of invented tokens, one account each: `(token, user)`.
    fn accounts_file(rows: &[(&str, i64)]) -> crate::handshake::Config {
        use crate::accounts::{hex, sha256};
        let lines: String = rows
            .iter()
            .map(|(t, u)| format!("{}\t{u}\tgm\tname{u}\n", hex(&sha256(t.as_bytes()))))
            .collect();
        crate::handshake::Config {
            accounts: Some(Arc::new(crate::accounts::Accounts::parse(&lines).unwrap())),
            ..crate::handshake::Config::default()
        }
    }

    /// One logged-in connection with a real socket pair registered in
    /// `writers`: the session, its wire (keep it alive) and the peer end.
    fn takeover_conn(
        world: &Arc<RwLock<Registry>>,
        writers: &Arc<crate::push::Writers>,
        client_id: i64,
        cfg: crate::handshake::Config,
        token: &str,
    ) -> (
        GpsSession,
        Arc<std::sync::Mutex<crate::push::Wire>>,
        std::net::TcpStream,
    ) {
        let mut gps = GpsSession::new(
            Handshake::new(cfg, [7u8; 64]),
            Session {
                client_id,
                ..Session::default()
            },
            Arc::clone(world),
        )
        .with_writers(Arc::clone(writers));
        let (ours, peer) = tcp_pair();
        let wire = Arc::new(std::sync::Mutex::new(crate::push::Wire::new(ours, None)));
        writers.register(client_id, &wire);
        gps = logged_in_with(gps, token);
        (gps, wire, peer)
    }

    /// Flush the writers once and say what the peer received: its bytes, and
    /// whether the server then hung up.
    fn drain(writers: &crate::push::Writers, peer: &mut std::net::TcpStream) -> (Vec<u8>, bool) {
        use std::io::Read as _;
        crate::push::sweep_shard(writers, 0, 1, Instant::now());
        peer.set_read_timeout(Some(Duration::from_millis(300)))
            .expect("timeout");
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match peer.read(&mut buf) {
                Ok(0) => return (got, true),
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(_) => return (got, false),
            }
        }
    }

    fn has(bytes: &[u8], needle: &str) -> bool {
        bytes.windows(needle.len()).any(|w| w == needle.as_bytes())
    }

    fn select(gps: &mut GpsSession, id: i64) {
        gps.on_payload(&service_call(
            "charUnboundMgr",
            "SelectCharacterID",
            vec![PyValue::Int(id)],
        ))
        .expect("select");
    }

    fn two_pilots_world() -> Arc<RwLock<Registry>> {
        let pilot = |id: i64| crate::model::Character {
            character_id: id,
            ship_id: id + 5,
            station_id: Some(60_000_011),
            ..crate::model::Character::default()
        };
        Arc::new(RwLock::new(Registry::new(crate::model::SandboxConfig {
            characters: vec![pilot(90_000_021), pilot(90_000_022)],
            ..crate::model::SandboxConfig::default()
        })))
    }

    /// A second login of one account closes the first (reason
    /// `CLIENTDISCONNECTED`, then end of stream); the second stays up. A
    /// different account is left alone.
    #[test]
    fn the_same_account_twice_closes_the_first() {
        let world = world();
        let writers = Arc::new(crate::push::Writers::default());
        let cfg = || accounts_file(&[("tok-a", 7001), ("tok-b", 7002)]);
        let (_a, _wa, mut pa) = takeover_conn(&world, &writers, 11, cfg(), "tok-a");
        let (_o, _wo, mut po) = takeover_conn(&world, &writers, 12, cfg(), "tok-b");
        let (_b, _wb, mut pb) = takeover_conn(&world, &writers, 13, cfg(), "tok-a");
        let (got, hung) = drain(&writers, &mut pa);
        assert!(has(&got, "CLIENTDISCONNECTED"), "{} bytes", got.len());
        assert!(hung, "the first connection is hung up");
        let (got, hung) = drain(&writers, &mut pb);
        assert!(!has(&got, "CLIENTDISCONNECTED") && !hung);
        let (got, hung) = drain(&writers, &mut po);
        assert!(!has(&got, "CLIENTDISCONNECTED") && !hung, "another account");
    }

    /// The catch-all shares one user id, so the account rule is off: two
    /// connections choosing two characters close nothing.
    #[test]
    fn catch_all_with_two_characters_closes_nothing() {
        let world = two_pilots_world();
        let writers = Arc::new(crate::push::Writers::default());
        let cfg = crate::handshake::Config::default;
        let (mut a, _wa, mut pa) = takeover_conn(&world, &writers, 21, cfg(), "x");
        let (mut b, _wb, mut pb) = takeover_conn(&world, &writers, 22, cfg(), "y");
        select(&mut a, 90_000_021);
        select(&mut b, 90_000_022);
        for p in [&mut pa, &mut pb] {
            let (got, hung) = drain(&writers, p);
            assert!(!has(&got, "CLIENTDISCONNECTED") && !hung);
        }
    }

    /// A bot's character takes several sessions: a human on it and the bot's
    /// own reconnect would otherwise evict each other for ever.
    #[test]
    fn a_bot_character_is_never_taken_over() {
        let world = two_pilots_world();
        world
            .write()
            .expect("lock")
            .character_mut(90_000_021)
            .expect("pilot")
            .is_bot = true;
        let writers = Arc::new(crate::push::Writers::default());
        let cfg = crate::handshake::Config::default;
        let (mut a, _wa, mut pa) = takeover_conn(&world, &writers, 41, cfg(), "x");
        let (mut b, _wb, mut pb) = takeover_conn(&world, &writers, 42, cfg(), "y");
        select(&mut a, 90_000_021);
        select(&mut b, 90_000_021);
        for p in [&mut pa, &mut pb] {
            let (got, hung) = drain(&writers, p);
            assert!(!has(&got, "CLIENTDISCONNECTED") && !hung);
        }
    }

    /// The same character chosen twice closes the older connection, the
    /// table holds one session for it afterwards, and the old connection's
    /// logoff does not wipe the new session's presence.
    #[test]
    fn the_same_character_twice_closes_the_first() {
        let world = two_pilots_world();
        let writers = Arc::new(crate::push::Writers::default());
        let cfg = crate::handshake::Config::default;
        let (mut a, _wa, mut pa) = takeover_conn(&world, &writers, 31, cfg(), "x");
        let (mut b, _wb, mut pb) = takeover_conn(&world, &writers, 32, cfg(), "y");
        select(&mut a, 90_000_021);
        select(&mut b, 90_000_021);
        let (got, hung) = drain(&writers, &mut pa);
        assert!(has(&got, "CLIENTDISCONNECTED") && hung);
        let (got, hung) = drain(&writers, &mut pb);
        assert!(!has(&got, "CLIENTDISCONNECTED") && !hung);
        // The old connection's thread now ends its session.
        {
            let mut w = world.write().expect("lock");
            assert_eq!(w.sessions().of_character(90_000_021).len(), 2);
            w.session_closed(31);
            let left = w.sessions().of_character(90_000_021);
            assert_eq!(left.len(), 1);
            assert_eq!(left[0].client_id, 32);
            assert!(w.sessions().is_online(90_000_021), "still online");
        }
    }

    /// An answered call on a released object is `UnMachoDestination`, so the
    /// client's `MonikeredCall` binds again; an id we never issued (another
    /// node, or past the counter) is still `None`.
    #[test]
    fn a_released_object_answers_unmacho_and_a_foreign_id_none() {
        let world = world();
        let mut gps = logged_in(&world);
        gps.unanswered.clear();
        gps.mint_object("invbroker", PyValue::None);
        let id = gps
            .bound_objects
            .keys()
            .next_back()
            .expect("minted")
            .clone();
        // Live: routed, no UnMacho.
        let live = gps
            .on_payload(&node_call(&id, "Anything", vec![]))
            .expect("live");
        assert!(!has(&live.payloads[0], "UnMachoDestination"));
        // Released (the client's `Release` notification does this).
        gps.bound_objects.remove(&id);
        let reply = gps
            .on_payload(&node_call(&id, "Anything", vec![]))
            .expect("gone");
        assert!(has(&reply.payloads[0], "UnMachoDestination"));
        let unknown = |g: &GpsSession| {
            g.unanswered
                .keys()
                .filter(|k| k.starts_with("call on unknown object"))
                .count()
        };
        assert_eq!(unknown(&gps), 0, "{:?}", gps.unanswered);
        // A world-minted object that is gone is the same.
        let (wid, _) = world
            .write()
            .expect("lock")
            .mint_world_object("tradeSession", PyValue::None);
        world
            .write()
            .expect("lock")
            .prune_world_objects(0, |_| false);
        let reply = gps
            .on_payload(&node_call(&wid, "Abort", vec![]))
            .expect("gone");
        assert!(has(&reply.payloads[0], "UnMachoDestination"));
        // Foreign: another node, a serial never minted.
        let node = gps.handshake.node_id();
        for foreign in [
            format!("N={}:5", node + 1000),
            format!("N={node}:{}", i64::MAX / 2),
        ] {
            let reply = gps
                .on_payload(&node_call(&foreign, "Anything", vec![]))
                .expect("none");
            assert!(!has(&reply.payloads[0], "UnMachoDestination"), "{foreign}");
        }
        assert_eq!(unknown(&gps), 2, "{:?}", gps.unanswered);
    }
}
