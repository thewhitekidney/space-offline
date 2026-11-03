//! The listeners: accept, name a capture file, hand the socket to a thread.
//!
//! One thread per listener (`--bind`, repeatable), each with its own live
//! count and cap, and chat beside each one. [`fail_closed`] refuses, at
//! startup, every listener a friend could reach unless `--accounts` is
//! given (IP27-a).

use crate::capture::Capture;
use crate::conn::{ConnEnd, serve};
use crate::dispatch::{CaptureOnly, Dispatch, GpsSession};
use crate::handshake::{self, CHALLENGE_LEN, Handshake};
use crate::push::{Wire, Writers, default_writers, spawn_writers};
use crate::service::Registry;
use eve_marshal::PyValue;
use eve_proto::{MachoAddress, MachoPacket};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The host side of the jail's veth pair: the one non-loopback address a
/// listener may use before accounts exist. [V] `tools/up.sh:37` (`HOST_IP`,
/// default `10.137.0.1`), `eve-dns/src/main.rs:5`. [A] that nobody overrides
/// `EVE_JAIL_HOST_IP`; falsified by a jail on another address, whose
/// listener this refuses at startup (fail closed, and the message names it).
pub const JAIL_HOST: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 137, 0, 1));

/// Default cap on live connections for a loopback or jail listener: the bots
/// and the owner's own client. [A] 500+ bots plus reconnects with headroom on
/// the dedicated host (PA-5; it was 128 for 50 bots, `docs/review/plan-review.md`
/// M6); falsified by a soak whose log shows `at its cap` while every bot is
/// healthy. Each connection costs about two descriptors on the machoNet side
/// and two on chat's, so [`fd_warning`] checks `ulimit -n` against it.
pub const LOCAL_CAP: usize = 4096;

/// Default cap for any other listener (friends, through a tunnel). [A] far
/// above a 2-10 player group; falsified by a friend refused at the cap.
pub const REMOTE_CAP: usize = 16;

/// How long the accept loop sleeps after an error that is not about one
/// connection, such as running out of descriptors (`EMFILE`). [A] long
/// enough not to spin, short enough that nobody notices; falsified by a log
/// of back-to-back accept errors.
pub const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// The highest role a session on a listener may hold.
///
/// The session role is `min(account role, ceiling)` (`docs/lanes/account-spec.md`
/// §4.1), applied by `crate::accounts::Accounts::resolve`. Without
/// `--accounts` [`fail_closed`] still refuses `Player`: the catch-all is a
/// GM, and a friends' listener must never face it (RR1 H3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Ceiling {
    #[default]
    Gm,
    Player,
}

/// One `--bind`:
/// `ADDR[,ceiling=gm|player][,cap=N][,account=UID][,chat=ADDR][,advertise=IP]
/// [,signup=on|off][,capture=on|off][,botnames=on|off]`.
///
/// ```
/// use eve_server::server::{Ceiling, Listener};
/// let l = Listener::parse("127.0.0.1:26001,ceiling=player,cap=4").unwrap();
/// assert_eq!(l.ceiling, Ceiling::Player);
/// assert_eq!(l.cap(), 4);
/// assert!(Listener::parse("127.0.0.1:26001,colour=red").is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    pub addr: SocketAddr,
    pub ceiling: Ceiling,
    /// Live connections allowed at once. `None` is [`LOCAL_CAP`] or
    /// [`REMOTE_CAP`] by address. Past it a new stream is closed at once.
    pub cap: Option<usize>,
    /// Identity by listener: every session here is this account, whatever
    /// its token (`account-spec.md` §2.2, §4.1).
    pub account: Option<i64>,
    /// Where chat listens beside this listener; naming it turns chat on.
    /// `None` is no chat here, unless `Config::chat` (`--chat`) puts it on
    /// the chat port of this listener's own address.
    pub chat: Option<SocketAddr>,
    /// The address chat tells a client on this listener to dial. `None` is
    /// the address its connection arrived on.
    pub advertise: Option<IpAddr>,
    /// `signup=on`: with `--world-accounts`, a login with an unknown user
    /// name makes that account, as a `player` (`crate::account_book`).
    /// Off by default; refused at startup in the other account modes.
    pub signup: bool,
    /// `capture=off`: with `--capture`, this listener's connections are not
    /// recorded (the dev world's loopback, where its 50 bots log in, so only
    /// the human client's traffic is kept). On by default.
    pub capture: bool,
    /// `botnames=on`: a sign-up here gets its character from the bots' name
    /// pool (`crate::bot_names`) instead of the account's own name. Only the
    /// one-shot loopback listener that signs the bots up turns it on.
    pub bot_names: bool,
}

impl Listener {
    /// A `ceiling=gm` listener with the default cap and nothing else.
    pub fn new(addr: SocketAddr) -> Self {
        Listener {
            addr,
            ceiling: Ceiling::Gm,
            cap: None,
            account: None,
            chat: None,
            advertise: None,
            signup: false,
            capture: true,
            bot_names: false,
        }
    }

    /// Parse one `--bind` value. Unknown or repeated keys are errors.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut parts = spec.split(',');
        let addr = parts.next().unwrap_or_default();
        let mut l = Listener::new(addr.parse().map_err(|e| format!("{addr}: {e}"))?);
        let mut seen: Vec<&str> = Vec::new();
        for part in parts {
            let (key, value) = part
                .split_once('=')
                .ok_or_else(|| format!("{part}: expected key=value"))?;
            if seen.contains(&key) {
                return Err(format!("{key}= given twice"));
            }
            seen.push(key);
            let bad = |e: &dyn std::fmt::Display| format!("{key}={value}: {e}");
            match key {
                "ceiling" => {
                    l.ceiling = match value {
                        "gm" => Ceiling::Gm,
                        "player" => Ceiling::Player,
                        _ => return Err(bad(&"expected gm or player")),
                    }
                }
                "cap" => match value.parse::<usize>() {
                    Ok(0) => return Err(bad(&"a cap of 0 refuses everyone")),
                    Ok(n) => l.cap = Some(n),
                    Err(e) => return Err(bad(&e)),
                },
                "account" => l.account = Some(value.parse().map_err(|e| bad(&e))?),
                "chat" => l.chat = Some(value.parse().map_err(|e| bad(&e))?),
                "advertise" => l.advertise = Some(value.parse().map_err(|e| bad(&e))?),
                "signup" => {
                    l.signup = match value {
                        "on" => true,
                        "off" => false,
                        _ => return Err(bad(&"expected on or off")),
                    }
                }
                "botnames" => {
                    l.bot_names = match value {
                        "on" => true,
                        "off" => false,
                        _ => return Err(bad(&"expected on or off")),
                    }
                }
                "capture" => {
                    l.capture = match value {
                        "on" => true,
                        "off" => false,
                        _ => return Err(bad(&"expected on or off")),
                    }
                }
                _ => return Err(format!("unknown key {key}=")),
            }
        }
        Ok(l)
    }

    /// The cap in force: the configured one, else by address.
    pub fn cap(&self) -> usize {
        self.cap.unwrap_or(if is_local(self.addr.ip()) {
            LOCAL_CAP
        } else {
            REMOTE_CAP
        })
    }
}

/// Loopback, or the jail's host address: reachable by this machine's own
/// processes and the jailed client, and by nobody else.
pub fn is_local(ip: IpAddr) -> bool {
    ip.is_loopback() || ip == JAIL_HOST
}

/// **Fail closed** (`docs/review/plan-review.md` C2, H3).
///
/// Without `--accounts` every session is the catch-all GM, so refuse, at
/// startup, any listener a friend could reach: an address that is not
/// loopback or [`JAIL_HOST`] (chat's too), and any `ceiling=player` (the
/// catch-all must never face a friend). `account=` must name the one
/// account there is, the catch-all `user_id`.
///
/// With `--accounts` (IP27-a) a public or `ceiling=player` listener is
/// allowed: an unknown token is refused there and the ceiling caps the
/// role. `account=` must then name an account in the file.
pub fn fail_closed(l: &Listener, hs: &handshake::Config) -> Result<(), String> {
    let in_world = hs.accounts.as_ref().is_some_and(|a| a.book().is_some());
    if l.signup && !in_world {
        return Err("signup=on needs --world-accounts".to_owned());
    }
    if l.bot_names && (!l.signup || !is_local(l.addr.ip())) {
        return Err("botnames=on needs signup=on on a loopback or jail-host address".to_owned());
    }
    // ACC2: identity by listener names a file line; world accounts have
    // none at startup, so the two are not combined.
    if in_world && l.account.is_some() {
        return Err("account= is refused with --world-accounts".to_owned());
    }
    if let Some(accounts) = &hs.accounts {
        return match l.account {
            Some(uid) if accounts.by_user(uid).is_none() => {
                Err(format!("account={uid} names no account in --accounts"))
            }
            _ => Ok(()),
        };
    }
    let user_id = hs.user_id;
    const UNTIL: &str = "is refused without --accounts: every session is a GM";
    if l.ceiling == Ceiling::Player {
        return Err(format!("ceiling=player {UNTIL}"));
    }
    if !is_local(l.addr.ip()) {
        return Err(format!(
            "{} is not loopback or the jail host {JAIL_HOST}, and {UNTIL}",
            l.addr.ip()
        ));
    }
    if let Some(chat) = l.chat
        && !is_local(chat.ip())
    {
        return Err(format!(
            "chat={chat} is not loopback or the jail host, and {UNTIL}"
        ));
    }
    match l.account {
        Some(uid) if uid != user_id => Err(format!(
            "account={uid} names no account: without --accounts the only one is \
             --user-id {user_id}"
        )),
        _ => Ok(()),
    }
}

/// Run an accept loop until `accept` yields `None`, which production never
/// does. **An accept error never ends it** (`docs/review/V2-wire.md` F4): it
/// is logged, and anything but a per-connection error (`ECONNABORTED`, a
/// reset, `EINTR`) also sleeps [`ACCEPT_BACKOFF`], so running out of
/// descriptors (`EMFILE`, `ENFILE`) neither kills the server nor spins.
pub fn accept_loop<T>(
    what: &str,
    mut accept: impl FnMut() -> Option<io::Result<T>>,
    mut handle: impl FnMut(T),
) {
    while let Some(next) = accept() {
        match next {
            Ok(conn) => handle(conn),
            Err(e) => match e.kind() {
                io::ErrorKind::Interrupted => {}
                io::ErrorKind::ConnectionAborted | io::ErrorKind::ConnectionReset => {
                    eprintln!("[accept] {what}: {e}");
                }
                _ => {
                    eprintln!("[accept] {what}: {e}; backing off");
                    std::thread::sleep(ACCEPT_BACKOFF);
                }
            },
        }
    }
}

