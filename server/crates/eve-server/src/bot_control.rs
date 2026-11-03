//! **Runtime bot control** (lane `bot-ctl`): the `/bots` GM command and the
//! local link it speaks over to `eve-bots`.
//!
//! A GM in a dev run types `/bots spawn ...`; the server cannot start bots
//! (they are clients, in another process), so it relays the command to the
//! connected `eve-bots` over a loopback TCP link and shows its short answer
//! to the caller. `docs/lanes/bot-ctl.md` has the whole picture.
//!
//! # The link
//!
//! [`Link::open`] binds `127.0.0.1:0` (loopback only, never another
//! interface), writes the port to `<dir>/bot-control.addr` and a random
//! token (32 bytes of `/dev/urandom`, hex) to `<dir>/bot-control.token`
//! (mode 0600). Lines of UTF-8, `\n` ended:
//!
//! - peer -> server, first: `HELLO <token>`; the server answers `OK`, or `NO`
//!   and hangs up. A newer peer replaces an older one.
//! - server -> peer: `CMD <id> <command words>`; peer -> server:
//!   `REPLY <id> <text>`, with a newline in the text written as `\n` (a
//!   backslash and an n). A reply for an older id is dropped.
//!
//! One command is in flight at a time and the caller waits for the answer
//! at most [`REPLY_WAIT`]: the slash runs under the world's lock, so the
//! peer must answer from state it keeps outside the bot threads' hot locks.
//!
//! # The gates
//!
//! `/bots` is refused unless the caller is a GM (the slash service refuses
//! a non-GM session first; this checks again, so a direct call cannot get
//! round it) **and** the server runs in dev mode ([`BotControl::dev`], set
//! from `Config::dev`). **Seam:** `Config::dev` is this lane's own bool,
//! default false; DEV-MODE's `--dev` sets it at landing.
//!
//! No dependency: std sockets and threads only.

use crate::pyutil::bstr;
use crate::service::{Caller, Registry, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The file holding the link's loopback address, in the control directory.
pub const ADDR_FILE: &str = "bot-control.addr";
/// The file holding the link's token, in the control directory.
pub const TOKEN_FILE: &str = "bot-control.token";
/// The file the clock rewrites every health window in the control directory:
/// where the humans are and the tick p95, for `eve-bots --bubble`
/// ([`BubbleState`]).
pub const BUBBLE_FILE: &str = "bubble.tsv";
/// How long a slash waits for the peer's answer.
pub const REPLY_WAIT: Duration = Duration::from_secs(4);
/// How long a new peer has to say `HELLO`.
const HELLO_WAIT: Duration = Duration::from_secs(2);
/// The most bots one `/bots spawn` may ask for (a typo guard; raised from 200
/// for the owner's scale runs, 2026-10-28).
pub const MAX_COUNT: u32 = 1000;

const USAGE: &str = "Usage: /bots spawn <group> <side> <role> <count> <skills> <ship> [system] | \
/bots list | /bots despawn <group|all> | /bots bring <group> | /bots come [group] | /bots side <group> <side>. \
Example: /bots spawn gang1 RED mine 3 role Hulk 30000042. \
Or in words: /bots 10 allied miners, /bots 5 hostile roamers, /bots help";

/// The connected `eve-bots`, if any.
struct Peer {
    writer: TcpStream,
    reader: BufReader<TcpStream>,
}

/// The server end of the control link.
pub struct Link {
    token: String,
    addr: SocketAddr,
    /// The control directory, when there is one (BUBBLE's file goes there).
    dir: Option<PathBuf>,
    peer: Mutex<Option<Peer>>,
    next_id: Mutex<u64>,
}

impl Link {
    /// Bind the link and start accepting peers. With `dir`, write the
    /// address and token files there (created if need be).
    pub fn open(dir: Option<&Path>) -> io::Result<Arc<Link>> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
        let addr = listener.local_addr()?;
        let token = hex(&crate::accounts::urandom32()?);
        if let Some(dir) = dir {
            std::fs::create_dir_all(dir)?;
            std::fs::write(dir.join(ADDR_FILE), format!("{addr}\n"))?;
            write_private(&dir.join(TOKEN_FILE), &format!("{token}\n"))?;
        }
        let link = Arc::new(Link {
            token,
            addr,
            dir: dir.map(Path::to_path_buf),
            peer: Mutex::new(None),
            next_id: Mutex::new(1),
        });
        let accept = Arc::clone(&link);
        std::thread::Builder::new()
            .name("bot-control".to_owned())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    accept.greet(stream);
                }
            })?;
        Ok(link)
    }

    /// Where the link listens.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The token a peer must say. For tests; the files are the real path.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Check a new connection's `HELLO` and, if good, make it the peer.
    fn greet(&self, stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(HELLO_WAIT));
        let _ = stream.set_nodelay(true);
        let (Ok(read_half), Ok(mut writer)) = (stream.try_clone(), stream.try_clone()) else {
            return;
        };
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        let said_ok = reader.read_line(&mut line).is_ok()
            && line
                .trim_end()
                .strip_prefix("HELLO ")
                .is_some_and(|t| same(t, &self.token));
        if !said_ok {
            let _ = writer.write_all(b"NO\n");
            return;
        }
        let _ = stream.set_read_timeout(Some(REPLY_WAIT));
        let Ok(mut hello_back) = writer.try_clone() else {
            return;
        };
        // The peer is seated **before** it hears OK, so a command sent the
        // moment it has its answer finds it.
        if let Ok(mut slot) = self.peer.lock() {
            *slot = Some(Peer { writer, reader });
        }
        let _ = hello_back.write_all(b"OK\n");
    }

    /// Send `command` to the peer and wait for its answer. `Err` is a
    /// sentence for the caller: nobody connected, or no answer in time.
    pub fn relay(&self, command: &str) -> Result<String, String> {
        let mut slot = self.peer.lock().map_err(|_| "control link poisoned")?;
        let Some(peer) = slot.as_mut() else {
            return Err("no eve-bots is connected to the control link".to_owned());
        };
        let id = {
            let mut n = self.next_id.lock().map_err(|_| "control link poisoned")?;
            *n += 1;
            *n
        };
        let one_line = command.replace(['\n', '\r'], " ");
        if peer
            .writer
            .write_all(format!("CMD {id} {one_line}\n").as_bytes())
            .is_err()
        {
            *slot = None;
            return Err("eve-bots hung up; start it again".to_owned());
        }
        loop {
            let mut line = String::new();
            match peer.reader.read_line(&mut line) {
                Ok(0) => {
                    *slot = None;
                    return Err("eve-bots hung up; start it again".to_owned());
                }
                Err(_) => return Err("eve-bots did not answer in time".to_owned()),
                Ok(_) => {}
            }
            let Some(rest) = line.trim_end().strip_prefix("REPLY ") else {
                continue;
            };
            let (rid, text) = rest.split_once(' ').unwrap_or((rest, ""));
            if rid.parse::<u64>() == Ok(id) {
                return Ok(text.replace("\\n", "\n"));
            }
        }
    }
}

