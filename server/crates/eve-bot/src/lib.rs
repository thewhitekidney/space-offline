//! A client of our own server, speaking the same wire the real client speaks.
//!
//! **Why this exists and what it is not.** `docs/archive/plan-bots.md` has the design;
//! the short version is that the end goal is player-like bots, and a bot that
//! cannot see the ballpark cannot fly. EvEJS's bots go through a web gateway
//! that drops `DoDestinyUpdate` **[V2]**, so they never receive ball state at
//! all; ours is a TCP client of `26000` and receives it like anybody else.
//!
//! **What it is not is an oracle for our own bytes.** This crate marshals with
//! `eve-marshal`, exactly as the server does, so if that encoder is wrong the
//! bot will decode its own mistake back perfectly and agree. Wire correctness
//! is `tools/client-ci.sh`'s job, because only the real client can judge it.
//! The bot is for behaviour, interaction and scale — many actors, long runs,
//! and the things one Wine client per player cannot reach.
//!
//! Milestone 1 of six: connect, complete the handshake, hold the session open.
//!
//! **P8a (`docs/lanes/bots.md` §4.1) widened it into a library a player-like
//! bot can be built on**, without changing anything the older tests use:
//!
//! - generic bound objects -- [`Conn::bind`], [`Conn::call_on`], and the
//!   client's own moniker pattern, [`Conn::moniker_call`] (bind with the call
//!   fused, then call the object it handed back);
//! - every frame read anywhere goes through [`Conn::recv`], which applies a
//!   session change to [`Conn::session`] and a `DoDestinyUpdate` to
//!   [`Conn::world`] -- so an unsolicited change (a fleet seat, a jump) is
//!   never lost, and a dock or a jump releases the bound park as the
//!   client's `Park.Release` does;
//! - [`Conn::pump`], a read that loses nothing, as [`Event`]s;
//! - identity by name: [`Conn::handshake_as`], [`Conn::characters`],
//!   [`Conn::select_character_named`];
//! - the player verbs, in `verbs.rs`, each one the client's own call shape.
//!
//! **P8b (`docs/lanes/bots.md` §4.2-4.4) put players on it**: [`brain`], a
//! pure `decide(memory, observation) -> acts` for the mining-with-a-player
//! loop, and [`driver`], one thread per bot that executes the acts as verbs
//! and folds the answers back -- reconnecting, with back-off, only when the
//! connection itself fails. `bin/eve-bots.rs` runs N of them;
//! `tools/bots.sh` runs them beside the server and your client.
//!
//! **P8c added a second role**, [`escort`]: a bot in a Rifter that flies on
//! its fleet boss's wing and kills the NPCs that threaten the fleet -- lock,
//! orbit, fire, loot, back to the boss, home to repair and re-arm -- and
//! [`chat`], the client's own XMPP login, so bots say a line in fleet chat
//! or Local when something happens (when the server's chat is on).

use eve_marshal::PyValue;
use eve_proto::{try_split_frame, write_frame};
use eve_server::{crypto_hash, dumps, loads};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// The one clock seam (#146). Becomes `pub use eve_server::clock;` once the
/// server's `lib.rs` declares the module itself.
pub use eve_server::game_clock::clock;

pub mod abyss_run;
pub mod acquire;
pub mod ammo;
pub mod arena;
pub mod brain;
pub mod bubble;
pub mod budget;
pub mod build_choice;
pub mod builder;
pub mod burst;
pub mod camp;
pub mod cap;
pub mod capital;
pub mod capital_fight;
pub mod career;
pub mod chat;
pub mod chatter;
#[cfg(test)]
mod clock_lint;
pub mod coalition;
pub mod cofound;
pub mod contact_plan;
pub mod contracts;
pub mod control;
pub mod corp_ads;
pub mod corp_ops;
pub mod corpmind;
pub mod counter;
pub mod courier;
pub mod defend;
pub mod demand;
pub mod dev;
pub mod difficulty;
pub mod doctrine;
pub mod doctrine_assign;
pub mod doctrine_exchange;
pub mod doctrine_fit;
pub mod doctrine_hall;
pub mod doctrine_lead;
pub mod doctrine_parts;
pub mod doctrine_real;
pub mod doctrine_report;
pub mod doctrine_sense;
pub mod doctrine_stock;
pub mod doctrine_theatre;
pub mod driver;
pub mod drone_use;
pub mod earnings;
pub mod effects;
pub mod emblem_use;
pub mod escort;
pub mod escort_board;
pub mod ess_guard;
pub mod ess_raid;
pub mod ewar_aim;
pub mod explore_roam;
pub mod filament;
pub mod fit_plan;
pub mod fleet;
pub mod fleet_hangar;
pub mod fleet_orders;
pub mod foreman;
pub mod friends;
pub mod gas_price;
pub mod gateway_client;
pub mod geode_haul;
mod gm;
pub mod hacker;
pub mod hauler;
pub mod hostile;
pub mod hub_supply;
pub mod huffer;
pub mod hull_choice;
pub mod hull_sale;
pub mod hulls;
pub mod human;
pub mod hunt;
pub mod ice;
pub mod insure;
pub mod know;
pub mod logi;
pub mod login_gate;
pub mod lp_spend;
pub mod mail;
pub mod mind;
pub mod mine_ladder;
pub mod missioner;
pub mod modules;
pub mod moon_pref;
pub mod nes_use;
pub mod observe;
pub mod offline;
pub mod ore;
pub mod overload;
pub mod park;
pub mod persona_log;
pub mod pi;
pub mod pi_p2;
pub mod pi_p3;
pub mod plex_trade;
pub mod plexer;
pub mod price_memory;
pub mod pricing;
pub mod probe;
pub mod profile;
pub mod progression;
pub mod pvp_kit;
pub mod rally;
pub mod rare_ore;
pub mod react;
pub mod reactor;
pub mod real_fits;
pub mod relations;
pub mod relocate;
pub mod rig;
pub mod roam_gang;
pub mod roles;
pub mod salvage;
pub mod scram;
pub mod session;
pub mod signal;
pub mod site_runner;
pub mod skill_plan;
pub mod society;
pub mod sov_bot;
pub mod speculate;
pub mod state;
pub mod stats;
pub mod summon;
pub mod supply;
pub mod supply_run;
pub mod t3;
pub mod tag_repair;
pub mod tank;
pub mod threat;
pub mod trader;
pub mod travel;
pub mod upwell_home;
mod verbs;
pub mod war_bot;
pub mod warp_bubble;
pub mod wh_dive;
pub mod wh_raid;
pub mod wing;
pub mod wing_fight;
pub mod wing_hall;

pub use park::{SlimInfo, Tracked, World};
pub use session::{GROUP_SOLAR_SYSTEM, GROUP_STATION, SessionChange, SessionView};
pub use verbs::{
    ATTRIBUTE_QUANTITY, CATEGORY_CHARGE, CONTAINER_HANGAR, CharacterSlot, DOCK_RANGE,
    EFFECT_ONLINE, FLAG_AUTOFIT, FLAG_CARGO, FLAG_HANGAR, FleetInvite, GodmaEffect, ItemRow,
    LOOT_RANGE, LockState, Looted, MIN_WARP_DISTANCE, MINING_LASER, ModuleRow,
    REPEAT_UNTIL_STOPPED, StructureRow, weapon_effect,
};

/// The handshake's challenge length, both directions.
const CHALLENGE_LEN: usize = 64;

/// The shortest a deferred action can take: the server's own clock period.
///
/// A lock answered `(1, [])` -- "not yet" -- cannot complete before the world
/// ticker next runs, so this is the floor a timing assertion can use without
/// copying a server constant into a test.
pub const TICK_FLOOR: Duration = Duration::from_millis(250);

/// How long to read while waiting for one reply before giving up.
///
/// A time, not a frame count: every notification that arrives first is one
/// more frame, and with 150 pilots in one system (the owner's +100 spawn,
/// 2026-10-28) a bot was sent more than the old cap of 32 before its reply
/// and dropped the connection. Finite, because an unattended bot that reads
/// for ever is indistinguishable from a hung one.
const REPLY_WAIT: Duration = Duration::from_secs(30);

/// What the client announces about itself. These are **our** build's values
/// `[V]` — the server checks `macho_version` and ends the connection with a
/// named reason when it disagrees.
const MACHO_VERSION: i64 = eve_marshal::build::REFERENCE_LINE.macho as i64;
const BOOT_VERSION: f64 = eve_marshal::build::REFERENCE_LINE.version;
/// The pinned build (`eve_marshal::build::REFERENCE`): a bot is a client of
/// the reference build, whichever builds the server also accepts.
const BOOT_BUILD: i64 = eve_marshal::build::REFERENCE as i64;
const BOOT_CODENAME: &str = eve_marshal::build::REFERENCE_LINE.codename;
const BOOT_REGION: &str = "ccp";
const USER_NAME: &str = "EVE-SSO-CONNECTION";

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// The server said something we cannot act on. Carries what we expected,
    /// because "the handshake failed" on its own has never helped anybody.
    Protocol(String),
    /// **Not sent, because the real client would not send it.** A verb whose
    /// client-side precondition fails -- docking from 4 km when the client's
    /// own gate is 2,500 m -- refuses here rather than ask the server a
    /// question no player can ask. Not a transport fault: the connection is
    /// fine, and the caller decides what to do instead.
    NotSent(String),
    /// **Sent, answered, and not done.** The server refused -- a `UserError`
    /// key when one was readable -- or the evidence the client waits for
    /// (a session change, a push, a changed listing) never came. Like
    /// [`Error::NotSent`] this is a verdict, not a transport fault: count it
    /// and decide, do not reconnect.
    Refused(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Protocol(why) => write!(f, "protocol: {why}"),
            Error::NotSent(why) => write!(f, "not sent: {why}"),
            Error::Refused(why) => write!(f, "refused: {why}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<eve_proto::FrameError> for Error {
    fn from(e: eve_proto::FrameError) -> Self {
        // A bad frame is the server and us disagreeing about the wire, which
        // is a protocol fault however it arrived.
        Error::Protocol(format!("frame: {e:?}"))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

fn bstr(s: &str) -> PyValue {
    PyValue::Bytes(s.as_bytes().to_vec())
}

/// A wire dict, which is **value-then-key** order. Getting this backwards
/// produces a dict the client reads as empty rather than an error.
fn dict(kv: Vec<(&str, PyValue)>) -> PyValue {
    PyValue::Dict(kv.into_iter().map(|(k, v)| (v, bstr(k))).collect())
}

/// A Python 2 `str` or `unicode` as text.
pub(crate) fn text(v: &PyValue) -> Option<String> {
    match v {
        PyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        PyValue::Unicode(s) => Some(s.clone()),
        _ => None,
    }
}

/// An old-style instance's attribute dict, or the value itself.
pub(crate) fn attributes(v: &PyValue) -> &PyValue {
    match v {
        PyValue::Instance { state, .. } => state.as_ref(),
        other => other,
    }
}

fn dict_get<'a>(v: &'a PyValue, key: &str) -> Option<&'a PyValue> {
    match v {
        PyValue::Dict(entries) => entries
            .iter()
            .find(|(_, k)| *k == bstr(key))
            .map(|(value, _)| value),
        _ => None,
    }
}

/// How many quiet windows a push wait may last at most, however busy the
/// stream (HANG-1): a 250 ms window gives up after 5 s, 2.5 s after 50 s.
const PUSH_WAIT_CAP: u32 = 20;

/// What runs just before a stargate jump ([`Conn::before_jump`]).
pub type JumpHook = Box<dyn FnMut(&mut Conn)>;

/// One connection, framing its own payloads.
pub struct Conn {
    stream: TcpStream,
    buf: Vec<u8>,
    /// The next `callID`. The client matches a reply to a request by this and
    /// nothing else, so it has to be ours to issue and ours to remember.
    next_call_id: i64,
    /// The bound `beyonce` object, once we have bound it. **Every movement
    /// command goes through this**, not through the service name: a bound call
    /// is addressed to the node with no service, and the object id in the
    /// payload is the only routing information in the frame.
    ballpark: Option<String>,
    /// The session as every session change so far has left it.
    session: SessionView,
    /// The park as every `DoDestinyUpdate` so far has left it.
    world: World,
    /// Bound objects held, as the client's `Moniker` holds them:
    /// `(service, bindParams) -> objectID`. Cleared when the session moves
    /// location, because a moniker on `(stationID, groupStation)` stops
    /// naming where the ship is the moment it undocks.
    monikers: Vec<((String, PyValue), ObjectRef)>,
    /// Every push read since [`Self::take_pushes`] last emptied it, when
    /// [`Self::keep_pushes`] has turned it on. See there.
    inbox: Option<Vec<(String, Vec<PyValue>)>>,
    /// Run before every payload is written ([`Self::gate_sends`]); `None`
    /// writes at once.
    gate: Option<Box<dyn FnMut()>>,
    /// Run just before `CmdStargateJump` is sent ([`Self::before_jump`]):
    /// the bot's farewell in the old system's Local.
    pre_jump: Option<JumpHook>,
    /// The pilot's security status when the bot last read it, for the
    /// police net: [`Self::police_status`].
    police_status: Option<f64>,
    /// The process-wide price index, when the driver shares one
    /// ([`Self::share_price_index`]); `None` asks the server every time.
    price_cache: Option<std::sync::Arc<pricing::PriceIndexCache>>,
}

/// A bound object's id, `N=<node>:<serial>`: the only routing information a
/// call on it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRef(pub String);

/// A fused call: `(method, args, kwargs)`, the third element of
/// `MachoBindObject(bindParams, call)`. `kwargs` are `(value, key)`, wire
/// order, as everywhere.
pub type Fused<'a> = (&'a str, Vec<PyValue>, Vec<(PyValue, PyValue)>);