/// One live connection's place under its listener's cap. Released on drop,
/// so a connection thread that panics or returns early gives it back.
struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(live: &Arc<AtomicUsize>, cap: usize) -> Option<Slot> {
        live.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            (n < cap).then_some(n + 1)
        })
        .ok()
        .map(|_| Slot(Arc::clone(live)))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A bound machoNet listener.
struct Bound {
    socket: TcpListener,
    spec: Arc<Listener>,
    live: Arc<AtomicUsize>,
}

/// A bound chat listener (its accept thread owns the socket).
struct ChatTap {
    addr: SocketAddr,
    live: Arc<AtomicUsize>,
}

/// What the dispatch seam is wired to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Run the GPS handshake.
    #[default]
    Gps,
    /// Answer nothing; log what the client sends unprompted.
    Capture,
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Every machoNet listener, in `--bind` order; at least one. The first
    /// is the one `run_once` and `local_addr` use.
    pub listeners: Vec<Listener>,
    /// The session handed to a client once it authenticates, including the
    /// role mask. See `crate::roles`.
    pub session: crate::session::Session,
    /// What the stub services answer from.
    pub sandbox: crate::model::SandboxConfig,
    /// Where to write one capture file per connection. `None` disables
    /// capture, which should only ever be a deliberate choice.
    pub capture_dir: Option<PathBuf>,
    pub mode: Mode,
    pub handshake: handshake::Config,
    /// Post this many inert probe notifications, one per second, to each
    /// connection once it is up. **Zero in every normal run**; see
    /// [`PUSH_PROBE_EVENT`].
    pub push_probe: u32,
    /// Where to keep the world between runs. `None` keeps nothing.
    ///
    /// `None` by default, so every test gets a fresh universe without
    /// arranging one -- and so that turning persistence on is a deliberate
    /// act rather than something a test inherits.
    pub world_file: Option<PathBuf>,
    /// `--accept-downgrade`: save over a world file holding records this
    /// build skips, after a backup (`crate::store`, IP11).
    pub accept_downgrade: bool,
    /// `--writers N`: socket-writer threads (`push::spawn_writers`); `None`
    /// is `push::default_writers()`.
    pub writers: Option<usize>,
    /// `--coalesce-destiny` (PA-10): merge a client's same-stamp Destiny
    /// pushes in its outbox. Off in `Config`'s default (tests); the command
    /// line turns it on unless `--no-coalesce-destiny` (destiny-batch).
    pub coalesce_destiny: bool,
    /// `--conn-stack-mib N`: the stack of each machoNet connection thread, in
    /// MiB; `None` is [`CONN_STACK`]. Virtual memory only, but it counts
    /// against `ulimit -v` and shows in a thread-heavy VSZ.
    pub conn_stack_mib: Option<usize>,
    /// `--status FILE`: the operator's TSV of live systems, rewritten every
    /// health report (`crate::live_status`, CP15). `None` writes none.
    pub status_file: Option<PathBuf>,
    /// `--stop-file PATH` (lane shutdown): the clock polls it, and its
    /// appearance stops the server cleanly -- `run` returns, and the binary
    /// hangs up, flushes the world and exits 0 (`crate::shutdown`). `None`
    /// polls nothing. The binary defaults it to `<world>.stop`.
    pub stop_file: Option<PathBuf>,
    /// In-game chat (`crate::chat`). `None` by default: the client is told
    /// no chat address and never dials, which is the only safe answer while
    /// nothing listens on 5222. A listener naming `chat=` turns it on.
    pub chat: Option<crate::chat::ChatConfig>,
    /// The public-gateway stub (`crate::gateway`): the port to listen on, on
    /// the address of every local listener. `None` (default) runs none, and
    /// the client's gateway requests then wait out their own deadlines (the
    /// slow fitting window and right-click menu). `--gateway` is `Some(8443)`.
    pub gateway: Option<u16>,
    /// `--gateway-addr`: the one address the gateway binds, instead of one
    /// beside each local listener. `None` (default) keeps it local.
    pub gateway_addr: Option<IpAddr>,
    /// The loaded tables (`crate::content`, `--content DIR`). `None` is the
    /// fixture, which `Registry::new` builds itself. `bind` moves it into
    /// the registry, as it does the universe (`docs/port-tracker.md` §67).
    pub content: Option<Arc<crate::content::Content>>,
    /// Builds each connection's dispatch in place of `mode`. **Tests only**:
    /// a double that panics is how `tests/wire_hostile.rs` proves that a
    /// panicking connection leaves no session behind.
    #[doc(hidden)]
    pub test_dispatch: Option<DispatchFactory>,
    /// `--time-scale N` (lane clock): the slow game timers run N times
    /// faster (`crate::game_clock`). `GameClock::REAL` by default.
    pub time_scale: crate::game_clock::GameClock,
    /// `--concord` / `--no-concord` (CONCORD-1b): CONCORD answers a
    /// high-sec crime. On by default.
    pub concord: bool,
    /// Undock and gate-arrival invulnerability (`crate::invuln`). **False
    /// here, true in the binary** (`--no-undock-invuln` turns it off): a
    /// test fixture that undocks and shoots at once must not wait 30 s.
    pub undock_invuln: bool,
    /// A dev run (lane `bot-ctl`): `/bots` is allowed. **Seam:** DEV-MODE's
    /// `--dev` sets this at landing. False by default.
    pub dev: bool,
    /// Where the bot control link writes its address and token files
    /// (`--bot-control DIR`; the binary defaults it to the world file's
    /// directory). `None`: no link is opened.
    pub bot_control: Option<PathBuf>,
    /// `--admin ADDR` (lane admin-web): the admin web listener's address.
    /// `None` (default): no listener. Its token file goes in the
    /// `bot_control` directory, so it needs one.
    pub admin: Option<std::net::SocketAddr>,
}

/// See `Config::test_dispatch`.
#[doc(hidden)]
#[derive(Clone)]
pub struct DispatchFactory(pub Arc<dyn Fn() -> Box<dyn Dispatch> + Send + Sync>);

impl std::fmt::Debug for DispatchFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DispatchFactory")
    }
}

impl Config {
    pub fn new(bind: SocketAddr) -> Self {
        Self::with_handshake(bind, handshake::Config::default())
    }

    /// Build a config around a handshake, deriving everything that must agree
    /// with it.
    ///
    /// **The handshake is a constructor argument and not a field to assign
    /// afterwards, and that is the whole point of this function.** Two
    /// identities have to be derived from it:
    ///
    /// - **The node id.** The handshake tells the client `proxy_nodeid`;
    ///   `populationCap.MachoResolveObject` tells it which node owns a bound
    ///   object. If those disagree the client addresses object calls to a node
    ///   we do not answer as, and `ServiceCallGPCS.CallUp` raises
    ///   `WrongMachoNode` -- a hang with no missing method behind it.
    /// - **The user id.** The **account** id. It appears in the handshake's
    ///   own H10, in `initialstate.userid`, and in
    ///   `GetCharacterSelectionData` -- three copies of one value, derived
    ///   here so they cannot disagree. It is *not* the destination of our
    ///   notifications: that is `Session::client_id`, minted per connection in
    ///   `serve_one`, and it used to be this field.
    ///
    /// This used to derive them from `handshake::Config::default()` and let
    /// callers overwrite `config.handshake` afterwards, which `cli.rs`
    /// did -- so `--proxy-node 5` produced a client told node 5, a resolve
    /// answering node 1, and object ids reading `N=5:n`, and `--user-id 7`
    /// produced three different user ids on one connection. Both flags were
    /// silently broken through the only entry point a person uses, while the
    /// test that existed to catch exactly this passed, because it exercised
    /// `Config::new` and not the binary.
    ///
    /// Taking the handshake by value is what makes that unrepresentable
    /// rather than merely documented.
    pub fn with_handshake(bind: SocketAddr, handshake: handshake::Config) -> Self {
        let sandbox = crate::model::SandboxConfig {
            node_id: handshake.proxy_node_id,
            user_id: handshake.user_id,
            ..crate::model::SandboxConfig::default()
        };
        let session = crate::session::Session {
            user_id: handshake.user_id,
            ..crate::session::Session::default()
        };
        Config {
            listeners: vec![Listener::new(bind)],
            session,
            sandbox,
            world_file: None,
            accept_downgrade: false,
            writers: None,
            coalesce_destiny: false,
            conn_stack_mib: None,
            status_file: None,
            stop_file: None,
            chat: None,
            gateway: None,
            gateway_addr: None,
            content: None,
            test_dispatch: None,
            capture_dir: None,
            mode: Mode::default(),
            handshake,
            push_probe: 0,
            time_scale: crate::game_clock::GameClock::REAL,
            concord: true,
            undock_invuln: false,
            dev: false,
            bot_control: None,
            admin: None,
        }
    }
}

/// The broadcast id of the inert probe push.
///
/// **Provably a no-op on the client**, which is exactly why it is the first
/// thing we send. `CoreBroadcastStuff.NotifyUp` looks the id up in
/// `sm.notify`'s `defaultdict(list)`, finds an empty list, iterates nothing and
/// returns. So this frame exercises the whole path we are unsure of -- frame,
/// mutex, capture, client decode, four GPCS layers, dispatch -- and none of the
/// part we are unsure *about*, the handler. A ticker that sends nothing would
/// only test the sleep.
pub const PUSH_PROBE_EVENT: &str = "OnEveOfflinePushProbe";