/// **What the bubble planner reads** (lane `bubble`, `eve_bot::bubble`): one
/// window's snapshot, as the file [`BUBBLE_FILE`]. Text, two sections:
///
/// ```text
/// #bubble<TAB>seq<TAB>unix_s<TAB>tick_p95_ms
/// 12<TAB>1700000000<TAB>3.250
/// #humans<TAB>system<TAB>count
/// 30000001<TAB>2
/// ```
///
/// `seq` counts windows, so a reader can tell a new sample from the one it
/// has; `unix_s` says how old the file is. The p95 is of the clock's whole
/// iteration (`push::TickHealth::work`) over the window's ticks **[A]**:
/// EvEJS sheds on the p95 of its tick duration; ours is the same quantity.
/// Falsified if the p95 stays low while the owner feels lag (then the lock
/// wait is the number, not the work).
#[derive(Debug, Clone, PartialEq)]
pub struct BubbleState {
    pub seq: u64,
    pub unix_s: u64,
    pub p95_ms: f64,
    /// `(system, online humans there)`, ascending; docked or in space.
    pub humans: Vec<(i64, u32)>,
}

impl BubbleState {
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::from("#bubble\tseq\tunix_s\ttick_p95_ms\n");
        let _ = writeln!(out, "{}\t{}\t{:.3}", self.seq, self.unix_s, self.p95_ms);
        out.push_str("#humans\tsystem\tcount\n");
        for (s, n) in &self.humans {
            let _ = writeln!(out, "{s}\t{n}");
        }
        out
    }

    /// Read a file's text back; `None` when it is not this format.
    pub fn parse(text: &str) -> Option<BubbleState> {
        let mut lines = text.lines();
        if !lines.next()?.starts_with("#bubble") {
            return None;
        }
        let mut head = lines.next()?.split('\t');
        let seq = head.next()?.parse().ok()?;
        let unix_s = head.next()?.parse().ok()?;
        let p95_ms: f64 = head.next()?.parse().ok()?;
        if !p95_ms.is_finite() {
            return None;
        }
        let mut humans = Vec::new();
        for l in lines {
            if l.starts_with('#') || l.is_empty() {
                continue;
            }
            let (s, n) = l.split_once('\t')?;
            humans.push((s.parse().ok()?, n.parse().ok()?));
        }
        Some(BubbleState {
            seq,
            unix_s,
            p95_ms,
            humans,
        })
    }
}

/// Where the clock writes the bubble file: the control directory's
/// [`BUBBLE_FILE`], or `None` when the server has no link or no directory.
pub fn bubble_path(reg: &Registry) -> Option<PathBuf> {
    let dir = reg.ext::<BotControl>().link.as_ref()?.dir.clone()?;
    Some(dir.join(BUBBLE_FILE))
}

/// Humans per live system, from the liveness index (taken under the world
/// lock; [`BubbleState`] is written after it).
pub fn bubble_humans(reg: &Registry) -> Vec<(i64, u32)> {
    reg.ext::<crate::liveness::Liveness>()
        .iter()
        .filter(|(_, l)| l.humans > 0)
        .map(|(s, l)| (s, l.humans))
        .collect()
}

/// Write the bubble file (a sibling tmp, then a rename, so a reader never
/// sees half of it). A failure is the caller's one log line.
pub fn write_bubble(path: &Path, state: &BubbleState) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, state.render())?;
    std::fs::rename(&tmp, path)
}

/// The 95th percentile of `samples` (nearest rank); zero for none.
pub fn p95(samples: &[Duration]) -> Duration {
    if samples.is_empty() {
        return Duration::ZERO;
    }
    let mut v = samples.to_vec();
    v.sort();
    v[(v.len() * 95).div_ceil(100).max(1) - 1]
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Equal without stopping at the first different byte.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn write_private(path: &Path, text: &str) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(text.as_bytes())
}

/// The world slot: whether `/bots` is allowed, and the link it relays over.
#[derive(Default)]
pub struct BotControl {
    /// A dev run (`Config::dev`). `/bots` is refused without it.
    pub dev: bool,
    link: Option<Arc<Link>>,
    /// TT-B: set around a wire `slash` call (`allow_deferral`), so `run` may
    /// hand its relay out instead of waiting on the peer under the world lock.
    /// Every other caller (tests, the panel) relays inline.
    defer: bool,
    /// The relay `run` handed out, until `take_deferred` takes it.
    deferred: Option<Deferred>,
    /// A deferred relay parked for the dispatcher to run after the lock.
    post_lock: Option<PostLock>,
    /// GANG-SUMMON: the station a spawn seats its bots in when it can (the
    /// caller's own, when he is docked); read where the spawn seats them.
    seat_pref: Option<i64>,
}

/// A `/bots` relay that has not run yet: the link and two strings. It cannot
/// reach the world (no `Registry`), so it may run after the world lock is
/// dropped (TT-B C1).
pub struct Deferred {
    link: Arc<Link>,
    wire: String,
    head: String,
}

impl Deferred {
    /// The relay, with the answer `run` would have given inline.
    pub fn run(self) -> String {
        match self.link.relay(&self.wire) {
            Ok(text) if self.head.is_empty() => text,
            Ok(text) => format!("{}\n{text}", self.head),
            Err(why) => why,
        }
    }
}

/// A [`Deferred`] plus the slash line it answers, for the dispatcher.
pub struct PostLock {
    pub line: String,
    pub job: Deferred,
}

impl PostLock {
    /// The value the inline path builds for the same text.
    pub fn finish(self) -> Result<PyValue, ServiceError> {
        crate::service::gm::visible_reply(&self.line, bstr(&self.job.run()))
    }
}

/// Let the next `run` on this world defer its relay (the wire path only).
pub fn allow_deferral(reg: &mut Registry, on: bool) {
    reg.ext_mut::<BotControl>().defer = on;
}

/// The relay the last `run` deferred, if any.
pub fn take_deferred(reg: &mut Registry) -> Option<Deferred> {
    reg.ext_mut::<BotControl>().deferred.take()
}

/// Park a deferred relay for the dispatcher.
pub fn park(reg: &mut Registry, p: PostLock) {
    reg.ext_mut::<BotControl>().post_lock = Some(p);
}

/// The parked relay, if the call that just ran left one.
pub fn take_post_lock(reg: &mut Registry) -> Option<PostLock> {
    reg.ext_mut::<BotControl>().post_lock.take()
}

/// The slot, for `world_ext::SLOTS` (coordinator).
pub const SLOT: SlotSpec = SlotSpec {
    name: "bot_control",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(BotControl::default())
}

/// Hand the world its dev flag and its link (`Server::bind`).
pub fn attach(reg: &mut Registry, dev: bool, link: Option<Arc<Link>>) {
    let c = reg.ext_mut::<BotControl>();
    c.dev = dev;
    c.link = link;
}

/// The control directory a server uses: `--bot-control`'s, else the world
/// file's own directory, else none (no link).
pub fn dir_for(flag: Option<PathBuf>, world_file: Option<&Path>) -> Option<PathBuf> {
    flag.or_else(|| {
        world_file.map(|w| {
            w.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
        })
    })
}