/// What `MachoBindObject` handed back.
#[derive(Debug)]
pub struct Bound {
    /// The object bound.
    pub object: ObjectRef,
    /// Every **other** object reference in the reply, in order. A fused
    /// `invbroker.GetInventory` answers `(invbroker ref, inventory ref)`
    /// (`docs/reference/bound-objects.md` §4, the capture), and the inventory is this.
    pub also: Vec<ObjectRef>,
    /// The fused call's return value; `None` when nothing was fused.
    pub retval: PyValue,
    pub outcome: CallOutcome,
}

/// One call's answer, whichever route it took: a service call, a call on a
/// bound object, or a bind with the call fused onto it.
#[derive(Debug)]
pub struct Answer {
    /// The return value, unpickled -- for a fused bind, the `retval` half.
    /// `None` here means there was no `CallRsp` to read one from: a raise.
    pub value: Option<PyValue>,
    /// Every object reference in `value`, in order.
    pub objects: Vec<ObjectRef>,
    pub outcome: CallOutcome,
}

impl Answer {
    /// The `UserError` key, if the call was refused. See
    /// [`CallOutcome::refusal`].
    pub fn refusal(&self) -> Option<String> {
        self.outcome.refusal()
    }
}

/// One thing read off the socket while nothing was being waited for.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A `Notification`: `(event, args)`, as [`notification`] reads it.
    Push(String, Vec<PyValue>),
    /// A session change nobody asked for in this read -- a fleet seat, a
    /// jump's arrival. Already applied to [`Conn::session`] by the time it
    /// is returned.
    SessionChange(SessionChange),
    /// An answer to a call no longer being waited for, by its callID if it
    /// has one. A bot has one call in the air at a time, so this is news.
    StrayReply(Option<i64>),
}

/// How long [`Conn::pump`] keeps draining once the first frame of a burst
/// has arrived. A burst -- a warp's batch, a dock's three frames -- is
/// written in one flush, so its frames are microseconds apart; 50 ms is
/// generous for that and short beside every timer the bot cares about.
const BURST_GAP: Duration = Duration::from_millis(50);

impl Conn {
    /// Connect, with a read timeout so a silent server is an error rather than
    /// a bot that hangs for ever. A bot is unattended by definition.
    pub fn connect(addr: SocketAddr, read_timeout: Duration) -> Result<Self> {
        let stream = TcpStream::connect(addr)?;
        stream.set_read_timeout(Some(clock::real_timeout(read_timeout)))?;
        Ok(Conn {
            stream,
            buf: Vec::new(),
            next_call_id: 1,
            ballpark: None,
            session: SessionView::default(),
            world: World::default(),
            monikers: Vec::new(),
            inbox: None,
            gate: None,
            pre_jump: None,
            police_status: None,
            price_cache: None,
        })
    }

    /// A second handle on this connection's socket, for whoever must be able
    /// to cut it from outside the thread that owns the `Conn` -- a driver's
    /// supervisor, or a test that pulls the plug mid-mining
    /// (`docs/lanes/bots.md` §7, T7). `shutdown` on it ends this `Conn`'s
    /// next read with an error, as a dropped connection would.
    pub fn socket(&self) -> std::io::Result<TcpStream> {
        self.stream.try_clone()
    }

    /// The session, as the server has stated it.
    pub fn session(&self) -> &SessionView {
        &self.session
    }

    /// The park, as last told -- **not** carried forward. See
    /// [`Self::world_now`].
    pub fn world(&self) -> &World {
        &self.world
    }

    /// The park carried forward to now with the server's own motion law
    /// (`park.rs`). What a decision should read.
    pub fn world_now(&mut self) -> &World {
        self.world
            .advance_to(eve_server::space_state::now_seconds());
        &self.world
    }

    /// The bound ballpark's object id, if one is bound.
    pub fn ballpark(&self) -> Option<ObjectRef> {
        self.ballpark.clone().map(ObjectRef)
    }

    /// **Every frame read anywhere comes through here**, so this is where the
    /// two kinds of state a frame can carry are applied: a session change to
    /// [`Self::session`], and a `DoDestinyUpdate` to [`Self::world`].
    ///
    /// Done at the choke point rather than by each reader because the old
    /// readers threw session changes away -- `collect_pushes` dropped them,
    /// and a test's own `recv` loop never looked -- and an unsolicited one
    /// (the fleet seat, the jump timer) was simply lost.
    fn observe(&mut self, frame: &PyValue) {
        if let Some(change) = SessionChange::from_frame(frame) {
            self.session.apply(&change);
            if change.releases_park() {
                // `michelle.Park.Release`: the park and its bound object go
                // together. `bind_ballpark` is then correct after a dock or
                // a jump without `rebind_ballpark`.
                self.ballpark = None;
                self.world.reset();
            }
            if change.moves_location() {
                self.monikers.clear();
            }
            return;
        }
        if let Some((event, args)) = notification(frame) {
            if event == "DoDestinyUpdate" {
                stats::DESTINY.time(|| {
                    self.world
                        .apply_update(&args, eve_server::space_state::now_seconds())
                });
            }
            if let Some(inbox) = &mut self.inbox {
                inbox.push((event, args));
            }
        }
    }

    /// **Keep a copy of every push, wherever it was read.**
    ///
    /// A verb waits for its own evidence and keeps the pushes it read on
    /// the way in its [`CallOutcome`] -- or, for a verb that returns
    /// something else (`mine` returns the effect), drops them. A driver that
    /// learnt about the world only from [`Self::pump`] would then miss
    /// whatever landed during a call: the player's `OnFleetInvite` arriving
    /// while the bot was locking a rock. Turned on, every push read by
    /// [`Self::recv`] -- the choke point -- is also queued here, in order,
    /// until [`Self::take_pushes`]. Off by default, so a test that never
    /// drains it does not grow it.
    pub fn keep_pushes(&mut self) {
        self.inbox.get_or_insert_with(Vec::new);
    }