/// A listener that has bound its port but not yet accepted.
///
/// Binding is separate from serving so a caller — a test, or a supervisor that
/// wants to drop privileges — can learn the real address first. With port 0
/// that is the only way to find out which port was chosen.
pub struct Server {
    /// In `config.listeners` order; never empty.
    listeners: Vec<Bound>,
    /// Chat listeners, one per distinct chat address.
    chat: Vec<ChatTap>,
    /// Shared, not cloned, per connection: a `Config` used to be cloned into
    /// every connection thread, and it carried the loaded universe
    /// (`sandbox.celestials` / `sandbox.rocks`, ~1.2 GB with the full
    /// extracts), so each bot or player cost ~300 MB and fifty of them ran the
    /// server out of memory (`docs/port-tracker.md` §67). `bind` also moves
    /// those two out of the config into the registry, which owns them.
    config: Arc<Config>,
    accepted: Arc<AtomicU64>,
    /// Where to write, for every live connection. One per process, fanned out
    /// over by the one push ticker; see `crate::push`.
    writers: Arc<Writers>,
    /// The durable half of the world, shared with every connection.
    store: Arc<dyn crate::store::Store>,
    /// **The world. One per process, shared by every connection.**
    ///
    /// This used to be built per connection, from `config.sandbox.clone()`, so
    /// two clients were two universes that agreed only because they were
    /// cloned from the same `Default` -- and a reconnect threw away everything
    /// the player had done. There were in fact *two* copies per connection:
    /// `run` clones the whole `Config` into each thread and `serve_one` then
    /// cloned the sandbox out of it again. The second is gone; the first is
    /// now configuration only.
    ///
    /// See `crate::dispatch::GpsSession`'s `world` field for the locking
    /// discipline, which is load-bearing, and `docs/reference/world-state.md` for the
    /// design.
    world: Arc<RwLock<Registry>>,
    /// The clean stop (`crate::shutdown`): requested by the `/shutdown`
    /// countdown or the stop file, watched by every accept loop.
    stop: Arc<crate::shutdown::Stop>,
    /// The clock's priority at the world lock (`push::ClockGate`); `None`
    /// with `EVE_CLOCK_GATE=0`.
    gate: Option<Arc<crate::push::ClockGate>>,
}

/// **Shutdown writes the world** (CP52): payload turns no longer save, so
/// a dropped server renders its last dirty world and closes the saver,
/// which writes it, then releases the `--world` file's lock. A kill skips
/// this, and then the disk is at most `push::SAVE_EVERY` behind.
impl Drop for Server {
    fn drop(&mut self) {
        crate::dispatch::flush_world(&self.world, &*self.store);
    }
}