fn may(reg: &Registry) -> bool {
    match reg.caller() {
        Caller::Internal => true,
        Caller::Session { role, .. } => role & crate::roles::ROLE_GML != 0,
    }
}

/// `/bots ...` (a `slash::COMMANDS` row, coordinator).
pub fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    Ok(bstr(&run(reg, actor, args)))
}

fn run(reg: &mut Registry, actor: Option<i64>, args: &[&str]) -> String {
    if !may(reg) {
        return "/bots is for GMs".to_owned();
    }
    let ctl = reg.ext::<BotControl>();
    if !ctl.dev {
        return "/bots works only on a dev server (--dev)".to_owned();
    }
    let link = ctl.link.clone();
    // BOT-ADMIN: the owner's words become one explicit command line.
    let (args, mut head, quarry, seat) = match natural(reg, actor, args) {
        Ok(Planned::Show(text)) => return text,
        Ok(Planned::Run {
            args,
            head,
            quarry,
            seat,
        }) => (args, head, quarry, seat),
        Err(why) => return why,
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let args = args.as_slice();
    let mut wire = match wire_for(reg, actor, args) {
        Ok(w) => w,
        Err(why) => return why,
    };
    let Some(link) = link else {
        return "no eve-bots is connected (this server has no control link)".to_owned();
    };
    // BOT-ACCOUNTS: a spawn's bots need accounts, characters and tokens,
    // which only the server can make (the link is loopback; the tokens are
    // in the wire line only, never printed).
    if args.first() == Some(&"spawn") {
        // GANG-SUMMON: the quarry closes the line before the logins.
        if let Some(q) = &quarry {
            wire.push_str(" quarry=");
            wire.push_str(q);
        }
        // FC-1: a wing follows the caller, so the wire names him and his
        // fleet is listed where the bots can apply to it.
        if matches!(args.get(3), Some(&"wing" | &"winglogi"))
            && let Some(fc) = actor
        {
            wire.push_str(&format!(" fc={fc}"));
            let mut note = wing_advert_note(reg, fc);
            // FC-3: entosis needs an alliance; say so before the sov work.
            if reg.character(fc).is_some_and(|c| c.alliance_id.is_none()) {
                note.push_str(
                    "\nyour corp is not in an alliance (Corporation > Alliances > Create): sov work needs one",
                );
            }
            head = if head.is_empty() {
                note
            } else {
                format!("{head}\n{note}")
            };
        }
        reg.ext_mut::<BotControl>().seat_pref = seat;
        let made = provision(reg, args);
        reg.ext_mut::<BotControl>().seat_pref = None;
        match made {
            Ok(tokens) => {
                wire.push(' ');
                wire.push_str(TOKENS_KEY);
                wire.push_str(&tokens);
            }
            Err(why) => return format!("spawn: {why}"),
        }
    }
    let job = Deferred { link, wire, head };
    let ctl = reg.ext_mut::<BotControl>();
    if ctl.defer && ctl.deferred.is_none() {
        ctl.deferred = Some(job);
        return String::new();
    }
    job.run()
}

/// FC-1: make sure the caller's fleet has an advert the wing can apply to,
/// and say what happened. A closed advert he made himself is left alone.
fn wing_advert_note(reg: &mut Registry, fc: i64) -> String {
    use crate::fleet_finder::{INVITE_ALLIANCE, INVITE_CORP, INVITE_PUBLIC, INVITE_PUBLIC_OPEN};
    match crate::fleet_finder::ensure_open_advert(reg, fc) {
        Ok((_, true)) => {
            "your fleet is now listed (corp and alliance, no approval): the wing applies to it"
                .to_owned()
        }
        Ok((_, false)) => {
            let open = crate::fleet_finder::mine(reg, Some(fc)).is_some_and(|a| {
                a.invite_scope
                    & (INVITE_CORP | INVITE_ALLIANCE | INVITE_PUBLIC | INVITE_PUBLIC_OPEN)
                    != 0
                    && !a.join_needs_approval
            });
            if open {
                "your fleet's advert is kept: the wing applies to it".to_owned()
            } else {
                "your fleet's advert is closed or needs approval: invite the wing yourself"
                    .to_owned()
            }
        }
        Err(_) => "you are not the boss of a fleet: form one, then invite the wing (or /bots join)"
            .to_owned(),
    }
}

/// FC-3: roles in the kind table that `eve-bots` has no role for yet
/// (the sov wing's linkers and hub hauler: FC-4/FC-5; the logi is FC-2's `winglogi`).
const NOT_BUILT: &[&str] = &["winglink", "winghaul"];

/// FC-3: the share of a `sovfleet` that is DPS wing [A]. Falsified by an
/// owner who wants more or fewer guns in the sov mix.
const SOV_DPS_PERCENT: u32 = 60;

/// What the owner's words come to.
enum Planned {
    /// Say this; nothing goes to the bots.
    Show(String),
    /// Run this explicit command; `head` is a line said before the answer.
    Run {
        args: Vec<String>,
        head: String,
        /// GANG-SUMMON: `<character>:<system>` a roaming gang hunts or
        /// joins (the wire's `quarry=`).
        quarry: Option<String>,
        /// GANG-SUMMON: the station to seat a spawn's bots in, if it has one.
        seat: Option<i64>,
    },
}

/// Turn the words after `/bots` into an explicit command ([`bot_words`]).
fn natural(reg: &mut Registry, actor: Option<i64>, args: &[&str]) -> Result<Planned, String> {
    use crate::bot_words::{At, Cmd};
    let run = |words: Vec<String>| Planned::Run {
        args: words,
        head: String::new(),
        quarry: None,
        seat: None,
    };
    let own = || args.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
    Ok(match crate::bot_words::parse(args) {
        Cmd::Help => Planned::Show(crate::bot_words::usage()),
        Cmd::Bad(why) => Planned::Show(format!("{why}\n\n{}", crate::bot_words::usage())),
        Cmd::Raw => run(own()),
        Cmd::StopAll => run(vec!["stop-all".to_owned()]),
        Cmd::Pause(n) => run(vec!["pause".to_owned(), n]),
        Cmd::Resume(n) => run(vec!["resume".to_owned(), n]),
        Cmd::Inspect(n) => run(vec!["inspect".to_owned(), n]),
        Cmd::Despawn(n) => run(vec!["despawn".to_owned(), n]),
        Cmd::Bring(n) => run(vec!["bring".to_owned(), n]),
        Cmd::Come(n) => run(vec!["come".to_owned(), n]),
        // FC-3: the wing's words, to the caller's wing or to one group.
        Cmd::Order { target, words } => {
            // Only plain words reach the wire; a name may have more than one.
            let words: Vec<String> = words
                .iter()
                .map(|w| {
                    w.chars()
                        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                        .collect()
                })
                .filter(|w: &String| !w.is_empty())
                .collect();
            let mut line = match target {
                Some(g) => vec!["order-group".to_owned(), g],
                None => vec!["order".to_owned(), "me".to_owned()],
            };
            line.extend(words);
            run(line)
        }
        Cmd::Join => {
            let who = actor.ok_or("no character")?;
            Planned::Run {
                args: vec!["join".to_owned()],
                head: wing_advert_note(reg, who),
                quarry: None,
                seat: None,
            }
        }
        Cmd::Spawn(mut req) => {
            // FC-3: roles the bots do not have yet are said, not spawned; a
            // sov fleet spawns the DPS share of its mix for now.
            let mut deferred = String::new();
            match req.kind.role {
                role if NOT_BUILT.contains(&role) => {
                    return Ok(Planned::Show(format!(
                        "{} are not built yet (plan-fc-follow FC-5/FC-4); `/bots {} wing` gives the DPS",
                        req.kind.label, req.count
                    )));
                }
                "sovfleet" => {
                    let dps = (req.count * SOV_DPS_PERCENT / 100).max(1);
                    deferred = format!(
                        "sovfleet: {dps} of {} are DPS wing now; add `/bots 2 logi` for repairers; linkers and the hub hauler are not built yet",
                        req.count
                    );
                    req.count = dps;
                    if let Some(k) = crate::bot_words::kind_of("wing") {
                        req.kind = k;
                    }
                }
                _ => {}
            }
            let who = actor.filter(|a| reg.character(*a).is_some());
            let here = who.and_then(|a| reg.session_system(a));
            let (system, place) = match &req.at {
                At::Here => (
                    here.ok_or("I do not know where you are; name a system: ... in <system>")?,
                    "your system".to_owned(),
                ),
                At::Near => {
                    let here = here.ok_or("I do not know where you are; name a system")?;
                    let serial = serial_of(reg, req.kind.stem);
                    (
                        crate::bot_panel::nearby_where(reg, here, 3, u64::from(serial), &|s| {
                            crate::bot_panel::has_station(reg, s)
                        }),
                        "a system 1-3 jumps away".to_owned(),
                    )
                }
                At::System(name) => (resolve_system(reg, name)?, format!("system {name}")),
            };
            // A bot is seated at a station of its system; without one the
            // spawn would report success and leave it at its start station.
            if !crate::bot_panel::has_station(reg, system) {
                return Err(format!("system {system} has no station"));
            }
            let side = if req.hostile {
                crate::bot_panel::FOE_SIDE.to_owned()
            } else {
                match who {
                    Some(a) => crate::bot_panel::seat_ally(reg, a),
                    None => crate::bot_panel::ALLY_SIDE.to_owned(),
                }
            };
            let group = match &req.group {
                Some(g) => g.clone(),
                None => fresh_group(reg, req.kind.stem, req.hostile),
            };
            let words = vec![
                "spawn".to_owned(),
                group.clone(),
                side.clone(),
                req.kind.role.to_owned(),
                req.count.to_string(),
                req.kind.skills.to_owned(),
                req.kind.ship_for(req.count).to_owned(),
                system.to_string(),
            ];
            // GANG-SUMMON: a gang that hunts (or joins) the caller knows him
            // and his system; his own station is where its bots undock.
            let caller = who.and_then(|a| reg.character(a));
            let quarry = match (caller, req.kind.role) {
                (Some(c), "roamgang") if req.quarry || req.escort => reg
                    .session_system(c.character_id)
                    .map(|here| format!("{}:{here}", c.character_id)),
                _ => None,
            };
            let seat = caller
                .filter(|c| c.solar_system_id == Some(system))
                .and_then(|c| c.station_id);
            let mut head = format!(
                "{} {} {} (side {side}) in {place}, group {group}:",
                req.count,
                if req.hostile { "hostile" } else { "allied" },
                req.kind.label,
            );
            if !deferred.is_empty() {
                head = format!("{deferred}\n{head}");
            }
            if quarry.is_some() {
                head = format!(
                    "{} {}",
                    head.trim_end_matches(':'),
                    if req.quarry {
                        "(hunting you):"
                    } else {
                        "(coming to you):"
                    }
                );
            }
            Planned::Run {
                args: words,
                head,
                quarry,
                seat,
            }
        }
    })
}

/// The smallest serial whose group `<stem><n>x` has no account and no
/// character yet. The account is the test since NAMES: a bot's character has
/// a pool name, so a character named `Miners1x1` never exists and every spawn
/// came out `Miners1x` ("group already exists").
fn serial_of(reg: &Registry, stem: &str) -> u32 {
    (1u32..)
        .find(|n| {
            let first = format!("{stem}{n}x1");
            crate::account_book::find(reg.deltas(), &first).is_none()
                && !reg
                    .config()
                    .characters
                    .iter()
                    .any(|c| c.name.eq_ignore_ascii_case(&first))
        })
        .unwrap_or(1)
}

/// A fresh readable group name: `Miners3x` (bots `Miners3x1`...); hostile
/// groups start `Red`. Letters and digits only, and the `x` keeps group 3's
/// bot 12 apart from group 31's bot 2.
fn fresh_group(reg: &Registry, stem: &str, hostile: bool) -> String {
    let stem = if hostile {
        format!("Red{stem}")
    } else {
        stem.to_owned()
    };
    format!("{stem}{}x", serial_of(reg, &stem))
}

/// A system by id, exact name, or the one name that starts with the words
/// (case does not matter).
fn resolve_system(reg: &Registry, text: &str) -> Result<i64, String> {
    if let Ok(id) = text.parse::<i64>() {
        return Ok(id);
    }
    let Some(u) = reg.content().try_get::<crate::celestial::Universe>() else {
        return Err(format!(
            "this server has no system names; give a system id instead of '{text}'"
        ));
    };
    let want = text.to_ascii_lowercase();
    if let Some(s) = u.systems().find(|s| s.name.eq_ignore_ascii_case(&want)) {
        return Ok(s.id);
    }
    let starts: Vec<_> = u
        .systems()
        .filter(|s| s.name.to_ascii_lowercase().starts_with(&want))
        .collect();
    match starts.as_slice() {
        [one] => Ok(one.id),
        [] => Err(format!("no system named '{text}'")),
        many => Err(format!(
            "'{text}' could be {} or {} others; type more of the name",
            many[0].name,
            many.len() - 1
        )),
    }
}

/// The word before a spawn's logins in the wire line: `tokens=NAME:PW,...`.
/// The peer reads it from the end of the line (`eve_bot::control`).
pub const TOKENS_KEY: &str = "tokens=";

/// The corporation ticker of a side (`eve_bot::dev::ticker_of`: its first
/// five characters, upper case; a unit test in `eve-bot` keeps the two equal).
fn ticker_of(side: &str) -> String {
    side.chars()
        .take(5)
        .collect::<String>()
        .to_ascii_uppercase()
}

/// **BOT-ACCOUNTS.** For `spawn <group> <side> <role> <count> <skills> <ship>
/// [system]`: make (or reuse) the bot accounts and characters `<group>1..N`
/// (`eve_bot::dev::expand`'s names), each marked bot, in the side's dev
/// corporation and seated in `system`'s station. Returns the logins
/// `NAME:PASSWORD,...`, a fresh random password per bot. Secret: the caller
/// puts it on the loopback link and nowhere else.
fn provision(reg: &mut Registry, args: &[&str]) -> Result<String, String> {
    // One read of `/dev/urandom` for the whole spawn (TT-B C2): per bot, a
    // password's 16 bytes and the account's salt.
    let count = args
        .get(4)
        .and_then(|c| c.parse::<usize>().ok())
        .unwrap_or(0)
        .min(MAX_COUNT as usize);
    let mut bytes = vec![0u8; count * BOT_BYTES];
    crate::accounts::urandom_fill(&mut bytes).map_err(|e| e.to_string())?;
    provision_with(reg, args, &bytes)
}

/// Random bytes one bot takes: 16 of password, then the account's salt.
const BOT_BYTES: usize = 16 + crate::account_book::SALT_LEN;

/// [`provision`] over given random bytes (`count * BOT_BYTES` of them).
pub(crate) fn provision_with(
    reg: &mut Registry,
    args: &[&str],
    bytes: &[u8],
) -> Result<String, String> {
    let [
        "spawn",
        group,
        side,
        _role,
        count,
        _skills,
        _ship,
        rest @ ..,
    ] = args
    else {
        return Err(USAGE.to_owned());
    };
    // `eve_bot::dev::parse_roster`'s rule, so names agree with `expand`.
    let plain = |w: &str| !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric());
    if !plain(group) || !plain(side) {
        return Err("group and side are letters and digits only".to_owned());
    }
    let count: u32 = count.parse().map_err(|_| "count is a number".to_owned())?;
    let system = rest.first().and_then(|s| s.parse::<i64>().ok());
    let ticker = ticker_of(side);
    // One batch for every account: the rows are read once, the next user id is
    // counted up and each password (128 random bits, see
    // `BOT_SPAWN_ITERATIONS`) is hashed at one iteration, so 300 bots hold the
    // world lock for milliseconds, not ~0.5 s.
    let mut wanted: Vec<(String, String, Vec<u8>)> = Vec::with_capacity(count as usize);
    for i in 1..=count {
        let mine = bytes
            .get((i as usize - 1) * BOT_BYTES..i as usize * BOT_BYTES)
            .ok_or_else(|| "not enough random bytes".to_owned())?;
        wanted.push((format!("{group}{i}"), hex(&mine[..16]), mine[16..].to_vec()));
    }
    let batch: Vec<(&str, &str, Vec<u8>)> = wanted
        .iter()
        .map(|(n, p, s)| (n.as_str(), p.as_str(), s.clone()))
        .collect();
    let accounts = crate::account_book::ensure_bot_accounts(
        reg.deltas_mut(),
        &batch,
        crate::account_book::BOT_SPAWN_ITERATIONS,
    )?;
    let mut logins = Vec::new();
    // Read once; each pick is added, so the next bot avoids it.
    let mut taken = crate::account_book::TakenNames::new(reg);
    for ((name, password, _), row) in wanted.iter().zip(&accounts) {
        // The account's own character first (its name is a pool name since
        // NAMES, the key before that), else one named like the key that is
        // nobody's yet or someone else's (refused below).
        let existing = reg
            .config()
            .characters
            .iter()
            .find(|c| c.owner_user_id == Some(row.user_id))
            .or_else(|| {
                reg.config()
                    .characters
                    .iter()
                    .find(|c| c.name.eq_ignore_ascii_case(name))
            })
            .map(|c| (c.character_id, c.owner_user_id));
        let character = match existing {
            Some((id, owner)) if owner == Some(row.user_id) => id,
            Some(_) => {
                return Err(format!(
                    "a character named {name} belongs to someone else; pick another group name"
                ));
            }
            None => {
                let character_name = taken.pick(name);
                reg.create_owned_character(row.user_id, character_name)
            }
        };
        if let Some(c) = reg.character_mut(character) {
            c.is_bot = true;
        }
        crate::service::corp::dev_join(reg, character, &ticker);
        if let Some(system) = system {
            // A bot that is not docked (still logged in somewhere) is left
            // where it is; a new one is always docked.
            let prefer = reg.ext::<BotControl>().seat_pref;
            let _ = crate::dev_mode::seat_in_system(reg, character, system, prefer);
        }
        logins.push(format!("{name}:{password}"));
    }
    Ok(logins.join(","))
}