    /// Every push queued since the last call, oldest first. Empty unless
    /// [`Self::keep_pushes`] is on.
    pub fn take_pushes(&mut self) -> Vec<(String, Vec<PyValue>)> {
        self.inbox.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// **Every payload passes `gate` first**: the driver's pacing (CP39,
    /// `driver::Pacer`). It may sleep; the payload is written when it
    /// returns.
    pub fn gate_sends(&mut self, gate: impl FnMut() + 'static) {
        self.gate = Some(Box::new(gate));
    }

    /// **Run `hook` just before a stargate jump is sent** (CHATTER-2: the
    /// farewell, which must reach the old system's Local before the bot is
    /// gone). `None` clears it. The hook gets the connection; it runs only
    /// once the jump's own checks passed.
    pub fn before_jump(&mut self, hook: Option<JumpHook>) {
        self.pre_jump = hook;
    }

    /// **The status gate travel avoids the police net by** (BOT-LAW,
    /// [`travel::Map::in_police_net`]): `Some(status)` keeps every route
    /// out of the systems where faction police attack that status; `None`
    /// (never read, or a clean pilot) routes by the floor alone.
    pub fn police_status(&self) -> Option<f64> {
        self.police_status
    }

    /// Set [`Self::police_status`].
    pub fn set_police_status(&mut self, status: Option<f64>) {
        self.police_status = status;
    }

    /// Call the [`Self::before_jump`] hook, if one is set.
    pub(crate) fn run_pre_jump(&mut self) {
        if let Some(mut hook) = self.pre_jump.take() {
            hook(self);
            // A hook that installed another one keeps it.
            if self.pre_jump.is_none() {
                self.pre_jump = Some(hook);
            }
        }
    }

    pub fn send(&mut self, v: &PyValue) -> Result<()> {
        if let Some(gate) = self.gate.as_mut() {
            gate();
        }
        let mut framed = Vec::new();
        write_frame(&dumps(v), &mut framed)?;
        self.stream.write_all(&framed)?;
        Ok(())
    }

    /// Answer `frame` if it is a `PingReq`: the same `times` list back in a
    /// `PingRsp` with the addresses swapped. Written straight to the stream,
    /// past the send gate, which is a test hook for the bot's own calls.
    fn answer_ping(&mut self, frame: &PyValue) -> Result<bool> {
        let PyValue::Instance { class, .. } = frame else {
            return Ok(false);
        };
        if !class.ends_with(b".PingReq") {
            return Ok(false);
        }
        let Ok(packet) = eve_proto::MachoPacket::from_value(frame) else {
            return Ok(false);
        };
        let rsp = eve_proto::MachoPacket::respond_to(
            &packet,
            eve_marshal::MachoNetMsg::PingRsp,
            vec![packet.body.first().cloned().unwrap_or(PyValue::None)],
        );
        let value = rsp
            .to_value()
            .map_err(|e| Error::Protocol(format!("cannot build a PingRsp: {e:?}")))?;
        let mut framed = Vec::new();
        write_frame(&dumps(&value), &mut framed)?;
        self.stream.write_all(&framed)?;
        Ok(true)
    }

    pub fn recv(&mut self) -> Result<PyValue> {
        loop {
            if let Some(f) = self.recv_frame(false)? {
                return Ok(f);
            }
        }
    }

    /// One frame, or `None` when the frame was a `PingReq` that was answered
    /// and `ping_ends` asks the caller to re-arm its own timeout.
    fn recv_frame(&mut self, ping_ends: bool) -> Result<Option<PyValue>> {
        let mut chunk = [0u8; 4096];
        loop {
            if let Some((frame, consumed)) = try_split_frame(&self.buf)? {
                let payload = frame.payload.to_vec();
                self.buf.drain(..consumed);
                stats::FRAME_BYTES
                    .fetch_add(payload.len() as u64, std::sync::atomic::Ordering::Relaxed);
                let frame = stats::DECODE
                    .time(|| loads(&payload))
                    .map_err(|e| Error::Protocol(format!("payload does not parse: {e}")))?;
                // **The server's keepalive** (NET-PING): after 30 s of silence
                // it sends a `PingReq` and drops the connection if nothing
                // comes back. Answered here, like `eve-probe`, so no caller
                // ever sees one and a bot blocked in a call still answers.
                if self.answer_ping(&frame)? {
                    if ping_ends {
                        return Ok(None);
                    }
                    continue;
                }
                stats::OBSERVE.time(|| self.observe(&frame));
                return Ok(Some(frame));
            }
            let n = self.stream.read(&mut chunk)?;
            if n == 0 {
                return Err(Error::Protocol(
                    "the server closed while a payload was expected".to_owned(),
                ));
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    /// Make a service call and return its result, plus any notification that
    /// arrived while we waited.
    ///
    /// **Reading the next frame and calling it the answer is a race.** A push
    /// posted for this connection by *another* connection's turn is delivered
    /// by the world ticker, so it can land between our request and its reply.
    /// The server's own loopback tests learned this the moment a second player
    /// existed: tests that assumed the next frame was the response passed only
    /// while nothing was ever pushed to a session that had not acted. A bot is
    /// the second player by construction, so it must never assume that.
    ///
    /// Notifications are returned rather than dropped, because for a bot they
    /// are the interesting half: `DoDestinyUpdate` is how it learns where
    /// anything is.
    ///
    /// A `SessionChangeNotification` is *not* a push -- it is the other half
    /// of the call that caused it -- so it is reported separately in
    /// [`CallOutcome::session_changed`].
    pub fn call(&mut self, service: &str, method: &str, args: Vec<PyValue>) -> Result<CallOutcome> {
        self.call_kw(service, method, args, Vec::new())
    }

    /// A service call with keyword arguments, `(value, key)` in wire order
    /// (see [`Self::bound_call_kw`] for why that order).
    pub fn call_kw(
        &mut self,
        service: &str,
        method: &str,
        args: Vec<PyValue>,
        kwargs: Vec<(PyValue, PyValue)>,
    ) -> Result<CallOutcome> {
        let call_id = self.next_call_id;
        self.next_call_id += 1;
        let payload = eve_proto::CallPayload::Service {
            method: method.to_owned(),
            args,
            kwargs,
        };
        let packet = eve_proto::MachoPacket::new(
            eve_marshal::MachoNetMsg::CallReq,
            eve_proto::MachoAddress::Client {
                client_id: 1,
                call_id: Some(call_id),
                service: None,
            },
            eve_proto::MachoAddress::Any {
                service: Some(service.to_owned()),
                call_id: Some(call_id),
            },
            vec![payload.to_payload()],
        );
        let value = packet
            .to_value()
            .map_err(|e| Error::Protocol(format!("cannot build a CallReq: {e:?}")))?;
        self.send(&value)?;

        self.read_reply(&format!("{service}.{method}"), call_id)
    }

    /// The reply-reading half of a call, shared by service and bound calls.
    ///
    /// A bounded wait: a bot that reads for ever on a server answering only
    /// with pushes is a hung bot, which is the thing we refuse to be.
    fn read_reply(&mut self, what: &str, call_id: i64) -> Result<CallOutcome> {
        let mut outcome = CallOutcome {
            result: PyValue::None,
            notifications: Vec::new(),
            session_changed: false,
            session_change: None,
        };
        let deadline = clock::instant() + REPLY_WAIT;
        while clock::instant() < deadline {
            let frame = self.recv()?;
            let PyValue::Instance { class, .. } = &frame else {
                return Err(Error::Protocol(format!(
                    "expected a packet instance, got {frame:?}"
                )));
            };
            let name = String::from_utf8_lossy(class).into_owned();
            // **The final segment, compared exactly.** `ends_with`
            // ("Notification") is also true of `SessionChangeNotification`,
            // and conflating the two is a documented mistake in this protocol:
            // the undock is a call whose reply arrives beside a session
            // change.
            match name.rsplit('.').next() {
                Some("Notification") => {
                    outcome.notifications.push(frame);
                    continue;
                }
                Some("SessionChangeNotification") => {
                    outcome.session_changed = true;
                    if let Some(change) = SessionChange::from_frame(&frame) {
                        outcome.absorb(change);
                    }
                    continue;
                }
                _ => {
                    // **Match on the callID, which is the only thing that
                    // identifies a reply.** Taking the first non-notification
                    // frame is wrong and was: `ship.Undock` leaves a reply in
                    // the stream that the *next* call read as its own, so a
                    // bind came back as the undock's `None` and the ballpark
                    // was never bound -- reported as "nothing is bound yet"
                    // against a server that had answered correctly. The
                    // comment on `next_call_id` said replies are matched by
                    // callID and nothing else; this is that, finally done.
                    let theirs = eve_proto::MachoPacket::from_value(&frame)
                        .ok()
                        .and_then(|p| p.destination.call_id());
                    if theirs.is_some_and(|id| id != call_id) {
                        // Somebody else's answer, still in flight. Drop it:
                        // keeping it would mean a queue, and a bot that needs
                        // a queue of replies has more than one call in the
                        // air, which it does not.
                        continue;
                    }
                    outcome.result = frame;
                    return Ok(outcome);
                }
            }
        }
        Err(Error::Protocol(format!(
            "{what}: no reply within {REPLY_WAIT:?} \
             ({} notification(s) seen, session_changed={})",
            outcome.notifications.len(),
            outcome.session_changed
        )))
    }

    /// Become a character.
    ///
    /// `charUnboundMgr.SelectCharacterID` is **not an ordinary service call**:
    /// its effect is a session change and the client discards the return value
    /// (`dispatch.rs` answers it specially for exactly that reason). So the
    /// thing to assert is the session change, not the result.
    ///
    /// The id has to be supplied here. To be told by name instead, as the
    /// select screen is, see [`Self::select_character_named`] -- an earlier
    /// version of this comment said the server answers no "list the
    /// characters" call, and `GetCharacterSelectionData` had been answered
    /// all along (`docs/lanes/bots.md` §1).
    pub fn select_character(&mut self, character_id: i64) -> Result<CallOutcome> {
        let mut outcome = self.call(
            "charUnboundMgr",
            "SelectCharacterID",
            vec![PyValue::Int(character_id)],
        )?;
        // **The reply comes first; the session change follows.** Measured, not
        // assumed: the first version of this asserted `session_changed`
        // straight off the call and failed against a server whose own log said
        // `session change: character 90000001 entering the game`. The effect
        // had happened and the notification was simply still in flight. So the
        // wait is a separate step, and a short one -- a failed selection sends
        // no notification at all, and we must not pay the full read timeout to
        // find that out.
        self.await_session_change(&mut outcome, Duration::from_millis(1500))?;
        if !outcome.session_changed {
            return Err(Error::Protocol(format!(
                "SelectCharacterID({character_id}) answered without a session change -- \
                 the real client would stay on the character screen"
            )));
        }
        Ok(outcome)
    }

    /// The select screen's own list: `charUnboundMgr.GetCharacterSelectionData`,
    /// a 4-tuple whose third element is one `KeyVal` per character
    /// (`service.rs::character_selection_data`, the shapes from
    /// `charselData.CharacterSelectionData.FetchCharacterInfoForUserID`)
    /// **[V]**.
    pub fn characters(&mut self) -> Result<Vec<CharacterSlot>> {
        let outcome = self.call("charUnboundMgr", "GetCharacterSelectionData", vec![])?;
        let value = reply_value(&outcome.result).ok_or_else(|| {
            Error::Protocol("GetCharacterSelectionData answered with no value".to_owned())
        })?;
        let rows = match &value {
            PyValue::Tuple(parts) => match parts.get(2) {
                Some(PyValue::List(rows)) => rows,
                _ => {
                    return Err(Error::Protocol(format!(
                        "GetCharacterSelectionData: no character list in {value:?}"
                    )));
                }
            },
            other => {
                return Err(Error::Protocol(format!(
                    "GetCharacterSelectionData is not a tuple: {other:?}"
                )));
            }
        };
        Ok(rows.iter().filter_map(CharacterSlot::from_row).collect())
    }

    /// **Become a character by name**, as a player picks one off the select
    /// screen: list, find, select. Never by index or id constant -- ids
    /// derive from the roster's order (`docs/lanes/bots.md` §5), and a bot
    /// that selects `90000001 + i` is somebody else the day the roster is
    /// reordered.
    pub fn select_character_named(&mut self, name: &str) -> Result<CallOutcome> {
        let slots = self.characters()?;
        let Some(slot) = slots.iter().find(|s| s.name == name) else {
            let offered: Vec<&str> = slots.iter().map(|s| s.name.as_str()).collect();
            return Err(Error::Protocol(format!(
                "no character named {name:?} on the select screen; offered {offered:?}"
            )));
        };
        let id = slot.character_id;
        self.select_character(id)
    }

    /// Leave the station.
    ///
    /// `ship.Undock` is the call the real client's `UndockAttempt` ends in
    /// (`station/base.py` -> `sessionMgr.PerformSessionChange('undock',
    /// shipsvc.Undock, shipID)`), and like `SelectCharacterID` its evidence is
    /// the session change rather than the return value.
    ///
    /// **On the station's `ship` moniker, as the client sends it** -- this was
    /// a plain `ship.Undock()` service call until P8a, which the server
    /// answers identically and no client ever sends. The capture is
    /// `ship.MachoBindObject((60012526, 15), ("Undock", (shipID, False),
    /// {"onlineModules": {}}))` (`docs/lanes/round2-spec.md` row 1, frame
    /// `[595]`) **[V]**: `(shipID, ignoreContraband)` and the keyword
    /// `onlineModules = {flagID: moduleID}` (`fitting.md` §3.4) **[V]**. See
    /// [`Self::undock_with`] for a ship with modules online.
    pub fn undock(&mut self) -> Result<CallOutcome> {
        self.undock_with(&[])
    }

    /// [`Self::undock`], naming the modules the client would list as online:
    /// `(flagID, moduleID)` pairs. **[A]** that the bot's list matches what
    /// the client's would; the server ignores the keyword today
    /// (`fitting.md` §3.4).
    pub fn undock_with(&mut self, online_modules: &[(i64, i64)]) -> Result<CallOutcome> {
        let (Some(station), Some(ship)) = (self.session.station_id(), self.session.ship_id())
        else {
            return Err(Error::NotSent(
                "undock: the session names no station and ship -- not docked".to_owned(),
            ));
        };
        let online = PyValue::Dict(
            online_modules
                .iter()
                .map(|(flag, module)| (PyValue::Int(*module), PyValue::Int(*flag)))
                .collect(),
        );
        let mut outcome = self
            .moniker_call(
                "ship",
                station_moniker(station),
                "Undock",
                vec![PyValue::Int(ship), PyValue::Bool(false)],
                vec![(online, bstr("onlineModules"))],
            )?
            .outcome;
        self.await_session_change(&mut outcome, Duration::from_millis(2500))?;
        if !outcome.session_changed {
            return Err(Error::Protocol(
                "ship.Undock answered without a session change -- still docked".to_owned(),
            ));
        }
        Ok(outcome)
    }

    /// Bind the solar system's ballpark, which is what makes the world arrive.
    ///
    /// **This bind is the trigger, not the undock.** `michelle.DoDestinyUpdate`
    /// returns immediately while the client has no ballpark, and the real
    /// client builds one about three seconds after the undock session change;
    /// so the server holds the bootstrap state until
    /// `beyonce.MachoBindObject(solarSystemID, None)` arrives
    /// (`destiny::on_ballpark_bind`). A bot that undocks and waits will wait
    /// for ever -- it has to ask.
    ///
    /// The server queues the state to drain **after** this response in the
    /// same flush, so the `DoDestinyUpdate` lands just behind the reply rather
    /// than inside it. Hence the short collect afterwards.
    pub fn bind_ballpark(&mut self, solar_system_id: i64) -> Result<CallOutcome> {
        // **Binding twice teleports you back to the station mouth, so refuse
        // to do it by accident.** `destiny::on_ballpark_bind` rewrites the
        // roster entry from `undock_motion` on *every* bind, deliberately:
        // "a bind means a ship has just arrived in space, and the mouth is
        // where it starts -- not wherever the last flight ended", because
        // `michelle.Park.Release` destroys the park on docking so the real
        // client binds once per undock.
        //
        // A bot has no such limit. Measured: `/tp 1000` then a second
        // `enter_space` put the ship back 17 m from the mouth, and the 1000 km
        // was gone from the roster -- silently, because a bind that resets you
        // looks exactly like a bind that worked. The premise is sound for the
        // client and is an assumption for anything else, so the escape hatch
        // is named rather than default: [`Conn::rebind_ballpark`].
        if self.ballpark.is_some() {
            return Err(Error::Protocol(
                "already bound -- binding again resets this ship to the station \
                 mouth server-side (destiny::on_ballpark_bind). Use \
                 rebind_ballpark if that is really what you want"
                    .to_owned(),
            ));
        }
        self.rebind_ballpark(solar_system_id)
    }

    /// Bind again, knowing it resets the ship to the station mouth.
    ///
    /// Only for a caller that wants exactly that -- a test of the reset, or a
    /// bot simulating a dock-and-undock. Everything else wants
    /// [`Conn::bind_ballpark`].
    pub fn rebind_ballpark(&mut self, solar_system_id: i64) -> Result<CallOutcome> {
        // **Keep the object id: every movement command needs it.** A bind
        // answers `(boundObject, retval)`, and the bound object is a callback
        // reference whose child is a whole second marshal stream holding
        // `(objectID, refID)` -- which is what `ObjectCall.ParseObjectID`
        // loads on the client side.
        let bound = self.bind("beyonce", PyValue::Int(solar_system_id), None)?;
        self.ballpark = Some(bound.object.0);
        let mut outcome = bound.outcome;
        // Stop at the first readable SetState: on a busy grid (another
        // pilot's guns cycling) frames arrive faster than the idle window,
        // and waiting for quiet would wait for the fight to end (HANG-1).
        let more = self.collect_pushes_until(Duration::from_millis(2500), |frame| {
            notification(frame).is_some_and(|(event, args)| {
                event == "DoDestinyUpdate" && read_set_state(&args).is_some()
            })
        })?;
        outcome.notifications.extend(more);
        // Then the grid's follow-ups (a hole's slim, a site's balls), which
        // come right after the state: a short quiet window, itself capped at
        // PUSH_WAIT_CAP windows, so a busy grid still cannot hold it long.
        let tail = self.collect_pushes(Duration::from_millis(500))?;
        outcome.notifications.extend(tail);
        Ok(outcome)
    }

    /// `service.MachoBindObject(bindParams, call)`: bind an object, with the
    /// call that triggered the bind fused onto it or `None`.
    ///
    /// **Bind and first call are one round trip** (`dispatch.rs::handle_bind`,
    /// `Moniker.Bind`): the reply is `(boundObject, retval)`, and `retval`
    /// can itself be an object -- a fused `GetInventory` answers two
    /// references. So every reference is returned, in order, and not only
    /// the first: [`Bound::object`] and then [`Bound::also`].
    ///
    /// A raise inside the fused call comes back as an `ErrorResponse` with no
    /// object at all, which is an error here; see [`CallOutcome::refusal`]
    /// for reading its key.
    pub fn bind(
        &mut self,
        service: &str,
        params: PyValue,
        fused: Option<Fused<'_>>,
    ) -> Result<Bound> {
        let call = match fused {
            None => PyValue::None,
            Some((method, args, kwargs)) => PyValue::Tuple(vec![
                bstr(method),
                PyValue::Tuple(args),
                PyValue::Dict(kwargs),
            ]),
        };
        let outcome = self.call(service, "MachoBindObject", vec![params, call])?;
        // A raise in the fused call is the whole bind's answer: no object.
        // That is a refusal, not a broken connection.
        if let Some(key) = outcome.refusal() {
            return Err(Error::Refused(format!("{service}.MachoBindObject: {key}")));
        }
        let reply = reply_value(&outcome.result);
        let (object, retval) = match &reply {
            Some(PyValue::Tuple(pair)) if pair.len() == 2 => (pair[0].clone(), pair[1].clone()),
            _ => {
                return Err(Error::Protocol(format!(
                    "{service}.MachoBindObject: expected (boundObject, retval), got {reply:?} \
                     (refusal: {:?})",
                    outcome.refusal()
                )));
            }
        };
        let Some(object) = object_refs(&object).into_iter().next() else {
            return Err(Error::Protocol(format!(
                "{service}.MachoBindObject: no object reference in {object:?}"
            )));
        };
        Ok(Bound {
            object,
            also: object_refs(&retval),
            retval,
            outcome,
        })
    }

    /// **A call on a moniker, the way the client's `Moniker` makes one**:
    /// the first call binds with the call fused onto the bind; every later
    /// call goes to the object that bind handed back. `(service,
    /// bindParams)` is the moniker's identity.
    ///
    /// The held objects are dropped when the session moves location
    /// ([`SessionChange::moves_location`]). The client also re-binds on its
    /// own schedule (it releases objects after about thirty seconds,
    /// `service/inventory.rs`'s `Undock` arm); the server answers both
    /// routes the same, so the bot keeping an object longer asks nothing the
    /// client does not.
    pub fn moniker_call(
        &mut self,
        service: &str,
        params: PyValue,
        method: &str,
        args: Vec<PyValue>,
        kwargs: Vec<(PyValue, PyValue)>,
    ) -> Result<Answer> {
        let held = self
            .monikers
            .iter()
            .find(|((s, p), _)| s == service && *p == params)
            .map(|(_, o)| o.clone());
        if let Some(object) = held {
            let outcome = self.call_on(&object, method, args, kwargs)?;
            let value = reply_value(&outcome.result);
            return Ok(Answer {
                objects: value.as_ref().map(object_refs).unwrap_or_default(),
                value,
                outcome,
            });
        }
        let outcome_or_bound = self.bind(service, params.clone(), Some((method, args, kwargs)));
        match outcome_or_bound {
            Ok(bound) => {
                self.monikers
                    .push(((service.to_owned(), params), bound.object.clone()));
                Ok(Answer {
                    value: Some(bound.retval),
                    objects: bound.also,
                    outcome: bound.outcome,
                })
            }
            Err(e) => Err(e),
        }
    }

    /// Call a method on a bound object.
    ///
    /// **Addressed to the node with no service name**: the object id in the
    /// payload is the only routing information in the frame
    /// (`dispatch.rs::handle_bound_call`).
    pub fn call_on(
        &mut self,
        object: &ObjectRef,
        method: &str,
        args: Vec<PyValue>,
        kwargs: Vec<(PyValue, PyValue)>,
    ) -> Result<CallOutcome> {
        let call_id = self.next_call_id;
        self.next_call_id += 1;
        let payload = eve_proto::CallPayload::BoundObject {
            object_id: object.0.clone(),
            method: method.to_owned(),
            args,
            kwargs,
        };
        let packet = eve_proto::MachoPacket::new(
            eve_marshal::MachoNetMsg::CallReq,
            eve_proto::MachoAddress::Client {
                client_id: 1,
                call_id: Some(call_id),
                service: None,
            },
            eve_proto::MachoAddress::Node {
                node_id: Some(1),
                service: None,
                call_id: None,
            },
            vec![payload.to_payload()],
        );
        let value = packet
            .to_value()
            .map_err(|e| Error::Protocol(format!("cannot build a bound CallReq: {e:?}")))?;
        self.send(&value)?;
        self.read_reply(method, call_id)
    }

    /// Bind the ballpark and decode the state that follows.
    ///
    /// The whole of milestone 2 in one call, for a caller that wants the world
    /// rather than the frames it arrived in.
    pub fn enter_space(&mut self, solar_system_id: i64) -> Result<WorldState> {
        let outcome = self.bind_ballpark(solar_system_id)?;
        let mut seen = Vec::new();
        for frame in &outcome.notifications {
            let Some((event, args)) = notification(frame) else {
                continue;
            };
            if event != "DoDestinyUpdate" {
                seen.push(event);
                continue;
            }
            if let Some(world) = read_set_state(&args) {
                return Ok(world);
            }
            seen.push(format!("{event} (not a SetState we could read)"));
        }
        Err(Error::Protocol(format!(
            "no readable SetState after binding {solar_system_id}; saw {seen:?}"
        )))
    }

    /// Call a method on the bound ballpark.
    ///
    /// **Addressed to the node with no service name.** A bound call carries
    /// the object id in its payload and that is the only routing information
    /// in the frame — which is why `bind_ballpark` has to be called first and
    /// why its reply is worth keeping.
    pub fn bound_call(&mut self, method: &str, args: Vec<PyValue>) -> Result<CallOutcome> {
        self.bound_call_kw(method, args, Vec::new())
    }

    /// A bound call with keyword arguments, in the **wire order a real client
    /// produces**.
    ///
    /// `kwargs` entries are `(value, key)`, not `(key, value)`, because that
    /// is what the format is: the dict handler reads the value first and the
    /// key second, confirmed from `PyDict_SetItem(dict, key=second,
    /// value=first)` (`eve_marshal::PyValue::Dict`). A bot that sent them the
    /// other way round would be exercising a client that does not exist,
    /// which is exactly the mistake this is here to avoid.
    pub fn bound_call_kw(
        &mut self,
        method: &str,
        args: Vec<PyValue>,
        kwargs: Vec<(PyValue, PyValue)>,
    ) -> Result<CallOutcome> {
        let Some(object_id) = self.ballpark.clone() else {
            return Err(Error::Protocol(format!(
                "{method} is a bound call and nothing is bound yet -- \
                 bind_ballpark or enter_space first"
            )));
        };
        self.call_on(&ObjectRef(object_id), method, args, kwargs)
    }

    /// Open the throttle. `fraction` is 0.0 to 1.0.
    ///
    /// **1.0 is the only value that reaches warp.** The sub-warp goto
    /// integrator multiplies `maxVelocity` by this, the align gate needs
    /// strictly more than 0.75 of top speed, and a goto only approaches its
    /// commanded speed from below -- so anything less converges short of the
    /// gate and the ship aligns until the 180-tick watchdog gives up
    /// (`docs/port-tracker.md` §34).
    pub fn set_speed_fraction(&mut self, fraction: f64) -> Result<CallOutcome> {
        self.bound_call("CmdSetSpeedFraction", vec![PyValue::Real(fraction)])
    }

    /// Lock a target.
    ///
    /// **The server never times a lock, and that is the interesting part.**
    /// The acquisition delay a player waits through is computed entirely
    /// client-side -- `min(180000, 40e6 / scanResolution / asinh(sigRadius)^2)`
    /// ms at `bracketMgr.py:1305` **[V]** -- from attributes we serve. So a bot
    /// can prove the lock is accepted and listed, and it **cannot** measure
    /// the delay: there is nothing on this side to measure. `AddTarget`
    /// answers `(0, [])` and the falsy flag is what makes the client draw the
    /// bracket.
    ///
    /// Not a bound call: targeting is `dogmaIM`, a service.
    pub fn lock(&mut self, target: i64) -> Result<CallOutcome> {
        self.call("dogmaIM", "AddTarget", vec![PyValue::Int(target)])
    }

    /// The ships that currently have this character's ship locked
    /// (`dogmaIM.GetTargeters`, which the client reads for the same list).
    pub fn targeters(&mut self) -> Result<CallOutcome> {
        self.call("dogmaIM", "GetTargeters", vec![])
    }

    /// Everything this character currently has locked.
    pub fn targets(&mut self) -> Result<CallOutcome> {
        self.call("dogmaIM", "GetTargets", vec![])
    }

    /// `CmdAlignTo(dstID=item, bookmarkID=None)`, which is CCP's own call.
    ///
    /// **Both keywords, always, because that is what the client sends**:
    /// `menusvc.py:2760` is `bp.CmdAlignTo(dstID=targetID,
    /// bookmarkID=bookmarkID)` with exactly one of the two set and the other
    /// `None` `[V]`. Sending only `dstID` would work against our server and
    /// would be a bot the client is not.
    ///
    /// The keywords travel in wire order; see [`Self::bound_call_kw`].
    pub fn align_to(&mut self, item_id: i64) -> Result<CallOutcome> {
        self.bound_call_kw(
            "CmdAlignTo",
            Vec::new(),
            vec![
                (PyValue::Int(item_id), bstr("dstID")),
                (PyValue::None, bstr("bookmarkID")),
            ],
        )
    }

    /// Orbit a ball at a range, in metres.
    ///
    /// `(targetID, range)`, which is what `space_state`'s command table reads.
    pub fn orbit(&mut self, target: i64, range: f64) -> Result<CallOutcome> {
        self.bound_call("CmdOrbit", vec![PyValue::Int(target), PyValue::Real(range)])
    }

    /// Follow a ball at a range, in metres. The "keep at range" command.
    pub fn follow(&mut self, target: i64, range: f64) -> Result<CallOutcome> {
        self.bound_call(
            "CmdFollowBall",
            vec![PyValue::Int(target), PyValue::Real(range)],
        )
    }

    /// Point the ship down a unit vector without warping. The align command.
    pub fn goto_direction(&mut self, direction: [f64; 3]) -> Result<CallOutcome> {
        self.bound_call(
            "CmdGotoDirection",
            vec![
                PyValue::Real(direction[0]),
                PyValue::Real(direction[1]),
                PyValue::Real(direction[2]),
            ],
        )
    }

    /// Warp to an item, the way the overview's menu does.
    ///
    /// `min_range` of 0.0 lets the server pick the standoff, which is what the
    /// client sends when a player clicks "Warp to" without a range.
    /// `CmdWarpToStuff('item', itemID, minRange=...)`.
    ///
    /// **`minRange` is a keyword, and this sent it as a third positional for
    /// as long as the function existed.** The client's call is two positionals
    /// plus keyword `minRange`/`fleet` `[V]`, and the server reads
    /// `req.kwargs`, so the third positional was dropped on the floor: every
    /// warp a bot ever made used the default range whatever it asked for. It
    /// went unnoticed because every caller asked for `0.0`, which is what the
    /// default produces -- the one value that cannot tell the two apart.
    /// Found while auditing the kwargs path after §39.
    ///
    /// **Never below the minimum warp distance** (WARP-MIN): the server
    /// refuses a warp that lands within 150 km, so when the park says this
    /// one would, the ship approaches (`CmdFollowBall` to the same range)
    /// and nothing is warped. Every caller that warps "when far" gets the
    /// approach for free at the margin its own gap test cannot see.
    pub fn warp_to_item(&mut self, item_id: i64, min_range: f64) -> Result<CallOutcome> {
        let w = self.world_now();
        let gap = w.ego.and_then(|e| w.surface_distance(e, item_id));
        if gap.is_some_and(|g| {
            crate::escort::warp_lands_short(
                g,
                min_range.max(eve_server::space_state::WARP_STANDOFF),
            )
        }) {
            return self.follow(item_id, min_range.max(crate::escort::WRECK_APPROACH));
        }
        let outcome = self.bound_call_kw(
            "CmdWarpToStuff",
            vec![PyValue::Bytes(b"item".to_vec()), PyValue::Int(item_id)],
            vec![(PyValue::Real(min_range), bstr("minRange"))],
        )?;
        // The server's own minimum-distance refusal, for a gap this park
        // could not see: approach instead.
        if outcome.refusal().as_deref() == Some("CustomNotify") {
            return self.follow(item_id, min_range.max(crate::escort::WRECK_APPROACH));
        }
        Ok(outcome)
    }

    /// [`Self::recv`] with a read timeout of `d` (game time: the socket wait
    /// is `clock::real_timeout(d)`), then the caller's timeout put back
    /// whatever happened, or every later call would inherit this one's much
    /// shorter patience. A timeout comes back as `Error::Io` ([`is_timeout`]).
    pub fn recv_within(&mut self, d: Duration) -> Result<PyValue> {
        let restore = self.stream.read_timeout()?;
        let deadline = clock::instant() + d;
        let frame = loop {
            let left = deadline.saturating_duration_since(clock::instant());
            if left.is_zero() {
                break Err(Error::Io(std::io::ErrorKind::TimedOut.into()));
            }
            // A keepalive ping is answered and the read re-armed with what is
            // left, never with the whole `d` again (a ping every 30 s held a
            // 31 s wait for ever).
            if let Err(e) = self.stream.set_read_timeout(Some(
                clock::real_timeout(left).max(Duration::from_millis(1)),
            )) {
                break Err(e.into());
            }
            match self.recv_frame(true) {
                Ok(Some(f)) => break Ok(f),
                Ok(None) => continue,
                Err(e) => break Err(e),
            }
        };
        self.stream.set_read_timeout(restore)?;
        frame
    }

    /// **Read whatever arrives, losing nothing**: block up to `within` for
    /// the first frame, then drain the rest of its burst, and return every
    /// frame as an [`Event`].
    ///
    /// The driver's clock: a bot sleeps on its socket, not on a timer, so it
    /// cannot sleep through a push. Unlike [`Self::collect_pushes`] -- which
    /// keeps notifications only and waits for a whole quiet `within` -- this
    /// returns as soon as a burst has landed, and reports session changes
    /// and stray replies too. Session changes are already applied to
    /// [`Self::session`], and `DoDestinyUpdate`s to [`Self::world`], by the
    /// time they are returned.
    ///
    /// An empty `Vec` is a quiet `within`, not an error.
    pub fn pump(&mut self, within: Duration) -> Result<Vec<Event>> {
        let started = clock::instant();
        let mut events = Vec::new();
        let mut wait = within.max(Duration::from_millis(1));
        let verdict = loop {
            match self.recv_within(wait) {
                Ok(frame) => {
                    events.push(event_of(&frame));
                    // The burst: whatever follows within `BURST_GAP`, bounded
                    // by `within` overall so a chatty system cannot hold the
                    // bot here for ever.
                    if clock::since(started) >= within {
                        break Ok(());
                    }
                    wait = BURST_GAP;
                }
                Err(Error::Io(e)) if is_timeout(&e) => break Ok(()),
                Err(e) => break Err(e),
            }
        };
        verdict.map(|()| events)
    }

    /// Read notification frames until `within` elapses with nothing new.
    ///
    /// The timeout is the terminating condition, not a failure: pushes are
    /// asynchronous and there is no count to expect.
    ///
    /// Keeps notifications only. A session change read here is not lost --
    /// [`Self::recv`] has applied it to [`Self::session`] -- but it is not
    /// returned either; [`Self::pump`] is the read that returns everything.
    pub fn collect_pushes(&mut self, within: Duration) -> Result<Vec<PyValue>> {
        self.collect_pushes_until(within, |_| false)
    }

    /// [`Self::collect_pushes`], also stopping at the first notification
    /// `done` accepts. The idle timeout restarts with every frame, so a
    /// busy connection never goes quiet; `done` is the bound for a caller
    /// that knows which push it is waiting for.
    pub fn collect_pushes_until(
        &mut self,
        within: Duration,
        done: impl Fn(&PyValue) -> bool,
    ) -> Result<Vec<PyValue>> {
        let mut seen = Vec::new();
        // A hard cap besides the quiet window: on a busy grid frames never
        // stop, so a wait for quiet alone could last the whole fight (HANG-1).
        let cap = clock::instant() + within * PUSH_WAIT_CAP;
        loop {
            if clock::instant() >= cap {
                break Ok(seen);
            }
            match self.recv_within(within) {
                Ok(frame) => {
                    if notification(&frame).is_some() {
                        let stop = done(&frame);
                        seen.push(frame);
                        if stop {
                            break Ok(seen);
                        }
                    }
                }
                Err(Error::Io(e)) if is_timeout(&e) => {
                    break Ok(seen);
                }
                Err(e) => break Err(e),
            }
        }
    }

    /// If `outcome` carries no session change yet, wait up to `within` for
    /// one and fold it in. "None came" is left for the caller to judge.
    pub(crate) fn await_session_change(
        &mut self,
        outcome: &mut CallOutcome,
        within: Duration,
    ) -> Result<()> {
        if outcome.session_changed {
            return Ok(());
        }
        if let Some(change) = self.wait_for_session_change(within, &mut outcome.notifications)? {
            outcome.session_changed = true;
            outcome.absorb(change);
        }
        Ok(())
    }

    /// Read frames until a `SessionChangeNotification` arrives or `within`
    /// elapses, keeping any push seen on the way.
    ///
    /// Returns `None` on the timeout rather than erroring: "no session
    /// change" is a *verdict* here, not a transport fault, and the caller
    /// decides what it means.
    fn wait_for_session_change(
        &mut self,
        within: Duration,
        notifications: &mut Vec<PyValue>,
    ) -> Result<Option<SessionChange>> {
        let deadline = clock::instant() + within;
        loop {
            let left = deadline.saturating_duration_since(clock::instant());
            if left.is_zero() {
                break Ok(None);
            }
            match self.recv_within(left) {
                Ok(frame) => {
                    let PyValue::Instance { class, .. } = &frame else {
                        continue;
                    };
                    let name = String::from_utf8_lossy(class).into_owned();
                    match name.rsplit('.').next() {
                        Some("SessionChangeNotification") => {
                            break Ok(SessionChange::from_frame(&frame));
                        }
                        Some("Notification") => notifications.push(frame),
                        _ => {}
                    }
                }
                Err(Error::Io(e)) if is_timeout(&e) => {
                    break Ok(None);
                }
                Err(e) => break Err(e),
            }
        }
    }

    /// Complete the login handshake and return what the server said last.
    ///
    /// The sequence is H1..H10 and it is **server-initiated**: the first frame
    /// is the server's. Each step below is named for the capture it came from;
    /// `docs/reference/client-architecture.md` §2 has the whole exchange, and
    /// `eve-server/tests/loopback.rs` asserts the server's half of it.
    ///
    /// The two checks the real client makes on H8 are made here too, because a
    /// bot that accepts a handshake the client would reject is a bot that
    /// tests nothing:
    ///
    /// 1. `challenge_responsehash` must equal our own challenge hashed — a
    ///    mismatch is *"Server response signature incorrect"* in the client.
    /// 2. `user_logonqueueposition` of 2 or more aborts the login.
    pub fn handshake(&mut self) -> Result<Session> {
        self.handshake_as("token")
    }

    /// [`Self::handshake`], sending `token` as `user_sso_token`.
    ///
    /// A bot sends **its own name** (`docs/lanes/bots.md` §5): the server
    /// ignores the token today, and under either reading of the open account
    /// question -- "one token is one account" or "the token selects a
    /// character" -- a per-bot token is the one that keeps working, and the
    /// one a per-account `multiLoginBlocker` would not lump together.
    pub fn handshake_as(&mut self, token: &str) -> Result<Session> {
        // H1: the server greets us with a 7-tuple.
        let h1 = self.recv()?;
        let PyValue::Tuple(h1) = h1 else {
            return Err(Error::Protocol(format!("H1 is not a tuple: {h1:?}")));
        };
        if h1.len() != 7 {
            return Err(Error::Protocol(format!(
                "H1 must be a 7-tuple, got {} items",
                h1.len()
            )));
        }
        if h1.get(1) != Some(&PyValue::Int(MACHO_VERSION)) {
            return Err(Error::Protocol(format!(
                "H1 macho_version is {:?}, we speak {MACHO_VERSION}",
                h1.get(1)
            )));
        }

        // H2..H4: our version, the VK placeholder, and the crypto context.
        // `placebo` is the only one our server accepts, and the only one worth
        // having: `docs/guides/jail.md` on why no real crypto pack is wanted here.
        self.send(&PyValue::Tuple(vec![
            PyValue::Int(i64::from(eve_marshal::build::REFERENCE_LINE.secret)),
            PyValue::Int(MACHO_VERSION),
            PyValue::Int(0),
            PyValue::Real(BOOT_VERSION),
            PyValue::Int(BOOT_BUILD),
            bstr(&format!("{BOOT_CODENAME}@{BOOT_REGION}")),
        ]))?;
        self.send(&PyValue::Tuple(vec![
            PyValue::None,
            bstr("VK"),
            bstr("name-hash"),
        ]))?;
        self.send(&PyValue::Tuple(vec![
            bstr("placebo"),
            PyValue::Dict(vec![]),
        ]))?;

        // H5
        let h5 = self.recv()?;
        if h5 != bstr("OK CC") {
            return Err(Error::Protocol(format!("H5 should be 'OK CC', got {h5:?}")));
        }

        // H6: our challenge and who we claim to be.
        let client_challenge = vec![0x5Au8; CHALLENGE_LEN];
        self.send(&PyValue::Tuple(vec![
            PyValue::Bytes(client_challenge.clone()),
            login_request(token),
        ]))?;

        // H7
        let h7 = self.recv()?;
        if h7 != PyValue::Int(2) {
            return Err(Error::Protocol(format!(
                "H7 passwordVersion should be 2, got {h7:?}"
            )));
        }

        // H8: the server's challenge, its signed func, and the session dict.
        let h8 = self.recv()?;
        let PyValue::Tuple(h8) = h8 else {
            return Err(Error::Protocol(format!("H8 is not a tuple: {h8:?}")));
        };
        if h8.len() != 4 {
            return Err(Error::Protocol(format!(
                "H8 must be a 4-tuple, got {} items",
                h8.len()
            )));
        }
        let PyValue::Bytes(server_challenge) = &h8[0] else {
            return Err(Error::Protocol(format!(
                "H8[0] should be the server challenge, got {:?}",
                h8[0]
            )));
        };
        if server_challenge.len() != CHALLENGE_LEN {
            return Err(Error::Protocol(format!(
                "server challenge is {} bytes, expected {CHALLENGE_LEN}",
                server_challenge.len()
            )));
        }
        let server_challenge = server_challenge.clone();

        // The client's own first check.
        let want = PyValue::Bytes(crypto_hash(&[PyValue::Bytes(client_challenge)]).into_bytes());
        match dict_get(&h8[3], "challenge_responsehash") {
            Some(got) if *got == want => {}
            other => {
                return Err(Error::Protocol(format!(
                    "challenge_responsehash is {other:?}, expected {want:?} -- \
                     this is 'Server response signature incorrect' in the client"
                )));
            }
        }
        // And its second.
        if let Some(PyValue::Int(n)) = dict_get(&h8[3], "user_logonqueueposition")
            && *n >= 2
        {
            return Err(Error::Protocol(format!(
                "queue position {n} aborts the login (the client refuses 2 or more)"
            )));
        }

        // H9: prove we saw the server's challenge.
        self.send(&PyValue::Tuple(vec![
            PyValue::Bytes(crypto_hash(&[PyValue::Bytes(server_challenge.clone())]).into_bytes()),
            bstr(""),
            PyValue::Dict(vec![]),
        ]))?;

        // H10: the session, which is what everything after this hangs off.
        let session = self.recv()?;
        Ok(Session {
            server_challenge,
            h10: session,
        })
    }
}

/// The event name and arguments out of a `Notification` frame.
///
/// Returns `None` rather than panicking on anything unexpected: the server's
/// own loopback tests assert this shape exhaustively and are the right place
/// for that, while a bot meeting a frame it cannot read should say so and
/// carry on rather than die.
///
/// The walk is: packet state -> destination address (a `MachoAddress`
/// *instance*, because `__setstate__` is what the client rebuilds it with)
/// -> the broadcast 4-tuple `(4, broadcastID, narrowcast, idtype)`. The event
/// name is the `broadcastID`. The arguments are nested two pickles deep,
/// through `ObjectCall`'s and `ServiceCall`'s else branches.
pub fn notification(frame: &PyValue) -> Option<(String, Vec<PyValue>)> {
    let PyValue::Instance { class, state } = frame else {
        return None;
    };
    if !String::from_utf8_lossy(class).ends_with("machoNetPacket.Notification") {
        return None;
    }
    let PyValue::Tuple(t) = &**state else {
        return None;
    };
    let dest = t.get(2)?;
    let PyValue::Instance { state: dest, .. } = dest else {
        return None;
    };
    let PyValue::Tuple(dest) = &**dest else {
        return None;
    };
    let PyValue::Bytes(event) = dest.get(1)? else {
        return None;
    };
    let event = String::from_utf8_lossy(event).into_owned();

    // The arguments, if they unwrap. An event whose body we cannot read is
    // still a useful thing to have seen, so the name comes back either way.
    let args = (|| {
        let PyValue::Tuple(body) = t.get(4)? else {
            return None;
        };
        let PyValue::Tuple(outer) = body.first()? else {
            return None;
        };
        let PyValue::Bytes(pickle) = outer.get(1)? else {
            return None;
        };
        let PyValue::Tuple(after_object) = loads(pickle).ok()? else {
            return None;
        };
        let PyValue::Tuple(after_service) = after_object.get(1)? else {
            return None;
        };
        let PyValue::Tuple(args) = after_service.get(1)? else {
            return None;
        };
        Some(args.clone())
    })()
    .unwrap_or_default();
    Some(unbatched(event, args))
}

/// **A one-entry `OnMultiEvent` is read as the event it carries.** The server
/// sends `OnTarget` inside one, `[(('OnTarget', *args), stamp)]`, as a live
/// server does (`eve_server::targeting::TargetChange`); every reader here
/// matches on `OnTarget`, so the batch is opened once, at the frame, and a
/// reader never sees it. A batch of several entries, or of anything else,
/// comes back as it is.
///
/// **A one-row `OnItemsChanged` is read as `OnItemChange`** the same way: the
/// server sends every item change as `OnItemsChanged([row], change,
/// location)` (`eve_server::notify::ItemChange`), and the readers here match
/// on the singular.
fn unbatched(event: String, args: Vec<PyValue>) -> (String, Vec<PyValue>) {
    if event == "OnMultiEvent"
        && let [PyValue::List(entries)] = &args[..]
        && let [PyValue::Tuple(entry)] = &entries[..]
        && let Some(PyValue::Tuple(call)) = entry.first()
        && let Some((PyValue::Bytes(name), rest)) = call.split_first()
    {
        return (String::from_utf8_lossy(name).into_owned(), rest.to_vec());
    }
    if event == "OnItemsChanged"
        && let [PyValue::List(rows), change, location] = &args[..]
        && let [row] = &rows[..]
    {
        return (
            "OnItemChange".to_owned(),
            vec![row.clone(), change.clone(), location.clone()],
        );
    }
    (event, args)
}

/// What a `SetState` told us: the clock, the ego, and the park.
#[derive(Debug)]
pub struct WorldState {
    /// The action's stamp, which is also the park's clock.
    pub stamp: i64,
    /// The ego ball's **item** id — the ship the camera attaches to.
    pub ego: i64,
    /// The decoded ball list.
    pub park: eve_server::destiny::Park,
}

impl WorldState {
    /// The ego ball, if the park actually contains it.
    ///
    /// `Park.SetState` assigns `self.ego` without checking that the ball
    /// exists, so a state can name an ego that is not there — which looks
    /// fine until the camera attaches to nothing. Hence an `Option` rather
    /// than an index.
    pub fn ego_ball(&self) -> Option<&eve_server::destiny::Ball> {
        self.park.balls.iter().find(|b| b.ball_id == self.ego)
    }
}

/// Every action in a `DoDestinyUpdate`, as `(stamp, name)`.
///
/// The incremental counterpart to [`read_set_state`]: after the bootstrap,
/// everything the server says about movement arrives as a list of
/// `(stamp, (name, args))`. A bot that only understood `SetState` would see a
/// warp as an opaque push.
///
/// Names only. [`read_action_list`] is the same walk with the operands kept,
/// and this is now expressed in terms of it -- most callers want to assert
/// *that* the server said `WarpTo` at a stamp and have no use for the numbers.
pub fn read_actions(args: &[PyValue]) -> Vec<(i64, String)> {
    read_action_list(args)
        .into_iter()
        .map(|(stamp, name, _)| (stamp, name))
        .collect()
}

/// Every action in a `DoDestinyUpdate`, with its operands.
///
/// `(stamp, name, operands)`. The operands are the positional arguments the
/// client splats as `(*args)`, and the first is the ball id for every action
/// this server sends (`destiny::Action::args`).
///
/// **Added when an assertion needed a number rather than a name**:
/// human-checks §7 step 7 asks whether an align leaves the throttle where the
/// player put it or jumps it to full, and "the batch contained a
/// `SetSpeedFraction`" cannot answer that. [`real_at`] reads one out.
pub fn read_action_list(args: &[PyValue]) -> Vec<(i64, String, Vec<PyValue>)> {
    let Some(PyValue::List(actions)) = args.first() else {
        return Vec::new();
    };
    actions
        .iter()
        .filter_map(|action| {
            let PyValue::Tuple(action) = action else {
                return None;
            };
            let PyValue::Int(stamp) = action.first()? else {
                return None;
            };
            let PyValue::Tuple(named) = action.get(1)? else {
                return None;
            };
            let PyValue::Bytes(name) = named.first()? else {
                return None;
            };
            // `('Name', (arg, arg, ...))` -- the operands are a tuple in the
            // second slot. An action with none still has the tuple.
            let operands = match named.get(1) {
                Some(PyValue::Tuple(items)) => items.clone(),
                _ => Vec::new(),
            };
            Some((*stamp, String::from_utf8_lossy(name).into_owned(), operands))
        })
        .collect()
}

/// One operand as an `f64`, accepting the `Real` the server sends and the
/// `Int` a whole number could arrive as.
///
/// `None` rather than a default, because "the throttle was absent" and "the
/// throttle was zero" are the two answers step 7 is trying to tell apart.
pub fn real_at(operands: &[PyValue], index: usize) -> Option<f64> {
    match operands.get(index)? {
        PyValue::Real(v) => Some(*v),
        PyValue::Int(v) => Some(*v as f64),
        _ => None,
    }
}

/// Read a `DoDestinyUpdate`'s arguments into a [`WorldState`].
///
/// The shape is `[[(stamp, ('SetState', (bag,)))], waitForBubble]`, and the
/// bag is a `KeyVal` whose state is an attribute dict in **value-then-key**
/// order. Only a `SetState` is understood here: an incremental action list is
/// a different thing and pretending to read one would be worse than
/// returning `None`.
///
/// **This decodes with the server's own `Park::decode`, on purpose.** A
/// second blob reader in this crate would be a second thing to keep right,
/// and the one in `eve-server` is already `[V]` against `_destiny.dll`'s
/// `ReadFullStateFromStream`. The cost is that a bot cannot catch our
/// encoder being wrong — it round-trips our own bytes and agrees. That is the
/// same limitation as the rest of this crate and it is the client suite's job.
pub fn read_set_state(args: &[PyValue]) -> Option<WorldState> {
    let PyValue::List(actions) = args.first()? else {
        return None;
    };
    let PyValue::Tuple(action) = actions.first()? else {
        return None;
    };
    let PyValue::Int(stamp) = action.first()? else {
        return None;
    };
    let PyValue::Tuple(named) = action.get(1)? else {
        return None;
    };
    let PyValue::Bytes(name) = named.first()? else {
        return None;
    };
    if name != b"SetState" {
        return None;
    }
    let PyValue::Tuple(operands) = named.get(1)? else {
        return None;
    };
    let PyValue::Instance { state: bag, .. } = operands.first()? else {
        return None;
    };
    let PyValue::Int(ego) = dict_get(bag, "ego")? else {
        return None;
    };
    let PyValue::Bytes(blob) = dict_get(bag, "state")? else {
        return None;
    };
    Some(WorldState {
        stamp: *stamp,
        ego: *ego,
        park: eve_server::destiny::Park::decode(blob)?,
    })
}

/// A call's return value, out of the `CallRsp` packet that carried it.
///
/// **The payload is a pickle, not the value.** The client feeds it straight to
/// `blue.marshal.Load`, so the bytes at `state[4][0]` have to be loaded before
/// anything in them can be read. Forgetting that is not a parse error -- it is
/// a `Bytes` that looks like a leaf, so a search through the packet simply
/// finds nothing and reports an empty result. That is exactly how the first
/// bound call here failed, with "nothing is bound yet" against a server that
/// had answered correctly.
pub fn reply_value(frame: &PyValue) -> Option<PyValue> {
    let PyValue::Instance { class, state } = frame else {
        return None;
    };
    if !String::from_utf8_lossy(class).ends_with("machoNetPacket.CallRsp") {
        return None;
    }
    let PyValue::Tuple(t) = &**state else {
        return None;
    };
    let PyValue::Tuple(body) = t.get(4)? else {
        return None;
    };
    let PyValue::Bytes(pickle) = body.first()? else {
        return None;
    };
    loads(pickle).ok()
}

/// Every bound-object reference in a value, in order.
///
/// A reference is a callback whose child is a whole second marshal stream
/// holding `(objectID, refID)` -- what `ObjectCall.ParseObjectID` loads on
/// the client side (`eve_proto::object_reference`). **All of them, not the
/// first**: a fused `GetInventory` answers `(invbroker ref, inventory ref)`,
/// and the first-only walk this replaced could only ever find the invbroker.
pub fn object_refs(v: &PyValue) -> Vec<ObjectRef> {
    fn walk(v: &PyValue, depth: usize, out: &mut Vec<ObjectRef>) {
        if depth > 8 {
            return;
        }
        if let PyValue::Callback(child) = v
            && let PyValue::Bytes(inner) = child.as_ref()
            && let Ok(PyValue::Tuple(parts)) = loads(inner)
            && let Some(PyValue::Bytes(id)) = parts.first()
        {
            out.push(ObjectRef(String::from_utf8_lossy(id).into_owned()));
            return;
        }
        match v {
            PyValue::Tuple(items) | PyValue::List(items) => {
                items.iter().for_each(|i| walk(i, depth + 1, out));
            }
            PyValue::Instance { state, .. } => walk(state, depth + 1, out),
            PyValue::Dict(entries) => entries
                .iter()
                .for_each(|(value, _)| walk(value, depth + 1, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, 0, &mut out);
    out
}

/// The `UserError` key in an `ErrorResponse` frame, if that is what it is.
///
/// The shape is `ErrorResponse(..., payload=(pickle,))` with the pickle a
/// `UserError(msg, dict)` reduce (`eve_proto::user_error`,
/// `wrapped_exception_payload`). `None` for any other frame.
pub fn user_error(frame: &PyValue) -> Option<String> {
    let packet = eve_proto::MachoPacket::from_value(frame).ok()?;
    if packet.command != eve_marshal::MachoNetMsg::ErrorResponse {
        return None;
    }
    packet.body.iter().find_map(|part| {
        let PyValue::Tuple(wrapped) = part else {
            return None;
        };
        let PyValue::Bytes(pickle) = wrapped.first()? else {
            return None;
        };
        let PyValue::ObjectEx { header, .. } = loads(pickle).ok()? else {
            return None;
        };
        let PyValue::Tuple(header) = header.as_ref() else {
            return None;
        };
        let PyValue::Tuple(args) = header.get(1)? else {
            return None;
        };
        args.first().and_then(text)
    })
}

/// The station-bound moniker's params, `(stationID, groupStation)`.
fn station_moniker(station_id: i64) -> PyValue {
    PyValue::Tuple(vec![PyValue::Int(station_id), PyValue::Int(GROUP_STATION)])
}

fn is_timeout(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// One frame as an [`Event`].
fn event_of(frame: &PyValue) -> Event {
    if let Some(change) = SessionChange::from_frame(frame) {
        return Event::SessionChange(change);
    }
    if let Some((event, args)) = notification(frame) {
        return Event::Push(event, args);
    }
    Event::StrayReply(
        eve_proto::MachoPacket::from_value(frame)
            .ok()
            .and_then(|p| p.destination.call_id()),
    )
}

/// One call's reply, and everything that arrived beside it.
#[derive(Debug)]
pub struct CallOutcome {
    /// The `CallRsp` packet, as received.
    pub result: PyValue,
    /// Notifications that landed while we waited, in order. Kept rather than
    /// dropped: for a bot these are the interesting half.
    pub notifications: Vec<PyValue>,
    /// Whether a session change arrived with the reply. The only evidence that
    /// a `SelectCharacterID` or an undock actually took effect.
    pub session_changed: bool,
    /// What it said: every pair of every session change that arrived with
    /// the reply, a later one winning. `session_changed` is
    /// `session_change.is_some()`, kept for the callers that only ask "did
    /// it".
    pub session_change: Option<SessionChange>,
}

impl CallOutcome {
    /// Fold one more session change in.
    fn absorb(&mut self, change: SessionChange) {
        match &mut self.session_change {
            Some(have) => have.pairs.extend(change.pairs),
            None => self.session_change = Some(change),
        }
    }

    /// The return value, unpickled, or `None` when the answer was not a
    /// `CallRsp`.
    pub fn value(&self) -> Option<PyValue> {
        reply_value(&self.result)
    }

    /// **The refusal's key**, when the call was answered with a raised
    /// `UserError` -- `"EffectAlreadyActive2"`, `"CustomNotify"`,
    /// `"ShipNotYours"` -- or `None` when it was answered.
    ///
    /// Read the key, never the text: the key is what the client branches on
    /// (`clientDogmaLocation.py:857` swallows `EffectAlreadyActive2` and
    /// nothing else) **[V]**.
    pub fn refusal(&self) -> Option<String> {
        user_error(&self.result)
    }
}

/// **A warp call's answer, read before waiting for the warp** (SOAK-S1): the
/// server answers a refused `CmdWarpToStuff` with `Ok(CallOutcome)` carrying a
/// `UserError`; the key (`WarpDisrupted`, a field's
/// `WarpScrambledByWarpDisruptionFieldGenerator`, `WarpScrambled`, ...) is
/// what the bot's scram and bubble senses read, so it must reach the verdict
/// as text instead of a later "no WarpTo" timeout. A key refuses; none passes.
pub fn warp_answer(what: &str, key: Option<String>) -> Result<()> {
    match key {
        Some(k) => Err(Error::Refused(format!("{what}: {k}"))),
        None => Ok(()),
    }
}

/// What a completed handshake leaves us holding.
#[derive(Debug)]
pub struct Session {
    /// Kept because the post-handshake call sequence refers back to it.
    pub server_challenge: Vec<u8>,
    /// The server's last handshake frame, verbatim. Not interpreted yet:
    /// milestone 1 is "log in and hold the session", and inventing a reading
    /// of this before a scenario needs one would be inventing a claim.
    pub h10: PyValue,
}

/// The login dict the client sends in H6.
///
/// `user_password_hash` is twenty zero bytes and `user_sso_token` is a
/// placeholder: our server validates neither, by design — it only requires the
/// token to be *present* (`tools/run-client.sh` passes `/ssoToken=` for the
/// same reason). A bot therefore needs no account anywhere.
fn login_request(token: &str) -> PyValue {
    dict(vec![
        ("macho_version", PyValue::Int(MACHO_VERSION)),
        ("boot_version", PyValue::Real(BOOT_VERSION)),
        ("boot_build", PyValue::Int(BOOT_BUILD)),
        ("boot_codename", bstr(BOOT_CODENAME)),
        ("boot_region", bstr(BOOT_REGION)),
        ("user_name", bstr(USER_NAME)),
        ("user_password", PyValue::None),
        ("user_password_hash", PyValue::Bytes(vec![0u8; 20])),
        ("user_languageid", bstr("EN")),
        ("user_affiliateid", PyValue::Int(0)),
        ("user_sso_token", bstr(token)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-entry `OnMultiEvent` reads as the call it carries; a batch of
    /// two, and any other event, read as they came.
    #[test]
    fn a_one_entry_multi_event_reads_as_its_call() {
        let call = |what: &[u8]| {
            PyValue::Tuple(vec![
                PyValue::Tuple(vec![
                    PyValue::Bytes(b"OnTarget".to_vec()),
                    PyValue::Bytes(what.to_vec()),
                    PyValue::Int(42),
                ]),
                PyValue::Int(9),
            ])
        };
        let one = vec![PyValue::List(vec![call(b"lost")])];
        assert_eq!(
            unbatched("OnMultiEvent".into(), one),
            (
                "OnTarget".to_owned(),
                vec![PyValue::Bytes(b"lost".to_vec()), PyValue::Int(42)]
            )
        );
        let two = vec![PyValue::List(vec![call(b"lost"), call(b"add")])];
        assert_eq!(
            unbatched("OnMultiEvent".into(), two.clone()).0,
            "OnMultiEvent"
        );
        assert_eq!(
            unbatched("OnTarget".into(), vec![PyValue::Int(1)]),
            ("OnTarget".to_owned(), vec![PyValue::Int(1)])
        );
        let items = vec![
            PyValue::List(vec![PyValue::Int(5)]),
            PyValue::Dict(vec![]),
            PyValue::None,
        ];
        assert_eq!(
            unbatched("OnItemsChanged".into(), items),
            (
                "OnItemChange".to_owned(),
                vec![PyValue::Int(5), PyValue::Dict(vec![]), PyValue::None]
            )
        );
    }

    /// Read one framed value off a fixture stream.
    fn read_value(stream: &mut TcpStream, buf: &mut Vec<u8>) -> PyValue {
        let mut chunk = [0u8; 4096];
        loop {
            if let Some((frame, used)) = try_split_frame(buf).expect("framing") {
                let v = loads(frame.payload).expect("payload");
                buf.drain(..used);
                return v;
            }
            let n = stream.read(&mut chunk).expect("read");
            assert!(n > 0, "peer closed");
            buf.extend_from_slice(&chunk[..n]);
        }
    }

    fn write_value(stream: &mut TcpStream, v: &PyValue) {
        let mut framed = Vec::new();
        write_frame(&dumps(v), &mut framed).expect("frame");
        stream.write_all(&framed).expect("write");
    }

    #[test]
    fn a_ping_is_answered_while_waiting_for_a_call_reply() {
        use eve_marshal::MachoNetMsg;
        use eve_proto::{MachoAddress, MachoPacket};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            let req = MachoPacket::from_value(&read_value(&mut s, &mut buf)).unwrap();
            // A ping first, as the server's keepalive would send it.
            let ping = MachoPacket::new(
                MachoNetMsg::PingReq,
                MachoAddress::node(1),
                MachoAddress::client(1),
                vec![PyValue::List(vec![PyValue::Int(5)])],
            );
            write_value(&mut s, &ping.to_value().unwrap());
            let seen = MachoPacket::from_value(&read_value(&mut s, &mut buf)).unwrap();
            let rsp = MachoPacket::respond_to(
                &req,
                MachoNetMsg::CallRsp,
                vec![eve_proto::response_payload(&PyValue::Int(3))],
            );
            write_value(&mut s, &rsp.to_value().unwrap());
            seen
        });
        let mut conn = Conn::connect(addr, Duration::from_secs(5)).unwrap();
        let out = conn
            .call("svc", "Method", vec![])
            .expect("the call returns");
        let got = MachoPacket::from_value(&out.result).unwrap();
        assert_eq!(got.command, MachoNetMsg::CallRsp);
        let seen = server.join().unwrap();
        assert_eq!(
            seen.command,
            MachoNetMsg::PingRsp,
            "the server got an answer"
        );
        assert_eq!(seen.body, vec![PyValue::List(vec![PyValue::Int(5)])]);
    }

    #[test]
    fn a_ping_does_not_stretch_a_pump_past_its_window() {
        use eve_marshal::MachoNetMsg;
        use eve_proto::{MachoAddress, MachoPacket};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            let ping = MachoPacket::new(
                MachoNetMsg::PingReq,
                MachoAddress::node(1),
                MachoAddress::client(1),
                vec![PyValue::List(vec![PyValue::Int(7)])],
            )
            .to_value()
            .unwrap();
            // The first ping is answered inside the window.
            write_value(&mut s, &ping);
            let first = MachoPacket::from_value(&read_value(&mut s, &mut buf)).unwrap();
            // Then a keepalive faster than the bot's window, for longer than
            // the window: before the fix each one restarted the read.
            let mut framed = Vec::new();
            write_frame(&dumps(&ping), &mut framed).unwrap();
            for _ in 0..30 {
                if s.write_all(&framed).is_err() {
                    break; // the bot hung up: done
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            first.command
        });
        let mut conn = Conn::connect(addr, Duration::from_secs(5)).unwrap();
        let t0 = clock::wall();
        let got = conn.pump(Duration::from_millis(400));
        let took = clock::wall_since(t0);
        drop(conn);
        let first = server.join().unwrap();
        assert_eq!(
            first,
            MachoNetMsg::PingRsp,
            "a ping in the window is still answered"
        );
        let events = got.expect("a quiet window is not an error");
        assert!(events.is_empty(), "pings are no events: {events:?}");
        assert!(
            took < Duration::from_millis(2_000),
            "pump(400 ms) took {took:?}: a ping re-armed the whole wait"
        );
    }

    #[test]
    fn a_wire_dict_is_value_then_key() {
        // Backwards, this produces a dict the client reads as empty rather
        // than an error, so it is worth one assertion.
        let d = dict(vec![("k", PyValue::Int(7))]);
        let PyValue::Dict(entries) = &d else {
            panic!("not a dict")
        };
        assert_eq!(entries[0].0, PyValue::Int(7), "value comes first");
        assert_eq!(entries[0].1, bstr("k"), "key comes second");
        assert_eq!(dict_get(&d, "k"), Some(&PyValue::Int(7)));
        assert_eq!(dict_get(&d, "absent"), None);
    }

    #[test]
    fn the_sso_token_is_whatever_the_bot_says_it_is() {
        // A bot sends its own name (`docs/lanes/bots.md` §5).
        let r = login_request("Miner1");
        assert_eq!(dict_get(&r, "user_sso_token"), Some(&bstr("Miner1")));
    }

    #[test]
    fn every_object_reference_is_found_in_order() {
        // A fused `GetInventory`'s reply, as `handle_bind` builds it.
        let reply = PyValue::Tuple(vec![
            eve_proto::object_reference("N=1:4", 0),
            eve_proto::object_reference("N=1:3", 0),
        ]);
        assert_eq!(
            object_refs(&reply),
            vec![ObjectRef("N=1:4".to_owned()), ObjectRef("N=1:3".to_owned())]
        );
        assert!(object_refs(&PyValue::None).is_empty());
    }

    #[test]
    fn a_user_error_s_key_is_read_out_of_an_error_response() {
        let frame = eve_proto::MachoPacket::error_response(
            eve_proto::MachoAddress::node(1),
            eve_proto::MachoAddress::client(1),
            eve_marshal::MachoNetMsg::CallReq,
            2,
            eve_proto::wrapped_exception_payload(&eve_proto::user_error(
                "EffectAlreadyActive2",
                vec![],
            )),
        )
        .to_value()
        .expect("encodes");
        assert_eq!(user_error(&frame).as_deref(), Some("EffectAlreadyActive2"));
        assert_eq!(user_error(&PyValue::None), None);
    }

    #[test]
    fn the_login_request_names_our_build_and_nothing_else() {
        // The server ends the connection with a named reason on a version it
        // does not know, so these five are the ones that matter.
        let r = login_request("token");
        assert_eq!(
            dict_get(&r, "macho_version"),
            Some(&PyValue::Int(MACHO_VERSION))
        );
        assert_eq!(
            dict_get(&r, "boot_build"),
            Some(&PyValue::Int(i64::from(eve_marshal::build::REFERENCE)))
        );
        assert_eq!(dict_get(&r, "boot_codename"), Some(&bstr(BOOT_CODENAME)));
        assert_eq!(dict_get(&r, "boot_region"), Some(&bstr("ccp")));
        assert_eq!(dict_get(&r, "user_name"), Some(&bstr("EVE-SSO-CONNECTION")));
        // Present but empty of meaning: the server checks neither.
        assert_eq!(
            dict_get(&r, "user_password_hash"),
            Some(&PyValue::Bytes(vec![0u8; 20]))
        );
    }

    /// CHATTER-2: the pre-jump hook gets the connection, runs once per
    /// call, and is gone when cleared.
    #[test]
    fn the_pre_jump_hook_runs_with_the_connection_until_cleared() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut c = Conn::connect(l.local_addr().unwrap(), Duration::from_secs(1)).unwrap();
        let n = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = n.clone();
        c.before_jump(Some(Box::new(move |conn: &mut Conn| {
            seen.set(seen.get() + 1);
            assert!(conn.session().solar_system_id().is_none());
        })));
        c.run_pre_jump();
        c.run_pre_jump();
        assert_eq!(n.get(), 2);
        c.before_jump(None);
        c.run_pre_jump();
        assert_eq!(n.get(), 2);
    }

    /// SOAK-S1: a refused warp's key reaches the verdict, so the bubble and
    /// scramble senses read it; an answered warp passes.
    #[test]
    fn a_warp_refusal_key_is_the_verdict_and_reads_as_its_kind() {
        let why = |key: &str| match warp_answer("CmdWarpToStuff", Some(key.to_owned())) {
            Err(Error::Refused(w)) => w,
            other => panic!("{other:?}"),
        };
        let disrupted = why("WarpDisrupted");
        assert!(crate::warp_bubble::is_refusal(&disrupted));
        assert!(!crate::scram::counts(&disrupted));
        assert!(!crate::scram::is_refusal(&disrupted));
        let field = why("WarpScrambledByWarpDisruptionFieldGenerator");
        assert!(crate::warp_bubble::is_refusal(&field));
        assert!(!crate::scram::is_refusal(&field), "a bubble, not a scram");
        let scram = why("WarpScrambled");
        assert!(crate::scram::is_refusal(&scram));
        assert!(!crate::warp_bubble::is_refusal(&scram));
        // Any other key still counts as a failure of the act.
        let other = why("ShipNotYours");
        assert!(crate::scram::counts(&other) && !crate::warp_bubble::is_refusal(&other));
        assert!(matches!(warp_answer("CmdWarpToStuff", None), Ok(())));
    }
}