impl Server {
    pub fn bind(mut config: Config) -> io::Result<Self> {
        if config.listeners.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no listener"));
        }
        // Before anything binds: a refused listener must not be reachable
        // even for the moment between bind and the error.
        for l in &config.listeners {
            fail_closed(l, &config.handshake).map_err(|e| {
                io::Error::new(io::ErrorKind::PermissionDenied, format!("{}: {e}", l.addr))
            })?;
        }
        if let Some(dir) = &config.capture_dir {
            std::fs::create_dir_all(dir)?;
        }
        let mut listeners = Vec::with_capacity(config.listeners.len());
        for spec in &config.listeners {
            let socket = TcpListener::bind(spec.addr)
                .map_err(|e| io::Error::new(e.kind(), format!("bind {}: {e}", spec.addr)))?;
            listeners.push(Bound {
                socket,
                spec: Arc::new(spec.clone()),
                live: Arc::default(),
            });
        }
        // The one clock, started beside the other once-per-process setup. One
        // for the process and not one per connection: the simulation clock is
        // world state, and two tickers are two clocks in one solar system.
        // `docs/reference/push-path.md` §1.4.
        let writers = Arc::new(Writers::default());
        crate::push::set_coalesce_destiny(config.coalesce_destiny);
        println!(
            "[push] destiny coalescing: {}",
            if config.coalesce_destiny { "on" } else { "off" }
        );
        spawn_writers(
            Arc::clone(&writers),
            config.writers.unwrap_or_else(default_writers),
        )?;
        // Seeded once, here, beside the other once-per-process setup, and for
        // the same reason the ticker is: a world per connection is not a world.
        let world_lock = match &config.world_file {
            Some(path) => Some(crate::account_book::lock_world(path)?),
            None => None,
        };
        // The `--world` file's lock (`crate::account_book::lock_world`) is
        // the saver's: it is held until the saver's last write, so that
        // `--grant` can never write under a running server (CP52).
        let store: Arc<dyn crate::store::Store> = match (&config.world_file, world_lock) {
            (Some(path), lock) => Arc::new(crate::store::Saver::spawn(
                crate::store::FileStore::new(path.clone())
                    .accept_downgrade(config.accept_downgrade),
                lock,
            )?),
            (None, _) => Arc::new(crate::store::NoStore),
        };
        // The world's account mode and start characters come from the
        // handshake's accounts, here and nowhere else (IP27-b), so the
        // roster filter cannot disagree with who a connection resolves to.
        match &config.handshake.accounts {
            Some(accounts) => accounts.configure(&mut config.sandbox),
            None => crate::accounts::Accounts::catch_all(config.handshake.user_id)
                .configure(&mut config.sandbox),
        }
        // The universe moves into the registry; the config keeps only the
        // small settings each connection reads (§67).
        let celestials = std::mem::take(&mut config.sandbox.celestials);
        let rocks = std::mem::take(&mut config.sandbox.rocks);
        let mut registry = Registry::new(crate::model::SandboxConfig {
            celestials,
            rocks,
            ..config.sandbox.clone()
        });
        if let Some(content) = config.content.take() {
            registry = registry.with_content(content);
        }
        // **Loaded once, here, and a failure is fatal.** Everywhere else in
        // this server an unreadable thing is logged and stepped over, because
        // the alternative is refusing to serve. Not here: continuing with a
        // world we could not read would hand the player a fresh character and
        // quietly orphan everything they had, and they would find out by
        // looking at their empty hangar. Refusing to start is the kind
        // failure.
        //
        // **A `--character` the file does not know is new** (P-START-2): it
        // is placed at its school before the snapshot is applied, so a
        // saved character keeps its saved station, and credited after, so
        // its journal row is not replaced by the file's journal.
        let fresh = match store.load() {
            Ok(Some(snapshot)) => {
                println!(
                    "[world] loaded {} items, {} journal entries",
                    snapshot.items.len(),
                    snapshot.journal.len()
                );
                let saved: std::collections::BTreeSet<i64> = snapshot
                    .characters
                    .iter()
                    .map(|r| r.character_id)
                    .chain(snapshot.balances.iter().map(|(id, _)| *id))
                    .collect();
                let fresh = registry.start_roster(&saved);
                registry.apply_snapshot(snapshot);
                fresh
            }
            // A first run: draw the world record now, so the generators have
            // a seed from the first tick. `deltas_mut` marks the world dirty,
            // so the first save writes it.
            Ok(None) => {
                registry
                    .deltas_mut()
                    .set_world(crate::delta::World::mint(crate::pyutil::filetime_now()));
                registry.start_roster(&std::collections::BTreeSet::new())
            }
            Err(e) => return Err(io::Error::new(e.kind(), format!("world file: {e}"))),
        };
        // E4: the ledger log beside the world file. Its journal rows,
        // trades and closed orders are merged in (a pre-E4 world's are
        // migrated into it), and every such row from here on is appended to
        // it, never to the snapshot. Before the funding below, so a new
        // character's first row is logged too.
        if let Some(path) = &config.world_file {
            crate::ledger::open(&mut registry, &crate::ledger::dir_of(path))
                .map_err(|e| io::Error::new(e.kind(), format!("ledger log: {e}")))?;
        }
        // A module's stored settings apply here: after the world is read,
        // before the first character is funded from the rules.
        for hook in &crate::module::plan().world_loaded {
            hook(&mut registry).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        }
        for id in fresh {
            crate::start::fund_new_character(&mut registry, id, crate::pyutil::filetime_now());
        }
        if registry.config().dev {
            let n = crate::start::dev_top_up(&mut registry, crate::pyutil::filetime_now());
            println!("[dev] {n} characters topped up to 1,000,000,000 ISK");
        }
        // Each account's `start_character`, created or adopted once
        // (account-spec §3.4), before the names are checked: a created
        // character's name is a visible name too. A conflict refuses startup.
        registry
            .provision_start_characters()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        // NES-BACKEND: a vault or redeem queue with no catalogue to serve it.
        crate::nes::check_catalogue(&registry)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        // SKINS: licence rows with no SKIN table.
        crate::skins::check_table(&registry)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        // **No token is a visible name** (ruling 2), checked once the world's
        // names are known, before any connection is served. A hit refuses
        // startup and names the line, never the token.
        if let Some(accounts) = &config.handshake.accounts {
            let names = registry.config().characters.iter().map(|c| c.name.as_str());
            accounts
                .check_names(names)
                .map_err(|e| io::Error::new(io::ErrorKind::PermissionDenied, e))?;
        }
        // A listener naming `chat=` turns chat on, as `--chat-advertise` does.
        let chat_config = config.chat.or_else(|| {
            config
                .listeners
                .iter()
                .any(|l| l.chat.is_some())
                .then(crate::chat::ChatConfig::default)
        });
        let chat = chat_config.map(|cc| {
            let hub = crate::chat::ChatHub::new(cc);
            registry.set_chat(Arc::clone(&hub));
            hub.attach_writers(Arc::clone(&writers));
            (hub, cc)
        });
        // The stop handle knows every listener (to wake its `accept`) and the
        // stop file; a stale stop file from an earlier run is removed, or it
        // would stop this one on its first tick.
        let bound: Vec<SocketAddr> = listeners
            .iter()
            .map(|b| b.socket.local_addr())
            .collect::<io::Result<_>>()?;
        let stop = Arc::new(crate::shutdown::Stop::new(&bound, config.stop_file.clone()));
        stop.clear_file();
        if let Some(p) = stop.file() {
            println!("[shutdown] touch {} to stop cleanly", p.display());
        }
        crate::shutdown::attach(&mut registry, Arc::clone(&stop));
        // After the world is loaded: `game_clock::set` re-indexes what the
        // snapshot brought in.
        crate::game_clock::set(&mut registry, config.time_scale);
        let link = match &config.bot_control {
            Some(dir) => {
                let l = crate::bot_control::Link::open(Some(dir))?;
                println!(
                    "[bots] control link on {} (token in {})",
                    l.addr(),
                    dir.display()
                );
                Some(l)
            }
            None => None,
        };
        crate::bot_control::attach(&mut registry, config.dev, link);
        if let Some(addr) = config.admin {
            let Some(dir) = &config.bot_control else {
                return Err(io::Error::other(
                    "--admin needs a control directory for its token (--world or --bot-control)",
                ));
            };
            let map = crate::admin_web::MapData::from_registry(&registry);
            let shared = crate::admin_web::Shared::open(addr, dir, &map)?;
            println!(
                "[admin] web on http://{addr}/ ({} systems, {} gate links; token in {})",
                map.systems.len(),
                map.links.len(),
                dir.join(crate::admin_web::TOKEN_FILE).display()
            );
            crate::admin_web::attach(&mut registry, shared);
        }
        let mult = registry.config().rules.training_mult;
        if config.time_scale.training() > 1 || mult != 1.0 {
            // The rule's multiplier is on top of the clock's pace, and shows
            // only when a module or flag set it.
            let by_rule = if mult != 1.0 {
                format!(
                    ", rules.training_mult {mult} (pace {}x)",
                    config.time_scale.training_pace() * mult
                )
            } else {
                String::new()
            };
            println!(
                "[clock] training scale {}: skills train {}x (time scale {} x training {}){by_rule}",
                config.time_scale.training(),
                config.time_scale.training_pace(),
                config.time_scale.scale(),
                config.time_scale.training(),
            );
        }
        crate::concord::set_enabled(&mut registry, config.concord);
        crate::invuln::set_enabled(&mut registry, config.undock_invuln);
        if !config.time_scale.is_real() {
            println!(
                "[clock] time scale {}: slow game timers run {}x, a game day is {} wall minutes",
                config.time_scale.scale(),
                config.time_scale.scale(),
                config.time_scale.day() / 600_000_000,
            );
        }
        let world = Arc::new(RwLock::new(registry));
        // Chat listens beside each listener that names `chat=`, and with
        // `--chat` (`config.chat`) beside **every** one (`plan-review.md`
        // H2), on the chat port of its own address -- the address the jail
        // opens 5222 on, and the one a LAN friend already reaches. Two
        // listeners naming one chat address share it.
        let everywhere = config.chat.is_some();
        let mut chat_taps: Vec<ChatTap> = Vec::new();
        if let Some((hub, cc)) = chat {
            for b in &listeners {
                let want = match (b.spec.chat, everywhere) {
                    (Some(a), _) => a,
                    (None, true) => SocketAddr::new(b.socket.local_addr()?.ip(), cc.port),
                    (None, false) => continue,
                };
                if want.port() != 0 && chat_taps.iter().any(|t| t.addr == want) {
                    continue;
                }
                let socket = TcpListener::bind(want)
                    .map_err(|e| io::Error::new(e.kind(), format!("chat bind {want}: {e}")))?;
                let addr = socket.local_addr()?;
                let cap = b.spec.cap();
                println!("[chat] listening on {addr} (cap {cap})");
                let live = Arc::new(AtomicUsize::new(0));
                spawn_chat(socket, cap, Arc::clone(&live), &hub)?;
                chat_taps.push(ChatTap { addr, live });
            }
            // Woken by every directory publish, else once a RECONCILE_EVERY.
            // It never takes the world (PA-11).
            let hub = Arc::clone(&hub);
            std::thread::Builder::new()
                .name("chat-reconcile".to_owned())
                .spawn(move || crate::chat::run_reconcile(&hub, crate::chat::RECONCILE_EVERY))?;
        }
        // The public-gateway stub beside each local listener's address, one
        // per distinct address (`crate::gateway`).
        if let Some(port) = config.gateway {
            // One hub for every listener: the world's outbox publishes to it
            // after a turn and a tick (`crate::gateway_out`, GW-OUT).
            let notices = crate::gateway::NoticeHub::default();
            crate::gateway_out::install_hub(&notices);
            let mut done: Vec<SocketAddr> = Vec::new();
            let chosen: Vec<IpAddr> = match config.gateway_addr {
                Some(ip) => vec![ip],
                None => {
                    let mut ips = Vec::new();
                    for b in &listeners {
                        let ip = b.socket.local_addr()?.ip();
                        if is_local(ip) {
                            ips.push(ip);
                        }
                    }
                    ips
                }
            };
            for ip in chosen {
                let want = SocketAddr::new(ip, port);
                if port != 0 && done.contains(&want) {
                    continue;
                }
                let bound = crate::gateway::spawn_with(
                    want,
                    crate::gateway::Ground::Server(Arc::clone(&world)),
                    notices.clone(),
                )
                .map_err(|e| io::Error::new(e.kind(), format!("gateway bind {want}: {e}")))?;
                println!("[gateway] listening on {bound}");
                done.push(want);
            }
        }
        // The **second** clock, and the one that lets the world speak first.
        // `spawn_writers` above takes no lock but the wire's and must keep
        // that property; this one takes the world lock, so it is its own
        // thread. `push::spawn_world_ticker` has the two-halves lock
        // discipline that makes it safe, and `docs/port-tracker.md` §36 has
        // why three features were waiting on it.
        let gate = crate::push::clock_gate_on().then(|| Arc::new(crate::push::ClockGate::new()));
        crate::push::spawn_world_ticker_gated(
            Arc::clone(&world),
            Arc::clone(&writers),
            config.handshake.proxy_node_id,
            Arc::clone(&store),
            config.status_file.clone(),
            gate.clone(),
        )?;
        Ok(Server {
            listeners,
            chat: chat_taps,
            config: Arc::new(config),
            accepted: Arc::new(AtomicU64::new(0)),
            writers,
            world,
            store,
            stop,
            gate,
        })
    }

    /// The stop handle: `request` it to stop the server as `/shutdown` and
    /// the stop file do.
    pub fn stop(&self) -> Arc<crate::shutdown::Stop> {
        Arc::clone(&self.stop)
    }

    /// Why the server stopped, once it has.
    pub fn stop_reason(&self) -> Option<String> {
        self.stop.reason()
    }

    /// End every live session (lane shutdown): each wire is shut in both
    /// directions, so its connection thread reads end-of-stream and its
    /// session ends through the normal path; then wait up to `drain` for
    /// the listeners' live counts to reach zero. Returns how many were still
    /// live when it gave up. Call after `run` has returned, before dropping
    /// the server (whose `Drop` writes the world).
    pub fn hang_up(&self, drain: Duration) -> usize {
        let wires = self.writers.live();
        let n = wires.len();
        for wire in wires {
            match wire.lock() {
                Ok(mut w) => w.hang_up(),
                Err(e) => e.into_inner().hang_up(),
            }
        }
        let deadline = Instant::now() + drain;
        loop {
            let live: usize = self.live().iter().sum();
            if live == 0 || Instant::now() >= deadline {
                println!("[shutdown] hung up {n} session(s); {live} still live");
                return live;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The first listener's address.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listeners[0].socket.local_addr()
    }

    /// Every listener's address, in `config.listeners` order.
    pub fn local_addrs(&self) -> io::Result<Vec<SocketAddr>> {
        self.listeners
            .iter()
            .map(|b| b.socket.local_addr())
            .collect()
    }

    /// Every chat listener's address.
    pub fn chat_addrs(&self) -> Vec<SocketAddr> {
        self.chat.iter().map(|t| t.addr).collect()
    }

    /// Live connections per listener, in `local_addrs` order.
    pub fn live(&self) -> Vec<usize> {
        self.listeners
            .iter()
            .map(|b| b.live.load(Ordering::Acquire))
            .collect()
    }

    /// Live connections per chat listener, in `chat_addrs` order.
    pub fn chat_live(&self) -> Vec<usize> {
        self.chat
            .iter()
            .map(|t| t.live.load(Ordering::Acquire))
            .collect()
    }

    /// The shared world, for tests and diagnostics.
    pub fn world(&self) -> Arc<RwLock<Registry>> {
        Arc::clone(&self.world)
    }

    /// Accept forever, one thread per connection.
    ///
    /// Connections are detached rather than joined: a dead client must not hold
    /// up the next one.
    ///
    /// **This comment used to end "and there is no shared state for a panicking
    /// thread to corrupt — each connection owns its dispatch and its capture."
    /// That stopped being true in the commit that added the push path**, and it
    /// is the kind of claim that is worth more as a record of when it expired
    /// than as a reassurance. What is shared now is `Writers`, a list of `Weak`
    /// handles to connections' write sides, read by one ticker thread. The
    /// sharing is deliberately the smallest thing that works: a panicking
    /// connection poisons only its own `Wire` (where the ticker skips it
    /// permanently, which is correct — that stream may be mid-frame), and
    /// `Writers`' own lock recovers from poisoning on purpose, because
    /// otherwise one panic would take the push path away from every other
    /// player. The dispatch and the registry are still per-connection, which is
    /// **And the registry is no longer per-connection either**, which closes
    /// the "its own problem" this comment used to end on. One
    /// `Arc<RwLock<Registry>>` is built in `bind` and shared; the locking
    /// discipline that makes that safe is documented on
    /// `GpsSession::world`. What a panicking connection can now corrupt is the
    /// world itself, which is why the poisoning policy there is spelled out
    /// rather than inherited.
    ///
    /// **One accept thread per listener**, each with its own cap: a full bot
    /// listener never refuses the friends' one (`plan-review.md` M6). The
    /// first listener accepts on the calling thread. Returns when the server
    /// is asked to stop (`crate::shutdown`: every accept loop ends, so no new
    /// connection is served), or if a listener thread cannot be started; an
    /// accept error never ends it ([`accept_loop`]).
    pub fn run(&self) -> io::Result<()> {
        std::thread::scope(|s| {
            for (i, b) in self.listeners.iter().enumerate().skip(1) {
                std::thread::Builder::new()
                    .name(format!("accept-{i}"))
                    .spawn_scoped(s, move || self.accept_on(b))?;
            }
            self.accept_on(&self.listeners[0]);
            Ok(())
        })
    }

    /// Accept on one listener forever. Over its cap, a stream is closed at
    /// once, never queued: a refused client sees the close now rather than
    /// a hang (`docs/review/V6-security-ops.md` F9).
    fn accept_on(&self, b: &Bound) {
        let cap = b.spec.cap();
        let what = format!("listener {}", b.spec.addr);
        accept_loop(
            &what,
            || (!self.stop.requested()).then(|| b.socket.accept()),
            |(stream, peer): (TcpStream, SocketAddr)| {
                // Stopping: this is the wake-up (or a late client); closed.
                if self.stop.requested() {
                    drop(stream);
                    return;
                }
                let Some(slot) = Slot::take(&b.live, cap) else {
                    println!("[accept] {peer} refused: {what} is at its cap of {cap}");
                    drop(stream);
                    return;
                };
                let id = self.accepted.fetch_add(1, Ordering::Relaxed);
                let config = Arc::clone(&self.config);
                let spec = Arc::clone(&b.spec);
                let writers = Arc::clone(&self.writers);
                let world = Arc::clone(&self.world);
                let gate = self.gate.clone();
                let conn_stack = conn_stack_bytes(self.config.conn_stack_mib);
                let spawned = std::thread::Builder::new()
                    .name(format!("conn-{id}"))
                    .stack_size(conn_stack)
                    .spawn(move || {
                        let _slot = slot;
                        let end =
                            serve_one(stream, peer, id, &config, &spec, &writers, &world, &gate);
                        println!("[conn {id}] {peer} ended: {end}");
                    });
                // The closure, its stream and its slot are dropped with the
                // error: the peer sees a close and the cap is not leaked.
                if let Err(e) = spawned {
                    eprintln!("[accept] {peer}: cannot start a connection thread: {e}");
                }
            },
        );
    }

    /// Accept exactly one connection on the first listener and serve it on
    /// this thread. For tests and for a single-player session where a second
    /// connection is a bug.
    pub fn run_once(&self) -> io::Result<ConnEnd> {
        let b = &self.listeners[0];
        let (stream, peer) = b.socket.accept()?;
        let id = self.accepted.fetch_add(1, Ordering::Relaxed);
        // Counted, never refused: one connection cannot pass any cap.
        b.live.fetch_add(1, Ordering::AcqRel);
        let _slot = Slot(Arc::clone(&b.live));
        Ok(serve_one(
            stream,
            peer,
            id,
            &self.config,
            &b.spec,
            &self.writers,
            &self.world,
            &self.gate,
        ))
    }

    pub fn accepted(&self) -> u64 {
        self.accepted.load(Ordering::Relaxed)
    }
}

/// One client id per connection, by CCP's own formula.
///
/// `machoNet.GetIDOfAddress` is `offset * 10000000000 + nodeID`, with the
/// offset counting from 1. A recording from Tranquility has
/// `clientID=61731000003120`: the 6173rd client of node 1000003120.
///
/// **Per connection, not per account.** This is the whole point of the
/// function: `config.session.user_id` is an account id and was being used as
/// this address, so two connections of one account were one address, and
/// anything keyed by client id would keep only the later of the two.
const CLIENT_ID_STEP: i64 = 10_000_000_000;

/// A connection thread's stack. It decodes the client's bytes, so it needs
/// [`eve_marshal::value::DECODE_STACK`] (a debug build overflows the 2 MiB
/// default at the client's full nesting depth, which aborts the whole
/// process: V2 F8, `docs/lanes/ip12.md`), and as much again for dispatch.
/// [A] the second half; falsified by a stack overflow in a connection
/// thread. Virtual: only the pages a connection touches are resident.
const CONN_STACK: usize = 2 * eve_marshal::value::DECODE_STACK;

/// The connection thread stack in bytes: the flag, else [`CONN_STACK`].
fn conn_stack_bytes(mib: Option<usize>) -> usize {
    mib.map_or(CONN_STACK, |m| m.saturating_mul(1 << 20))
}

/// The soft `Max open files` limit from `/proc/self/limits` (`ulimit -n`),
/// `None` where that file is missing or unlimited.
pub fn fd_limit() -> Option<u64> {
    parse_fd_limit(&std::fs::read_to_string("/proc/self/limits").ok()?)
}

fn parse_fd_limit(limits: &str) -> Option<u64> {
    let line = limits.lines().find(|l| l.starts_with("Max open files"))?;
    line["Max open files".len()..]
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// A warning when the descriptor limit is under twice the largest cap, `None`
/// when it is enough (or unknown). [A] 2x: a bot holds a machoNet socket
/// (three descriptors with the writer clone and the outbox's kill handle)
/// and a chat one (two), so a full house can need more than 2x; this is the
/// floor under which `EMFILE` is certain. Falsified by `Too many open files`
/// in a log with the limit above it.
pub fn fd_warning(limit: Option<u64>, cap: usize) -> Option<String> {
    let limit = limit?;
    (limit < 2 * cap as u64).then(|| {
        format!(
            "[limits] WARNING: ulimit -n {limit} is below 2x the connection cap {cap}; \
             raise it (ulimit -n {}) or lower cap=N, or accepts will fail with EMFILE",
            4 * cap
        )
    })
}

fn client_id(ordinal: u64, node_id: i64) -> i64 {
    (ordinal as i64 + 1) * CLIENT_ID_STEP + node_id
}

/// Accept chat connections on `socket`, at most `cap` live at once. Bots
/// open chat too, so chat gets its own count (`plan-review.md` M6).
fn spawn_chat(
    socket: TcpListener,
    cap: usize,
    live: Arc<AtomicUsize>,
    hub: &Arc<crate::chat::ChatHub>,
) -> io::Result<()> {
    let hub = Arc::clone(hub);
    std::thread::Builder::new()
        .name("chat-accept".to_owned())
        .spawn(move || {
            let what = match socket.local_addr() {
                Ok(a) => format!("chat {a}"),
                Err(_) => "chat".to_owned(),
            };
            accept_loop(
                &what,
                || Some(socket.accept()),
                |(stream, peer): (TcpStream, SocketAddr)| {
                    let Some(slot) = Slot::take(&live, cap) else {
                        println!("[chat] {peer} refused: {what} is at its cap of {cap}");
                        return;
                    };
                    let hub = Arc::clone(&hub);
                    let spawned = std::thread::Builder::new()
                        .name("chat-conn".to_owned())
                        .spawn(move || {
                            let _slot = slot;
                            crate::chat::serve(stream, &hub);
                        });
                    if let Err(e) = spawned {
                        eprintln!("[chat] {peer}: cannot start a connection thread: {e}");
                    }
                },
            );
        })?;
    Ok(())
}

/// Takes the session out of the world when its connection ends, **however
/// it ends**: a return, an early return, or a panic unwinding through
/// `serve_one` (`docs/review/V2-wire.md` F3). Without it a panicking
/// dispatch left a ghost: a character other players see online, and that
/// `of_character` routes pushes to.
struct SessionGuard<'a> {
    world: &'a RwLock<Registry>,
    client_id: i64,
    wire: &'a Arc<Mutex<Wire>>,
}

impl Drop for SessionGuard<'_> {
    fn drop(&mut self) {
        {
            let mut w = crate::service::world_write_timed(self.world, "session_closed");
            w.session_closed(self.client_id);
            if let Some(hub) = w.chat() {
                hub.forget_client(self.client_id);
            }
        }
        // Explicitly, rather than left to the `Arc`: "the ticker cannot
        // write to a dead connection" is then a statement about
        // `Wire::close`, not about reference-count timing. A poisoned `Wire`
        // is a panic mid-write; there is nothing to close safely.
        if let Ok(mut w) = self.wire.lock() {
            w.close();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn serve_one(
    stream: TcpStream,
    peer: SocketAddr,
    id: u64,
    config: &Config,
    listener: &Listener,
    writers: &Arc<Writers>,
    world: &Arc<RwLock<Registry>>,
    gate: &Option<Arc<crate::push::ClockGate>>,
) -> ConnEnd {
    let capture = match config.capture_dir.as_ref().filter(|_| listener.capture) {
        Some(dir) => match open_capture(dir, id) {
            Ok(c) => Some(c),
            Err(e) => {
                // A capture we cannot write is worth shouting about, but it is
                // not a reason to refuse the connection.
                eprintln!("[conn {id}] capture disabled: {e}");
                None
            }
        },
        None => None,
    };

    // Minted per connection, from the accept ordinal. `config.session` is one
    // template cloned for every connection; leaving these two at their
    // defaults is what made every connection address 1 and report sid 1.
    let session = crate::session::Session {
        client_id: client_id(id, config.handshake.proxy_node_id),
        session_id: id as i64 + 1,
        ..config.session.clone()
    };

    let mut dispatch: Box<dyn Dispatch> = match (&config.test_dispatch, config.mode) {
        (Some(make), _) => (make.0)(),
        (None, Mode::Gps) => Box::new(
            GpsSession::new(
                // The listener's ceiling and `account=` reach `resolve`
                // through the handshake (`account-spec.md` §5).
                Handshake::new(
                    handshake::Config {
                        listener: listener.clone(),
                        peer: Some(peer.ip()),
                        ..config.handshake.clone()
                    },
                    challenge(id, peer),
                ),
                session.clone(),
                Arc::clone(world),
            )
            .with_writers(Arc::clone(writers))
            .with_gate(gate.clone()),
        ),
        (None, Mode::Capture) => Box::<CaptureOnly>::default(),
    };

    // The write half, cloned so the reader can stay blocked in `read` for the
    // four minutes at a time the client is entitled to be silent for. **The
    // single strong handle lives in this frame and nowhere else**, which is the
    // whole lifetime design: the registry holds `Weak`, so a connection that
    // ended cannot be written to and nothing has to be told that it ended.
    let write_half = match stream.try_clone() {
        Ok(h) => h,
        Err(e) => {
            // Without a second handle there is no push path for this
            // connection, and a connection we cannot push to cannot be given a
            // ship. Refusing is clearer than serving half a session.
            eprintln!("[conn {id}] cannot clone the socket: {e}");
            return ConnEnd::Io(e);
        }
    };
    // **The write timeout goes on this handle too.** `try_clone` dups the
    // descriptor, and on every platform we target the send timeout is a
    // property of the socket rather than of the descriptor -- so setting it in
    // `conn::serve` would already cover these writes. Setting it here as well
    // costs one syscall and means the push path does not depend on that
    // sharing being true: this handle is the one the ticker and
    // `deliver_elsewhere` write through, and `conn::WRITE_TIMEOUT` explains
    // what an unbounded write here freezes.
    let _ = write_half.set_write_timeout(Some(crate::conn::WRITE_TIMEOUT));
    let wire = Arc::new(Mutex::new(Wire::new(write_half, capture)));
    writers.register(session.client_id, &wire);

    // Register this connection with the world, so the other players can see
    // it. Taken **once, here, outside any payload handling** -- a dispatch
    // arm already holds this lock and `RwLock` is not reentrant -- and with
    // nothing fallible inside the critical section, because a panic while
    // holding it poisons the world for everyone (`docs/reference/world-state.md` §4).
    //
    // **This used to skip its work on a poisoned lock, and that was a worse
    // trade than the comment it carried claimed.** The old text read "the
    // client can still handshake and will simply not appear in anyone's
    // lobby, which is a smaller failure than a refused login" -- priced
    // before the push path existed. It is no longer only the lobby:
    // `of_character` is how *every* addressed notification finds its
    // recipient, so a connection missing from the session table gets no fleet
    // invite, no other pilot's ship, nothing from the world ticker. And a
    // `RwLock`'s poison never clears, so one panic anywhere under this lock
    // did that to every connection for the rest of the process's life.
    // `service::world_write` is now the one policy; see its note.
    {
        let mut w = crate::service::world_write_timed(world, "session_opened");
        w.session_opened(crate::session_table::SessionEntry {
            client_id: session.client_id,
            session_id: session.session_id,
            user_id: session.user_id,
            character_id: None,
        });
        // The address chat tells the client to dial
        // (`XmppChatMgr.GetDeprecatedPrefsFallback`): the listener's
        // `advertise=`, else the one this connection arrived on. World, then
        // hub: the lock order `crate::chat` documents.
        if let (Some(hub), Ok(local)) = (w.chat(), stream.local_addr()) {
            hub.note_client_addr(session.client_id, listener.advertise.unwrap_or(local.ip()));
        }
    }
    // From here on every exit path, a panic included, unregisters: a
    // connection left in the session table is a character the station lobby
    // says is standing there, and `onlineStatus` says is logged in.
    let guard = SessionGuard {
        world,
        client_id: session.client_id,
        wire: &wire,
    };

    if config.push_probe > 0 {
        probe(&wire, config, session.client_id, config.push_probe);
    }

    let end = serve(stream, &wire, dispatch.as_mut());
    drop(guard);

    // A poisoned `Wire` is a panic that happened mid-write: nothing to count.
    let frames = wire.lock().map(|w| w.capture_count()).unwrap_or(0);
    println!("[conn {id}] state: {}", dispatch.state());
    if config.capture_dir.is_some() && listener.capture {
        println!("[conn {id}] captured {frames} frames");
    }
    end
}

/// Queue `n` inert probe notifications, one per second.
///
/// A dev-only first-contact test for the push path, off unless `--push-probe`
/// is passed. See [`PUSH_PROBE_EVENT`] for why a handler-less event is the
/// right probe, and `docs/reference/push-path.md` §2 for why each field of this packet is
/// what it is -- in particular that `oob` must stay empty (a non-zero `sn`
/// parks the client's tasklet in its `Sequencer` forever) and that the source
/// must be a **node** address (a forking source makes the client close the
/// transport with "A request must be sent from a specific address, not a
/// forking one").
fn probe(wire: &Arc<Mutex<Wire>>, config: &Config, client_id: i64, n: u32) {
    let packet = MachoPacket::notification(
        MachoAddress::node(config.handshake.proxy_node_id),
        PUSH_PROBE_EVENT,
        // The `*` is literal: `ScattercastWithoutTheStars` prepends it itself,
        // so the string that goes on the wire already carries it.
        "*clientID",
        // The connection's id, not the account's. Cosmetic for command 12 --
        // `_AssociateWithSession` takes the `macho.mode == 'client'` branch
        // without reading the packet (`docs/reference/push-path.md:775-787`) -- but it is
        // the field a two-player routing bug would hide in, so send the truth.
        vec![PyValue::Int(client_id)],
        Vec::new(),
    );
    let payload = match packet.to_payload() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[push] probe packet would not encode: {e}");
            return;
        }
    };
    let Ok(mut w) = wire.lock() else { return };
    let now = Instant::now();
    for i in 1..=u64::from(n) {
        w.post(now + Duration::from_secs(i), payload.clone());
    }
}

/// Name a capture file so that no two runs can collide.
///
/// **The pid is not decoration.** `id` counts connections *within one
/// process*, so two server runs started in the same second both produced
/// `<secs>-0000.evecap` -- and `Capture::create` deliberately refuses to
/// overwrite, so the second run failed to stderr and that session was simply
/// not recorded. Captures are the only artefact here that cannot be
/// regenerated: a recording of a failure is worth more than the memory of it,
/// which is why four of them are committed under `captures/regression/`.
fn open_capture(dir: &Path, id: u64) -> io::Result<Capture> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let pid = std::process::id();
    Capture::create(&dir.join(format!("{stamp}-{pid}-{id:04}.evecap")))
}