/// The command line the peer gets, or the caller's mistake.
fn wire_for(reg: &Registry, actor: Option<i64>, args: &[&str]) -> Result<String, String> {
    let word_ok = |w: &str| {
        !w.is_empty()
            && w.len() <= 40
            && w.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | ','))
    };
    for w in args {
        if !word_ok(w) {
            return Err(format!("'{w}' is not a plain word. {USAGE}"));
        }
    }
    match args {
        ["spawn", group, side, role, count, skills, ship, rest @ ..] if rest.len() <= 1 => {
            match count.parse::<u32>() {
                Ok(n) if (1..=MAX_COUNT).contains(&n) => {}
                _ => return Err(format!("count is 1..={MAX_COUNT}, not '{count}'")),
            }
            if let [system] = rest
                && system.parse::<i64>().is_err()
            {
                return Err(format!("system is a numeric id, not '{system}'"));
            }
            Ok(args.join(" "))
        }
        ["list"]
        | ["despawn", _]
        | ["pause", _]
        | ["resume", _]
        | ["inspect", _]
        | ["stop-all"] => Ok(args.join(" ")),
        ["side", _, _] => Ok(args.join(" ")),
        // FC-3: the caller's wing (every bot that follows him) or a group.
        ["order", "me", words @ ..] if !words.is_empty() => {
            let who = actor.ok_or("no character")?;
            Ok(format!("order {who} {}", words.join(" ")))
        }
        ["order-group", _, words @ ..] if !words.is_empty() => Ok(args.join(" ")),
        ["join"] => {
            let who = actor.ok_or("no character")?;
            Ok(format!("join {who}"))
        }
        ["bring", group] => {
            let who = actor.ok_or("no character")?;
            let c = reg.character(who).ok_or("no character")?;
            let (Some(system), Some(station)) = (c.solar_system_id, reg.docked_station(who)) else {
                return Err(
                    "dock first: the bots meet you in your station (bring does not find you in space)"
                        .to_owned(),
                );
            };
            Ok(format!("bring {group} {system} {station} {who}"))
        }
        // GANG-SUMMON: the bots fly to the caller's character, wherever he
        // is in his system (docked or in space): the server's `'char'` warp
        // resolver finds him anywhere in his system.
        ["come", group] => {
            let who = actor.ok_or("no character")?;
            reg.character(who).ok_or("no character")?;
            let system = reg
                .session_system(who)
                .ok_or("I do not know where you are")?;
            Ok(format!("summon {group} {system} {who}"))
        }
        _ => Err(USAGE.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_bubble_file_round_trips_and_p95_is_nearest_rank() {
        let st = BubbleState {
            seq: 7,
            unix_s: 100,
            p95_ms: 12.5,
            humans: vec![(30_000_001, 2), (30_000_009, 1)],
        };
        assert_eq!(BubbleState::parse(&st.render()), Some(st));
        assert_eq!(BubbleState::parse("nonsense"), None);
        assert_eq!(BubbleState::parse("#bubble\n1\t2\tNaN\n"), None);
        let ms = |n: u64| Duration::from_millis(n);
        let v: Vec<Duration> = (1..=40).map(ms).collect();
        assert_eq!(p95(&v), ms(38));
        assert_eq!(p95(&[]), Duration::ZERO);
        assert_eq!(p95(&[ms(5)]), ms(5));
    }

    use super::*;
    use crate::model::SandboxConfig;
    use std::io::Read;

    /// An invented peer: connects with the token, answers each command with
    /// `echo: <command>`.
    fn peer(link: &Arc<Link>, token: &str) -> (TcpStream, String) {
        let mut s = TcpStream::connect(link.addr()).expect("connects");
        s.write_all(format!("HELLO {token}\n").as_bytes()).unwrap();
        let mut first = String::new();
        BufReader::new(s.try_clone().unwrap())
            .read_line(&mut first)
            .unwrap();
        (s, first.trim().to_owned())
    }

    fn serve_echo(mut s: TcpStream) -> std::thread::JoinHandle<Vec<String>> {
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            while r.read_line(&mut line).unwrap_or(0) > 0 {
                let rest = line.trim_end().strip_prefix("CMD ").unwrap().to_owned();
                let (id, cmd) = rest.split_once(' ').unwrap();
                seen.push(cmd.to_owned());
                s.write_all(format!("REPLY {id} echo: {cmd}\\nsecond line\n").as_bytes())
                    .unwrap();
                line.clear();
            }
            seen
        })
    }

    /// NAMES: a spawn makes accounts keyed `<group><n>` whose characters have
    /// pool names, and a second spawn reuses them instead of making more.
    #[test]
    fn a_spawn_gives_each_bot_a_pool_name_and_keeps_it() {
        let mut reg = Registry::new(SandboxConfig::default());
        let args = ["spawn", "invg", "invside", "mine", "3", "all5", "none"];
        let logins = provision(&mut reg, &args).expect("provisions");
        let keys: Vec<&str> = logins
            .split(',')
            .map(|l| l.split(':').next().unwrap())
            .collect();
        assert_eq!(keys, ["invg1", "invg2", "invg3"]);
        let names = |reg: &Registry| -> Vec<String> {
            reg.config()
                .characters
                .iter()
                .map(|c| c.name.clone())
                .collect()
        };
        let first = names(&reg);
        let bots: Vec<&String> = first
            .iter()
            .filter(|n| crate::bot_names::pool().contains(n))
            .collect();
        assert_eq!(bots.len(), 3, "{first:?}");
        provision(&mut reg, &args).expect("provisions again");
        assert_eq!(names(&reg), first, "same characters, same names");
    }

    /// TT-B C2: a spawn's rows come from the bytes it is given, each bot
    /// taking its own slice, and the same bytes give the same rows.
    #[test]
    fn provision_from_bytes() {
        let args = ["spawn", "invg", "invside", "mine", "2", "all5", "none"];
        let bytes: Vec<u8> = (0..2 * BOT_BYTES as u8).collect();
        let run = |bytes: &[u8]| {
            let mut reg = Registry::new(SandboxConfig::default());
            let logins = provision_with(&mut reg, &args, bytes).expect("provisions");
            let rows: Vec<(String, i64, i64)> = (1..=2)
                .map(|i| {
                    let name = format!("invg{i}");
                    let row = crate::account_book::find(reg.deltas_mut(), &name).unwrap();
                    let ch = reg
                        .config()
                        .characters
                        .iter()
                        .find(|c| c.owner_user_id == Some(row.user_id))
                        .unwrap()
                        .character_id;
                    (name, row.user_id, ch)
                })
                .collect();
            (logins, rows)
        };
        let (a, rows_a) = run(&bytes);
        let (b, rows_b) = run(&bytes);
        assert_eq!((&a, &rows_a), (&b, &rows_b));
        assert!(a.contains(&format!("invg1:{}", hex(&bytes[..16]))), "{a}");
        assert!(
            a.contains(&format!("invg2:{}", hex(&bytes[BOT_BYTES..BOT_BYTES + 16]))),
            "{a}"
        );
        assert_ne!(rows_a[0].1, rows_a[1].1);
        let short = provision_with(
            &mut Registry::new(SandboxConfig::default()),
            &args,
            &bytes[..BOT_BYTES],
        );
        assert!(short.is_err());
    }

    /// The panel's inline seam keeps the relay text (it never defers).
    #[test]
    fn the_panel_still_answers_inline() {
        let link = Link::open(None).unwrap();
        let (s, _) = peer(&link, link.token());
        let seen = serve_echo(s);
        let mut reg = dev_world(true, Some(Arc::clone(&link)));
        let r = crate::bot_panel::issue(&mut reg, None, "/bots list");
        assert!(r.starts_with("echo: list"), "{r}");
        assert!(take_deferred(&mut reg).is_none());
        let _ = seen;
    }

    fn dev_world(dev: bool, link: Option<Arc<Link>>) -> Registry {
        let mut reg = Registry::new(SandboxConfig::default());
        attach(&mut reg, dev, link);
        reg
    }

    fn said(v: PyValue) -> String {
        match v {
            PyValue::Bytes(b) => String::from_utf8(b).unwrap(),
            other => panic!("not text: {other:?}"),
        }
    }

    #[test]
    fn a_bad_token_is_refused_and_a_good_one_relays() {
        let link = Link::open(None).unwrap();
        assert_eq!(peer(&link, "wrong").1, "NO");
        let (s, ok) = peer(&link, link.token());
        assert_eq!(ok, "OK");
        let seen = serve_echo(s);
        let r = link.relay("list").unwrap();
        assert_eq!(r, "echo: list\nsecond line");
        drop(link);
        let _ = seen;
    }

    #[test]
    fn no_peer_is_said_plainly() {
        let link = Link::open(None).unwrap();
        assert!(link.relay("list").unwrap_err().contains("no eve-bots"));
    }

    #[test]
    fn the_token_file_is_private_and_the_link_is_loopback() {
        let dir = std::env::temp_dir().join(format!("bot-ctl-{}", std::process::id()));
        let link = Link::open(Some(&dir)).unwrap();
        assert!(link.addr().ip().is_loopback());
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(dir.join(TOKEN_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode, 0o600);
        let mut t = String::new();
        std::fs::File::open(dir.join(TOKEN_FILE))
            .unwrap()
            .read_to_string(&mut t)
            .unwrap();
        assert_eq!(t.trim(), link.token());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refused_without_dev_and_checked_before_relaying() {
        let link = Link::open(None).unwrap();
        let (s, _) = peer(&link, link.token());
        let seen = serve_echo(s);
        let mut off = dev_world(false, Some(Arc::clone(&link)));
        let r = said(slash(&mut off, None, &["list"]).unwrap());
        assert!(r.contains("dev server"), "{r}");
        let mut on = dev_world(true, Some(Arc::clone(&link)));
        let r = said(slash(&mut on, None, &["list"]).unwrap());
        assert!(r.starts_with("echo: list"), "{r}");
        let r = said(
            slash(
                &mut on,
                None,
                &["spawn", "a", "red", "mine", "0", "all5", "merlin"],
            )
            .unwrap(),
        );
        assert!(r.contains("count"), "{r}");
        let r = said(slash(&mut on, None, &["frobnicate"]).unwrap());
        assert!(r.contains("/bots spawn"), "{r}");
        let _ = seen;
    }

    /// A non-GM session never reaches the relay, through the slash service
    /// the wire uses (it refuses first), and `run` refuses on its own too.
    #[test]
    fn a_non_gm_is_refused_over_the_slash_service() {
        use crate::service::Request;
        let link = Link::open(None).unwrap();
        let (s, _) = peer(&link, link.token());
        let seen = serve_echo(s);
        let mut reg = dev_world(true, Some(Arc::clone(&link)));
        let line = [bstr("/bots list ")];
        let asked = |reg: &mut Registry, role: i64| {
            reg.dispatch_as(
                Caller::Session { user_id: 1, role },
                &Request {
                    service: "slash",
                    method: "SlashCmd",
                    args: &line,
                    kwargs: &[],
                    bind_params: None,
                    actor: None,
                },
            )
        };
        assert!(
            asked(&mut reg, crate::roles::PLAYER_ONLY).is_err(),
            "refused"
        );
        // The GM's answer is a raised `CustomInfo`, so the client shows it
        // (a returned string only reaches the logger window).
        let Err(crate::service::ServiceError::Raise(PyValue::ObjectEx { header, .. })) =
            asked(&mut reg, crate::roles::ROLE_GML)
        else {
            panic!("a GM's /bots answers as a raised CustomInfo");
        };
        let PyValue::Tuple(h) = header.as_ref() else {
            panic!("UserError header is a tuple");
        };
        let PyValue::Tuple(args) = &h[1] else {
            panic!("UserError args are a tuple");
        };
        assert_eq!(args[0], bstr("CustomInfo"));
        let PyValue::Dict(d) = &args[1] else {
            panic!("UserError dict");
        };
        assert_eq!(d[0].1, bstr("info"));
        assert!(said(d[0].0.clone()).starts_with("echo: list"));
        let _ = seen;
    }

    #[test]
    fn a_world_without_a_link_says_so() {
        let mut reg = dev_world(true, None);
        let r = said(slash(&mut reg, None, &["list"]).unwrap());
        assert!(r.contains("no eve-bots"), "{r}");
    }

    /// BOT-ADMIN: the owner's words reach the peer as the explicit line, with
    /// a fresh group name, the side and the default count; the old form still
    /// goes through untouched.
    #[test]
    fn words_become_the_explicit_spawn_line() {
        let link = Link::open(None).unwrap();
        let (s, _) = peer(&link, link.token());
        let seen = serve_echo(s);
        let mut reg = dev_world(true, Some(Arc::clone(&link)));
        let r = said(slash(&mut reg, None, &["10", "Allied", "MINERS", "in", "70001"]).unwrap());
        assert!(
            r.starts_with("10 allied miners (side ALLY) in system 70001, group Miners1x:"),
            "{r}"
        );
        assert!(
            r.contains("echo: spawn Miners1x ALLY mine 10 role Hulk 70001 tokens=Miners1x1:"),
            "{r}"
        );
        let r = said(slash(&mut reg, None, &["hostile", "gang", "at", "70002"]).unwrap());
        assert!(
            r.contains("echo: spawn RedGang1x FOES roamgang 5 role Rifter 70002 tokens="),
            "{r}"
        );
        // Without a character there is no "here".
        let r = said(slash(&mut reg, None, &["3", "orcas"]).unwrap());
        assert!(r.contains("where you are"), "{r}");
        // Unknown words: a dialog with examples, nothing relayed.
        let r = said(slash(&mut reg, None, &["5", "purple", "widgets"]).unwrap());
        assert!(
            r.contains("purple") && r.contains("/bots 10 allied miners"),
            "{r}"
        );
        let r = said(slash(&mut reg, None, &["help"]).unwrap());
        assert!(r.contains("pause") && r.contains("KILL"), "{r}");
        // The old explicit form and the per-bot verbs.
        let r = said(
            slash(
                &mut reg,
                None,
                &[
                    "spawn", "gang1", "RED", "mine", "3", "role", "Hulk", "30000042",
                ],
            )
            .unwrap(),
        );
        assert!(
            r.starts_with("echo: spawn gang1 RED mine 3 role Hulk 30000042 tokens=gang11:"),
            "{r}"
        );
        for (words, wire) in [
            (vec!["pause", "Crew"], "echo: pause Crew"),
            (vec!["resume", "crew1"], "echo: resume crew1"),
            (vec!["inspect", "crew1"], "echo: inspect crew1"),
            (vec!["stop-all"], "echo: stop-all"),
        ] {
            let r = said(slash(&mut reg, None, &words).unwrap());
            assert!(r.starts_with(wire), "{r}");
        }
        let _ = seen;
    }

    /// GANG-SUMMON: `come` becomes `summon <group> <system> <char>` docked or
    /// in space; a hostile gang `here` carries the quarry before its logins,
    /// an allied one does not (unless it goes `with me`).
    #[test]
    fn come_summons_and_a_hostile_gang_hunts_the_caller() {
        let link = Link::open(None).unwrap();
        let (s, _) = peer(&link, link.token());
        let seen = serve_echo(s);
        let mut reg = dev_world(true, Some(Arc::clone(&link)));
        let me = reg.config().characters[0].character_id;
        let here = reg.config().characters[0]
            .solar_system_id
            .expect("a system");
        // Docked.
        let r = said(slash(&mut reg, Some(me), &["come", "Crew"]).unwrap());
        assert!(r.contains(&format!("echo: summon Crew {here} {me}")), "{r}");
        // In space.
        reg.character_mut(me).unwrap().station_id = None;
        let r = said(slash(&mut reg, Some(me), &["come"]).unwrap());
        assert!(r.contains(&format!("echo: summon all {here} {me}")), "{r}");
        // A hostile gang here hunts him; the quarry sits before the logins.
        let r = said(slash(&mut reg, Some(me), &["5", "hostile", "roamers", "here"]).unwrap());
        assert!(
            r.contains(&format!(" quarry={me}:{here} tokens=RedGang1x1:")),
            "{r}"
        );
        assert!(r.contains("(hunting you)"), "{r}");
        // HUNT-SIDE: `hunt me` without the word hostile is still the foe side,
        // and `with me` keeps his own.
        let r = said(slash(&mut reg, Some(me), &["20", "dictors", "hunt", "me"]).unwrap());
        let foe = crate::bot_panel::FOE_SIDE;
        assert!(r.contains(&format!("(side {foe})")), "{r}");
        assert!(r.contains("(hunting you)"), "{r}");
        let r = said(slash(&mut reg, Some(me), &["3", "roamers", "with", "me"]).unwrap());
        assert!(!r.contains(&format!("(side {foe})")), "{r}");
        // An allied one does not, nor does a hostile one named to a system.
        let r = said(slash(&mut reg, Some(me), &["5", "allied", "roamers"]).unwrap());
        assert!(!r.contains("quarry="), "{r}");
        let r = said(
            slash(
                &mut reg,
                Some(me),
                &["5", "hostile", "roamers", "in", "70003"],
            )
            .unwrap(),
        );
        assert!(!r.contains("quarry="), "{r}");
        // Allied `with me` goes to him.
        let r = said(
            slash(
                &mut reg,
                Some(me),
                &["3", "allied", "roamers", "with", "me"],
            )
            .unwrap(),
        );
        assert!(r.contains(&format!(" quarry={me}:{here} tokens=")), "{r}");
        // Flown to another system, his home unchanged: bots follow him.
        let other = here + 7_001;
        let ship = reg.character(me).unwrap().ship_id;
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                me,
                ship,
                other,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        let r = said(slash(&mut reg, Some(me), &["come", "Crew"]).unwrap());
        assert!(
            r.contains(&format!("echo: summon Crew {other} {me}")),
            "{r}"
        );
        let r = said(slash(&mut reg, Some(me), &["5", "hostile", "roamers", "here"]).unwrap());
        assert!(r.contains(&format!(" quarry={me}:{other} tokens=")), "{r}");
        let r = said(slash(&mut reg, Some(me), &["2", "wing", "here"]).unwrap());
        assert!(r.contains(&other.to_string()), "{r}");
        // `bring` refuses in space even with a station on the record.
        reg.character_mut(me).unwrap().station_id = Some(1);
        let r = said(slash(&mut reg, Some(me), &["bring", "Crew"]).unwrap());
        assert!(r.contains("dock first"), "{r}");
        // Without a character there is nobody to come to.
        let r = said(slash(&mut reg, None, &["come"]).unwrap());
        assert!(r.contains("no character"), "{r}");
        let _ = seen;
    }

    /// FC-1: `/bots 2 wing` names the caller as the wing's FC on the wire
    /// (before the logins), lists his fleet, and says what it did; without a
    /// fleet it says to form one and still spawns.
    #[test]
    fn a_wing_spawn_carries_the_fc_and_lists_his_fleet() {
        let link = Link::open(None).unwrap();
        let (s, _) = peer(&link, link.token());
        let seen = serve_echo(s);
        let mut reg = dev_world(true, Some(Arc::clone(&link)));
        let me = reg.config().characters[0].character_id;
        // No fleet yet.
        let r = said(slash(&mut reg, Some(me), &["2", "wing"]).unwrap());
        assert!(r.contains(&format!(" fc={me} tokens=")), "{r}");
        assert!(r.contains("form one"), "{r}");
        // A fleet he bosses: listed, then kept.
        let id = reg.fleets_mut().create(me).expect("created");
        let eff = reg
            .fleets_mut()
            .init(me, id, Some(670), None, None)
            .expect("formed");
        reg.fleet_apply(eff);
        let r = said(slash(&mut reg, Some(me), &["2", "wing"]).unwrap());
        assert!(r.contains("now listed"), "{r}");
        assert!(crate::fleet_finder::mine(&reg, Some(me)).is_some());
        let r = said(slash(&mut reg, Some(me), &["2", "wing"]).unwrap());
        assert!(r.contains("is kept"), "{r}");
        // Another kind of spawn carries no fc.
        let r = said(slash(&mut reg, Some(me), &["2", "escorts"]).unwrap());
        assert!(!r.contains(" fc="), "{r}");
        let _ = seen;
    }

    /// FC-3: the owner's words for the wing become `order <me> <words>` (or
    /// `order-group`), `join` lists his fleet and relays `join <me>`; a
    /// sov fleet spawns its DPS share and says what is deferred; roles the
    /// bots do not have are said, not spawned.
    #[test]
    fn the_wings_words_become_order_wires_and_the_sov_kinds_are_honest() {
        let link = Link::open(None).unwrap();
        let (s, _) = peer(&link, link.token());
        let seen = serve_echo(s);
        let mut reg = dev_world(true, Some(Arc::clone(&link)));
        let me = reg.config().characters[0].character_id;
        for (words, wire) in [
            (vec!["follow", "me"], format!("echo: order {me} follow me")),
            (vec!["hold"], format!("echo: order {me} hold")),
            (
                vec!["weapons", "free"],
                format!("echo: order {me} weapons free"),
            ),
            (vec!["go", "home"], format!("echo: order {me} go home")),
            (
                vec!["primary", "Invented", "Rat"],
                format!("echo: order {me} primary invented rat"),
            ),
            (vec!["order", "dance"], String::new()),
            (
                vec!["order", "Crew", "hold"],
                "echo: order-group Crew hold".to_owned(),
            ),
        ] {
            let r = said(slash(&mut reg, Some(me), &words).unwrap());
            if wire.is_empty() {
                assert!(r.contains("do not know"), "{r}");
            } else {
                assert!(r.starts_with(&wire), "{words:?}: {r}");
            }
        }
        // Hold with a name still pauses that group.
        let r = said(slash(&mut reg, Some(me), &["hold", "Crew"]).unwrap());
        assert!(r.starts_with("echo: pause Crew"), "{r}");
        // `join`: no fleet to list, still relayed.
        let r = said(slash(&mut reg, Some(me), &["join"]).unwrap());
        assert!(r.contains(&format!("echo: join {me}")), "{r}");
        assert!(r.contains("form one"), "{r}");
        // Not-built roles are said and nothing is spawned.
        for words in [vec!["2", "linkers"], vec!["hub", "hauler"]] {
            let r = said(slash(&mut reg, Some(me), &words).unwrap());
            assert!(r.contains("not built yet") && !r.contains("echo:"), "{r}");
        }
        // The logi is built (FC-2): a winglogi spawn with the caller as FC.
        let r = said(slash(&mut reg, Some(me), &["2", "logi"]).unwrap());
        assert!(r.contains(" winglogi 2 all5 "), "{r}");
        assert!(r.contains(&format!(" fc={me} tokens=")), "{r}");
        // A sov fleet: the DPS share is a wing with an FC, the rest is said.
        let r = said(slash(&mut reg, Some(me), &["10", "sovfleet"]).unwrap());
        assert!(r.contains("not built yet"), "{r}");
        assert!(r.contains(" wing 6 all5 Hurricane"), "{r}");
        assert!(r.contains(&format!(" fc={me} tokens=")), "{r}");
        let _ = seen;
    }

    #[test]
    fn the_control_dir_defaults_to_the_worlds() {
        assert_eq!(
            dir_for(None, Some(Path::new("/x/y/world.bin"))),
            Some(PathBuf::from("/x/y"))
        );
        assert_eq!(dir_for(None, None), None);
        assert_eq!(
            dir_for(Some("/c".into()), Some(Path::new("/x/w"))),
            Some(PathBuf::from("/c"))
        );
    }
}