/// A 64-byte server challenge.
///
/// **Not cryptographic, and does not need to be.** Under the Placebo crypto
/// pack the client's only use for this value is to hand back
/// `str(crc_hqx(marshal.Save((challenge,)), 0))` — a 16-bit checksum. A
/// `getrandom` dependency to feed a 16-bit check would be a dependency bought
/// for nothing, and this workspace has none. What the value must be is
/// *different per connection*, so that two sessions are distinguishable in a
/// capture, and that is what the clock and the connection id give it.
///
/// SplitMix64, from Steele, Lea & Flood 2014 — chosen because it is eleven
/// lines and needs no state beyond a `u64`.
fn challenge(id: u64, peer: SocketAddr) -> [u8; CHALLENGE_LEN] {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut state = nanos ^ (id << 32) ^ u64::from(peer.port());
    let mut out = [0u8; CHALLENGE_LEN];
    for block in out.chunks_mut(8) {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        block.copy_from_slice(&z.to_le_bytes()[..block.len()]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    /// The node id the handshake advertises and the one the services hand out
    /// for bound objects must be the same number.
    ///
    /// A bound-object id is `'N=<nodeID>:<n>'`, and
    /// `MachoObjectConnection.__init__` parses that node back out and
    /// addresses every later call on the object to it. If it named a node we
    /// do not answer as, those calls would go nowhere and the client would
    /// block -- a hang with no missing method behind it, which is a
    /// particularly unpleasant thing to debug.
    #[test]
    fn the_advertised_node_id_and_the_bound_object_node_id_agree() {
        let config = Config::new(peer(0));
        assert_eq!(
            config.sandbox.node_id, config.handshake.proxy_node_id,
            "a bound object would name a node we do not answer as"
        );
    }

    /// The same agreement, but for a handshake that is **not** the default.
    ///
    /// The test above passed throughout the whole period in which
    /// `--proxy-node` and `--user-id` were broken, because it exercises
    /// `Config::new` -- which derives from the default handshake -- and the
    /// binary assigned `config.handshake` afterwards. A test that only ever
    /// sees the default cannot see a desync that only a non-default value
    /// causes. So: pass one.
    #[test]
    fn a_non_default_handshake_still_agrees_everywhere() {
        let hs = crate::handshake::Config {
            proxy_node_id: 5,
            user_id: 7,
            ..crate::handshake::Config::default()
        };
        let config = Config::with_handshake(peer(0), hs);

        assert_eq!(
            config.sandbox.node_id, 5,
            "MachoResolveObject would answer a different node than the \
             handshake advertised, and the client would hang on the first \
             bound-object call"
        );
        // All three user ids -- three copies of one *account* id.
        assert_eq!(config.handshake.user_id, 7);
        assert_eq!(config.sandbox.user_id, 7, "GetCharacterSelectionData");
        assert_eq!(config.session.user_id, 7, "initialstate");
    }

    /// The other half of the test above: the account id reaches those three
    /// places and the connection address reaches **none** of them.
    ///
    /// `MachoAddress::client(session.user_id)` was the destination of every
    /// notification we pushed, which is how an account id came to be used as a
    /// connection address. A `client_id` derived from `handshake.user_id`
    /// would be the same bug with a new field name, so assert it is not.
    #[test]
    fn the_client_id_is_not_the_account_id() {
        let hs = crate::handshake::Config {
            user_id: 7,
            ..crate::handshake::Config::default()
        };
        let config = Config::with_handshake(peer(0), hs);
        for ordinal in 0..4u64 {
            assert_ne!(
                client_id(ordinal, config.handshake.proxy_node_id),
                config.handshake.user_id,
                "the connection address is the account id again"
            );
        }
    }

    /// Two connections of one account are two addresses.
    ///
    /// The formula is CCP's (`machoNet.GetIDOfAddress`), and the property that
    /// matters is only that it is injective in the ordinal -- whatever is
    /// keyed by client id would otherwise keep just the later of the two.
    #[test]
    fn two_connections_of_one_account_get_two_client_ids() {
        let node = 1_000_003_120;
        assert_eq!(client_id(0, node), 10_000_000_000 + node);
        assert_eq!(client_id(1, node), 20_000_000_000 + node);
        assert_ne!(client_id(0, node), client_id(1, node));
        // CCP's own recorded value, from the comment on `client_id`: the
        // 6173rd client of node 1000003120.
        assert_eq!(client_id(6172, node), 61_731_000_003_120);
    }

    fn peer(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    #[test]
    fn challenge_is_the_length_the_client_requires() {
        assert_eq!(challenge(0, peer(1)).len(), CHALLENGE_LEN);
        assert_eq!(CHALLENGE_LEN, 64);
    }

    #[test]
    fn challenge_differs_between_connections() {
        let a = challenge(0, peer(1));
        let b = challenge(1, peer(2));
        assert_ne!(a, b);
        // And is not degenerate: all 64 bytes must not be one repeated value.
        assert!(a.iter().any(|&x| x != a[0]), "challenge is a constant run");
    }

    #[test]
    fn binding_port_zero_reports_the_real_port() {
        let server = Server::bind(Config::new(peer(0))).unwrap();
        assert_ne!(server.local_addr().unwrap().port(), 0);
        assert_eq!(server.accepted(), 0);
    }

    /// The universe lives in the registry, once, and not in the config every
    /// connection reads: a copy per connection cost ~300 MB a pilot with the
    /// full extracts and ran a fifty-bot soak out of memory (§67).
    #[test]
    fn the_loaded_universe_moves_into_the_registry_not_the_shared_config() {
        let mut config = Config::new(peer(0));
        // Two listeners: neither may hold a copy.
        config.listeners.push(Listener::new(peer(0)));
        config.sandbox.celestials = crate::celestial::Catalogue::parse(
            "# solarSystemID\t39000001\n\
             #kind\tid\ttype_id\tradius\tx\ty\tz\n\
             star\t49000001\t1\t1000.0\t0.0\t0.0\t0.0\n\
             belt\t49000002\t2\t0.0\t5000.0\t0.0\t0.0\n",
        );
        assert!(!config.sandbox.celestials.is_empty(), "fixture parsed");
        let server = Server::bind(config).unwrap();
        assert_eq!(server.config.sandbox.celestials.len(), 0);
        let world = server.world.read().unwrap();
        assert!(!world.celestials().is_empty());
    }

    /// Wait for `cond` (accepts happen on other threads), or fail.
    fn eventually(what: &str, cond: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !cond() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn two_loopback_listeners() -> Config {
        let mut config = Config::new(peer(0));
        config.listeners.push(Listener::new(peer(0)));
        config
    }

    /// `--bind` is repeatable and every listener serves (V6 F8): each
    /// connection gets the server-initiated greeting.
    #[test]
    fn two_bind_addresses_both_accept() {
        let server = Arc::new(Server::bind(two_loopback_listeners()).unwrap());
        let addrs = server.local_addrs().unwrap();
        assert_eq!(addrs.len(), 2);
        assert_ne!(addrs[0], addrs[1]);
        let runner = Arc::clone(&server);
        std::thread::spawn(move || runner.run());
        let mut streams = Vec::new();
        for addr in &addrs {
            let mut s = TcpStream::connect(addr).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut buf = [0u8; 4];
            io::Read::read_exact(&mut s, &mut buf).expect("greeting from every listener");
            streams.push(s);
        }
        eventually("both listeners to count one", || server.live() == [1, 1]);
    }

    /// Chat listens beside every listener (`plan-review.md` H2): one at the
    /// listener's `chat=`, one on the chat port of a listener naming none.
    #[test]
    fn chat_listens_on_every_bind() {
        let mut config = two_loopback_listeners();
        config.listeners[1].chat = Some(peer(0));
        config.chat = Some(crate::chat::ChatConfig {
            port: 0,
            advertise: None,
        });
        let server = Server::bind(config).unwrap();
        let chat = server.chat_addrs();
        assert_eq!(chat.len(), 2, "one chat listener per machoNet listener");
        let _streams: Vec<TcpStream> = chat
            .iter()
            .map(|a| TcpStream::connect(a).expect("chat accepts"))
            .collect();
        eventually("each chat listener to count one", || {
            server.chat_live() == [1, 1]
        });
    }

    /// **Chat on port 0 binds race-free** (tracker §83): many servers with
    /// chat, bound at once from parallel threads, each get a distinct chat
    /// port the kernel picked, and none sees `AddrInUse`. Tests read the
    /// port back from `chat_addrs` and never probe one and hand it over.
    #[test]
    fn chat_on_port_zero_binds_in_parallel_without_addr_in_use() {
        const SERVERS: usize = 16;
        let barrier = Arc::new(std::sync::Barrier::new(SERVERS));
        let threads: Vec<_> = (0..SERVERS)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let mut config = Config::new(peer(0));
                    config.chat = Some(crate::chat::ChatConfig {
                        port: 0,
                        advertise: None,
                    });
                    barrier.wait();
                    let server = Server::bind(config).expect("binds without AddrInUse");
                    let chat = server.chat_addrs();
                    assert_eq!(chat.len(), 1);
                    assert_ne!(chat[0].port(), 0, "the bound port, not the asked one");
                    TcpStream::connect(chat[0]).expect("chat accepts");
                    (server, chat[0].port())
                })
            })
            .collect();
        let bound: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        let mut ports: Vec<u16> = bound.iter().map(|(_, p)| *p).collect();
        ports.sort_unstable();
        ports.dedup();
        assert_eq!(ports.len(), SERVERS, "every server its own chat port");
    }

    /// A `chat=` alone turns chat on, for its own listener only: a listener
    /// cannot name a chat address that nothing serves, and naming one does
    /// not open chat beside the others.
    #[test]
    fn a_chat_address_turns_chat_on_beside_its_listener() {
        let mut config = two_loopback_listeners();
        config.listeners[1].chat = Some(peer(0));
        let server = Server::bind(config).unwrap();
        assert_eq!(server.chat_addrs().len(), 1);
    }

    /// Fail closed until accounts exist (`plan-review.md` C2, H3): a public
    /// address, a public chat address, `ceiling=player` and an `account=`
    /// naming no account are each refused before anything binds; loopback
    /// and the jail host are not.
    #[test]
    fn a_public_bind_is_refused_without_accounts() {
        let public: SocketAddr = "0.0.0.0:0".parse().unwrap();
        let user = Config::new(peer(0)).handshake.user_id;
        let refused = |l: Listener| {
            let mut config = Config::new(peer(0));
            config.listeners = vec![l];
            match Server::bind(config) {
                Ok(_) => panic!("bound a listener that must be refused"),
                Err(e) => assert_eq!(e.kind(), io::ErrorKind::PermissionDenied, "{e}"),
            }
        };
        refused(Listener::new(public));
        refused(Listener::parse("192.0.2.1:0").unwrap());
        refused(Listener {
            chat: Some(public),
            ..Listener::new(peer(0))
        });
        refused(Listener {
            ceiling: Ceiling::Player,
            ..Listener::new(peer(0))
        });
        refused(Listener {
            account: Some(user + 1),
            ..Listener::new(peer(0))
        });

        let hs = crate::handshake::Config::default();
        assert!(fail_closed(&Listener::new(peer(0)), &hs).is_ok());
        assert!(fail_closed(&Listener::new("[::1]:0".parse().unwrap()), &hs).is_ok());
        let jail = Listener::new(SocketAddr::new(JAIL_HOST, 26000));
        assert!(fail_closed(&jail, &hs).is_ok(), "the jail host is local");
        let mine = Listener {
            account: Some(user),
            ..Listener::new(peer(0))
        };
        assert!(fail_closed(&mine, &hs).is_ok());
        assert!(Server::bind(Config::new(peer(0))).is_ok());
    }

    #[test]
    fn signup_needs_world_accounts_and_excludes_account() {
        let l = Listener::parse("127.0.0.1:1,signup=on").unwrap();
        assert!(l.signup);
        assert!(!Listener::parse("127.0.0.1:1").unwrap().signup);
        assert!(Listener::parse("127.0.0.1:1,signup=yes").is_err());
        let l = Listener::parse("127.0.0.1:1,signup=on,botnames=on").unwrap();
        assert!(l.bot_names && !Listener::parse("127.0.0.1:1").unwrap().bot_names);
        assert!(Listener::parse("127.0.0.1:1,botnames=maybe").is_err());

        let hs = handshake::Config::default();
        assert!(
            fail_closed(&l, &hs)
                .unwrap_err()
                .contains("--world-accounts")
        );
        let world = handshake::Config {
            accounts: Some(Arc::new(crate::accounts::Accounts::in_world(
                crate::account_book::Book::with_iterations(1),
            ))),
            ..handshake::Config::default()
        };
        assert!(fail_closed(&l, &world).is_ok());
        let named = Listener::parse("127.0.0.1:1,account=7").unwrap();
        assert!(fail_closed(&named, &world).is_err());
    }

    #[test]
    fn capture_is_on_unless_a_listener_says_off() {
        assert!(Listener::parse("127.0.0.1:1").unwrap().capture);
        assert!(!Listener::parse("127.0.0.1:1,capture=off").unwrap().capture);
        assert!(Listener::parse("127.0.0.1:1,capture=no").is_err());
    }

    #[test]
    fn the_bind_grammar_parses_every_key() {
        let l = Listener::parse(
            "127.0.0.1:26001,ceiling=player,cap=3,account=7,chat=127.0.0.1:5223,advertise=10.137.0.1",
        )
        .unwrap();
        assert_eq!(l.addr, "127.0.0.1:26001".parse().unwrap());
        assert_eq!(l.ceiling, Ceiling::Player);
        assert_eq!(l.cap(), 3);
        assert_eq!(l.account, Some(7));
        assert_eq!(l.chat, Some("127.0.0.1:5223".parse().unwrap()));
        assert_eq!(l.advertise, Some(JAIL_HOST));

        let plain = Listener::parse("127.0.0.1:26000").unwrap();
        assert_eq!(plain, Listener::new("127.0.0.1:26000".parse().unwrap()));
        assert_eq!(plain.cap(), LOCAL_CAP);
        assert_eq!(Listener::parse("192.0.2.1:1").unwrap().cap(), REMOTE_CAP);

        for bad in [
            "nonsense",
            "127.0.0.1:1,cap=0",
            "127.0.0.1:1,cap=1,cap=2",
            "127.0.0.1:1,ceiling=root",
            "127.0.0.1:1,cap",
            "127.0.0.1:1,port=2",
        ] {
            assert!(Listener::parse(bad).is_err(), "{bad} parsed");
        }
    }

    // --- accounts (IP27-a) -------------------------------------------------

    use crate::accounts::{Accounts, Role, hex, sha256};
    use crate::pyutil::{dict_get, pydict};
    use eve_marshal::handshake as hs;
    use std::io::{Read, Write};

    /// An invented token; never a real one.
    const TOKEN: &str = "ip27a-invented-token-not-a-name";
    const UID: i64 = 2001;

    fn accounts(role: &str) -> Arc<Accounts> {
        let line = format!(
            "{}\t{UID}\t{role}\tinvented\n",
            hex(&sha256(TOKEN.as_bytes()))
        );
        Arc::new(Accounts::parse(&line).unwrap())
    }

    /// The binary's path: the accounts go in through the handshake, the
    /// handshake through `with_handshake`.
    fn accounts_config(role: &str, ceiling: Ceiling) -> Config {
        let hs = crate::handshake::Config {
            accounts: Some(accounts(role)),
            ..crate::handshake::Config::default()
        };
        let mut config = Config::with_handshake(peer(0), hs);
        config.listeners[0].ceiling = ceiling;
        config
    }

    /// The client half of the wire, just enough to log in and make a call.
    struct Client {
        stream: TcpStream,
        buf: Vec<u8>,
    }

    impl Client {
        fn connect(addr: SocketAddr) -> Self {
            let stream = TcpStream::connect(addr).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            Client {
                stream,
                buf: Vec::new(),
            }
        }

        fn send(&mut self, v: &PyValue) {
            let mut framed = Vec::new();
            eve_proto::write_frame(&crate::wire::dumps(v), &mut framed).unwrap();
            self.stream.write_all(&framed).unwrap();
        }

        /// The next payload, or `None` once the server has closed.
        fn recv(&mut self) -> Option<PyValue> {
            let mut chunk = [0u8; 4096];
            loop {
                if let Some((frame, used)) = eve_proto::try_split_frame(&self.buf).unwrap() {
                    let v = crate::wire::loads(frame.payload).unwrap();
                    self.buf.drain(..used);
                    return Some(v);
                }
                match self.stream.read(&mut chunk) {
                    Ok(0) | Err(_) => return None,
                    Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                }
            }
        }

        /// The next instance whose class ends in `.{short}`.
        fn recv_instance(&mut self, short: &str) -> PyValue {
            loop {
                let v = self.recv().expect("server closed");
                if let PyValue::Instance { class, .. } = &v
                    && String::from_utf8_lossy(class).ends_with(&format!(".{short}"))
                {
                    return v;
                }
            }
        }
    }

    fn bstr(s: &str) -> PyValue {
        PyValue::Bytes(s.as_bytes().to_vec())
    }

    /// H1 to H6 with `token`; H7 and H8 if the server answers. Returns H8's
    /// server challenge, or `None` when the server closed instead.
    fn up_to_h8(c: &mut Client, token: &str) -> Option<Vec<u8>> {
        c.recv().expect("H1");
        c.send(&PyValue::Tuple(vec![
            PyValue::Int(i64::from(hs::SECRET_KEY)),
            PyValue::Int(i64::from(hs::MACHO_VERSION)),
            PyValue::Int(0),
            PyValue::Real(hs::BOOT_VERSION),
            PyValue::Int(i64::from(hs::BOOT_BUILD)),
            bstr(hs::CODENAME_AT_REGION),
        ]));
        c.send(&PyValue::Tuple(vec![PyValue::None, bstr("VK"), bstr("x")]));
        c.send(&PyValue::Tuple(vec![
            bstr("placebo"),
            PyValue::Dict(vec![]),
        ]));
        c.recv().expect("H5");
        c.send(&PyValue::Tuple(vec![
            PyValue::Bytes(vec![0x5a; CHALLENGE_LEN]),
            pydict(vec![
                ("user_name", bstr("EVE-SSO-CONNECTION")),
                ("user_sso_token", bstr(token)),
            ]),
        ]));
        c.recv()?; // H7
        let PyValue::Tuple(h8) = c.recv()? else {
            panic!("H8 is not a tuple")
        };
        match &h8[0] {
            PyValue::Bytes(b) => Some(b.clone()),
            other => panic!("H8[0] is {other:?}"),
        }
    }

    /// Log in with `token` and return the three copies of the user id
    /// (H10, `initialstate`, the select screen) and the session's role.
    fn login_and_read(config: Config) -> (i64, i64, i64, i64) {
        let server = Server::bind(config).unwrap();
        let addr = server.local_addr().unwrap();
        let handle = std::thread::spawn(move || server.run_once().unwrap());
        let mut c = Client::connect(addr);
        let challenge = up_to_h8(&mut c, TOKEN).expect("the token was refused");
        c.send(&PyValue::Tuple(vec![
            PyValue::Bytes(crate::wire::crypto_hash(&[PyValue::Bytes(challenge)]).into_bytes()),
            bstr(""),
            PyValue::Dict(vec![]),
        ]));
        let h10 = c.recv().expect("H10");
        let int = |v: Option<&PyValue>| match v {
            Some(PyValue::Int(n)) => *n,
            other => panic!("expected an int, got {other:?}"),
        };
        let h10_uid = int(dict_get(
            dict_get(&h10, b"session_init").unwrap(),
            b"userid",
        ));

        let PyValue::Instance { state, .. } = c.recv_instance("SessionInitialStateNotification")
        else {
            unreachable!()
        };
        let PyValue::Tuple(t) = &*state else {
            panic!("packet state")
        };
        let PyValue::Tuple(body) = &t[4] else {
            panic!("packet body")
        };
        let (init_uid, role) = (
            int(dict_get(&body[2], b"userid")),
            int(dict_get(&body[2], b"role")),
        );

        let call = eve_proto::CallPayload::Service {
            method: "GetCharacterSelectionData".to_owned(),
            args: vec![],
            kwargs: vec![],
        };
        let packet = eve_proto::MachoPacket::new(
            eve_marshal::MachoNetMsg::CallReq,
            eve_proto::MachoAddress::Client {
                client_id: 1,
                call_id: Some(3),
                service: None,
            },
            eve_proto::MachoAddress::Any {
                service: Some("charUnboundMgr".to_owned()),
                call_id: Some(3),
            },
            vec![call.to_payload()],
        );
        c.send(&packet.to_value().unwrap());
        let PyValue::Instance { state, .. } = c.recv_instance("CallRsp") else {
            unreachable!()
        };
        let PyValue::Tuple(t) = &*state else {
            panic!("packet state")
        };
        let PyValue::Tuple(body) = &t[4] else {
            panic!("packet body")
        };
        let PyValue::Bytes(pickle) = &body[0] else {
            panic!("CallRsp payload")
        };
        let PyValue::Tuple(four) = crate::wire::loads(pickle).unwrap() else {
            panic!("selection data")
        };
        let PyValue::List(user) = &four[0] else {
            panic!("userDetails")
        };
        let PyValue::Instance { state, .. } = &user[0] else {
            panic!("userDetails[0]")
        };
        let sel_uid = int(dict_get(state, b"userID"));
        c.stream.shutdown(std::net::Shutdown::Both).unwrap();
        handle.join().unwrap();
        (h10_uid, init_uid, sel_uid, role)
    }

    /// Ruling 2: a leaked GM token on a friends' listener is a player.
    #[test]
    fn a_gm_token_on_a_player_listener_gets_player() {
        let (h10, init, sel, role) = login_and_read(accounts_config("gm", Ceiling::Player));
        assert_eq!((h10, init, sel), (UID, UID, UID));
        assert_eq!(role, Role::Player.mask());
        assert_eq!(role & crate::roles::ROLE_GML, 0, "a GM bit on a player");
    }

    /// `a_non_default_handshake_still_agrees_everywhere`, through the
    /// binary's path and with accounts: one connection's three user ids
    /// are the one resolved id, not the catch-all's `user_id`.
    #[test]
    fn with_accounts_the_three_user_ids_are_the_resolved_one() {
        let config = accounts_config("gm", Ceiling::Gm);
        assert_ne!(config.handshake.user_id, UID);
        let (h10, init, sel, role) = login_and_read(config);
        assert_eq!((h10, init, sel), (UID, UID, UID));
        assert_eq!(role, Role::Gm.mask());
    }

    #[test]
    fn an_unknown_token_closes_the_connection() {
        let server = Server::bind(accounts_config("gm", Ceiling::Gm)).unwrap();
        let addr = server.local_addr().unwrap();
        let handle = std::thread::spawn(move || server.run_once().unwrap());
        let mut c = Client::connect(addr);
        assert!(
            up_to_h8(&mut c, "token").is_none(),
            "an unknown token got H7/H8"
        );
        handle.join().unwrap();
    }

    /// With `--accounts` a public or `ceiling=player` listener binds; an
    /// `account=` must name a line; a token equal to a character name
    /// refuses startup.
    #[test]
    fn accounts_relax_fail_closed_and_check_names_at_bind() {
        let hs = accounts_config("gm", Ceiling::Gm).handshake;
        let public: SocketAddr = "0.0.0.0:0".parse().unwrap();
        assert!(fail_closed(&Listener::new(public), &hs).is_ok());
        let friends = Listener {
            ceiling: Ceiling::Player,
            ..Listener::new(peer(0))
        };
        assert!(fail_closed(&friends, &hs).is_ok());
        let named = |uid| Listener {
            account: Some(uid),
            ..Listener::new(peer(0))
        };
        assert!(fail_closed(&named(UID), &hs).is_ok());
        assert!(fail_closed(&named(UID + 1), &hs).is_err());
        assert!(Server::bind(accounts_config("player", Ceiling::Player)).is_ok());

        let mut config = Config::new(peer(0));
        let name = config.sandbox.characters[0].name.clone();
        let line = format!(
            "{}\t{UID}\tplayer\tinvented\n",
            hex(&sha256(name.as_bytes()))
        );
        config.handshake.accounts = Some(Arc::new(Accounts::parse(&line).unwrap()));
        match Server::bind(config) {
            Ok(_) => panic!("a token equal to a character name was accepted"),
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::PermissionDenied, "{e}"),
        }
    }

    #[test]
    fn the_fd_limit_is_read_from_the_limits_file_and_checked_against_the_cap() {
        let text = "Limit                     Soft Limit           Hard Limit           Units\n\
                    Max open files            1024                 524288               files\n\
                    Max locked memory         8388608              8388608              bytes\n";
        assert_eq!(super::parse_fd_limit(text), Some(1024));
        assert_eq!(super::parse_fd_limit("Max open files unlimited x"), None);
        assert_eq!(super::parse_fd_limit("nothing"), None);
        assert!(super::fd_warning(Some(1024), LOCAL_CAP).is_some());
        assert!(super::fd_warning(Some(1024), 512).is_none());
        assert!(super::fd_warning(Some(1023), 512).is_some());
        assert!(super::fd_warning(None, LOCAL_CAP).is_none());
        assert_eq!(super::conn_stack_bytes(None), super::CONN_STACK);
        assert_eq!(super::conn_stack_bytes(Some(3)), 3 << 20);
    }
}
