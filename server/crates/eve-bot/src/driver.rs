//! The driver: one thread per bot, turning the brain's [`Act`]s into verbs
//! and the server's answers back into an [`Observation`].
//!
//! ```text
//! loop {                                   // supervisor: connect .. fail .. back off
//!   connect ; handshake_as(token) ; select_character_named(name)
//!   observe from scratch                   // session -> place; List hangar/ship; bind if in space
//!   loop {
//!     (memory, acts) = brain::decide(&memory, &observation)
//!     for act in acts { react ; execute ; log ; fold the result in }
//!     wait on the socket                   // pushes are the clock
//!   }
//!   Io | Protocol -> log, back off 1, 2, 4 .. 60 s (+ jitter), again
//! }
//! ```
//!
//! (`docs/lanes/bots.md` §4.3.) The rules that matter:
//!
//! - **The socket is the clock.** Every wait is [`Conn::pump`], which wakes
//!   on the first push, so a bot never sleeps through its laser stopping.
//!   Every push read anywhere -- inside a verb's own wait too -- reaches
//!   [`absorb`] through [`Conn::take_pushes`].
//! - **A refusal is not a fault.** [`Error::NotSent`] and [`Error::Refused`]
//!   go into `failures[kind]` for the brain to read next turn. Only
//!   `Error::Io` and `Error::Protocol` end the connection, and only they
//!   back off.
//! - **A reconnect rebuilds everything from the server** -- the session, the
//!   hangar, the hold, the park -- never from what the last connection
//!   believed. The brain's [`Memory`] survives; it holds no world.
//! - **GM lives in [`setup`] and nowhere else**, and each slash is an act-log
//!   entry, so a test can prove none comes after the first undock (T9).
//!   The closed economy's [`Act::Acquire`] (E8) is [`crate::acquire::run`]:
//!   no slash; each thing it cannot get is a `MISSING` line of its own.
//! - **Pacing is [A]**: a reaction delay before each act, a dwell before
//!   undocking (the brain's), a stagger between logins -- from a xorshift
//!   seeded by the bot's name. [`Pace::Fast`] makes every one zero, for
//!   tests. No number from here appears in a test.
//! - **Chat is optional and never fatal** (P8c). After login the bot asks
//!   the server for its chat address the client's way ([`crate::chat`]);
//!   `None` -- the server's `--chat` is off -- is one `chat` log line and
//!   silence. A chat that dies is dropped; the machoNet connection is not.
//! - **What the escort sees, and from where** (P8c): NPCs and NPC wrecks by
//!   their slims in the park ([`is_npc`], [`is_npc_wreck`]); which ship an
//!   NPC is shooting from its gun's `OnSpecialFX`; its own bars from
//!   `OnDamageStateChange`; its guns' rounds from `OnGodmaPrimeItem` and
//!   `OnModuleAttributeChanges`, and from `ItemGetInfo` on the
//!   `(ship, flag, type)` key whenever it lists its ship.
//! - **Pacing, homes and courtesy** (CP39, C7 §6.3). At [`Pace::Human`]
//!   every payload the bot writes passes a [`Pacer`] first, so no
//!   [`PACING_WINDOW`] holds more than [`BOT_PAYLOADS_PER_S`] a second of
//!   them. A bot may have a home system ([`BotConfig::home_system`]), drawn
//!   from [`bot_homes`]: the humans' home and the systems within
//!   [`HOME_RANGE_JUMPS`] of it. The park's humans ([`is_human`]) are what
//!   the brain's courtesy rule reads; the bot marker is [`BotMarker`],
//!   bot-side, never anything a client renders (ruling 5).
//! - **The courier** (`--loop courier`): [`Act::Courier`] is one
//!   [`crate::courier::run`] on this connection.
//! - **The site runner** (`--loop sites`): [`Act::Sites`] is one
//!   [`crate::site_runner::scout`] on this connection, on the process's
//!   [`crate::site_runner::BOARD`].
//! - **The mission runner** (`--loop missions`, SOC-B5): [`Act::Mission`]
//!   is one [`crate::missioner::run`] on this connection, with the
//!   policy's agent and map.
//! - **The market** (FX6): [`Act::Sell`] is one
//!   [`crate::builder::sell_goods`] (the miner's refine-and-sell), and
//!   [`Act::Market`] one [`crate::builder::run_with`] (the builder's
//!   cycle). Each logs what it did on its act line. The demand budget is
//!   the server's since E11, so the bot keeps none.
//! - **Society mode** (SOC-A2, `eve-bots --society`): a bot with a
//!   [`Society`] has a mind ([`crate::mind::Mind`]) whose career sets
//!   [`Policy::role`]. The career is decided only at a docked decision
//!   point ([`decision_point`]): docked, not stranded, and at most once per
//!   [`Society::decide_every`]; never in space, so never mid-trip. Each
//!   decision books the wallet's change since the last one into the
//!   career's income ledger, prints its `[soc]` line, and saves the mind
//!   when there is a state directory ([`crate::state`]). The bot plays in
//!   sessions ([`Timetable`]): at a window's end it logs off docked (or
//!   heads home and logs off after [`LOGOFF_GRACE`]), and it logs on again
//!   when the next window opens.
//! - **The way home** (SOC-A2, the travel lane's follow-up): an errand
//!   ([`Act::Market`], [`Act::Acquire`]) that ends anywhere but the bot's
//!   home station -- the first station it docked at in this run -- leaves
//!   the bot stranded, and its next turn flies it home over the gates
//!   ([`crate::travel::go`]), at most [`RETURN_TRIES`] times.
//! - **A park adrift** (FX6, `park.rs`'s header): when the ball the ego was
//!   tethered to leaves the park, one `UpdateStateRequest` per park state
//!   puts the ego back where the server has it.

use crate::brain::{
    self, Act, ActKind, Ball, FarewellKit, FleetView, GunInfo, Health, Homing, Hostile, Invite,
    Memory, ModuleInfo, Observation, ParkView, Place, Policy, RoleRef, ShipInfo, Stop, StopReason,
};
use crate::career::{self, Career, Signals};
use crate::chat::Chat;
use crate::escort;
use crate::mind::{self, Decision, Mind};
use crate::roles;
use crate::{Conn, Error, GodmaEffect, LockState, Result};
use eve_marshal::PyValue;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How a connection waits for a reply before calling it lost.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// The first and the longest back-off after a lost connection.
const BACKOFF_FIRST: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A connection that lasted this long resets the back-off. **[A]**
const HEALTHY: Duration = Duration::from_secs(60);
/// How often a bot prints its summary line.
const SUMMARY_EVERY: Duration = Duration::from_secs(60);
/// How often a held Reconnect (BOT-SCRAM) is written to the act log **[A]**.
const RECONNECT_HELD_LOG: Duration = Duration::from_secs(30);
/// The act log keeps this many entries, newest last.
///
/// 2000, not 20 000 (BOT-OPT): the log is for a GM's `/bots` and for tests,
/// and at ~200 bytes an entry the old cap let every bot hold 4 MB over a
/// long run; 500 bots held 2 GB of history nobody read.
const LOG_CAP: usize = 2_000;
/// How long a movement act waits for the park to show it.
const MOVE_EVIDENCE: Duration = Duration::from_millis(2_500);
/// `const.groupStation`-range ids: a station in the park when the bot has
/// never docked on this connection. **[A]**, like `park::is_rock`: the
/// client knows a station by its group, from static data this crate does
/// not ship.
const STATION_IDS: std::ops::Range<i64> = 60_000_000..64_000_000;
/// `IsNPCCorporation`: an NPC corporation's id range (`michelle.py:1972`,
/// quoted in `docs/lanes/damage-death.md`, P2.4) **[V]**.
const NPC_CORPS: std::ops::Range<i64> = 1_000_000..2_000_000;
/// `falloff` (158) and `maxRange` (54) **[V-data]**: a gun's reach.
/// How often the chat socket is drained. **[A]**
const CHAT_DRAIN_EVERY: Duration = Duration::from_secs(1);
/// How long a heard line or an arrival stays in the observation (S1):
/// longer than the chatter's own stale limit.
/// The chat and the governor's kit, lent to the connection's pre-jump hook
/// for one act ([`Driver::arm_farewell`]).
struct Armed(Rc<RefCell<FarewellJob>>);

struct FarewellJob {
    chat: Option<Chat>,
    kit: FarewellKit,
    said: Vec<String>,
    lost: Option<String>,
}

/// Character ids asked about in one `GetMultiOwnersEx`, and in all.
const NAMES_PER_ASK: usize = 16;
const NAMES_KEPT: usize = 1024;

const CHAT_KEEP: Duration = Duration::from_secs(60);
/// The most payloads a second a human-paced bot writes, averaged over any
/// [`PACING_WINDOW`] (C7 §6.3, L9). **[A]**: falsified by a human's turn
/// waiting on bot turns in `[live]` (too high), or by bots that visibly
/// stall mid-act (too low).
pub const BOT_PAYLOADS_PER_S: f64 = 2.0;
/// FAST-KIT: a `/bots` spawn ([`crate::brain::Policy::quick`]) is paced at
/// this many payloads a second instead, so its docked prep is not spread
/// over minutes by the person-rate budget. **[A]**: 10 a second per bot;
/// falsified by the server's tick p99 rising while a 150-bot spawn preps
/// (then lower it).
pub const QUICK_PAYLOADS_PER_S: f64 = 10.0;
/// The window [`BOT_PAYLOADS_PER_S`] is held over: a burst inside it is
/// fine, the average is not. **[A]** (L9's 10 s).
pub const PACING_WINDOW: Duration = Duration::from_secs(10);
/// How far from the humans' home system a bot's home may be, in jumps
/// (C7 §6.3). **[A]**: falsified by humans feeling crowded (then widen).
pub const HOME_RANGE_JUMPS: u32 = 2;

/// How quickly a bot acts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pace {
    /// Reaction delays, dwells and staggers like a person's. **[A]**
    Human,
    /// None at all: for tests.
    Fast,
}

impl Pace {
    /// The reaction delay before each act. **[A]**
    fn reaction(self) -> (Duration, Duration) {
        match self {
            Pace::Human => (Duration::from_millis(600), Duration::from_millis(2_500)),
            Pace::Fast => (Duration::ZERO, Duration::ZERO),
        }
    }

    /// The gap between one bot's login and the next one's. **[A]**
    pub fn stagger(self) -> (Duration, Duration) {
        match self {
            Pace::Human => (Duration::from_secs(3), Duration::from_secs(10)),
            Pace::Fast => (Duration::ZERO, Duration::ZERO),
        }
    }

    /// Whether payloads pass a [`Pacer`]: a person's pace is a person's
    /// rate; tests run unpaced.
    pub fn paced(self) -> bool {
        self == Pace::Human
    }

    /// The brain's policy at this pace.
    pub fn policy(self) -> Policy {
        match self {
            Pace::Human => Policy::human(),
            Pace::Fast => Policy::fast(),
        }
    }
}

/// The token a bot sends when it has none of its own: no `--tokens FILE`,
/// or no line for it there. [A] Without `--accounts` every token is the
/// catch-all GM, so this is today's behaviour; with `--accounts` the
/// server's `check_names` refuses any accounts file whose hashes accept it
/// (`eve_server::accounts::TOOL_TOKENS`), so a bot without a token is
/// refused rather than let in as somebody. Falsified by an accounts mode
/// that admits a tool token.
pub const DEV_TOKEN: &str = "dev";

/// **The bot marker** (ruling 5, plan §4.7): the character ids of the bots
/// this process runs, each added at its login. Bot-side only. **[A]**: a
/// bot run by another `eve-bots` process is a human to these; falsified by
/// bots that yield to each other across processes.
pub type BotMarker = Arc<Mutex<BTreeSet<i64>>>;

/// **Bots of this process with no session** (pilotless held ships): the
/// server holds a despawned or logged-off pilot's ship in space for minutes,
/// and a hunter must not take that ship for a foe to fight. Added when a
/// session ends, removed at the bot's next login. Process-wide, as the
/// marker's scope is **[A]**, per server so that two worlds never mix; falsified
/// by a hunter that leaves a live bot alone, or one that fights a ship nobody
/// flies.
static DEPARTED: Mutex<BTreeSet<(SocketAddr, i64)>> = Mutex::new(BTreeSet::new());

fn departed(server: SocketAddr, char_id: i64) -> bool {
    DEPARTED
        .lock()
        .map(|d| d.contains(&(server, char_id)))
        .unwrap_or(false)
}

fn set_departed(server: SocketAddr, char_id: i64, gone: bool) {
    if let Ok(mut d) = DEPARTED.lock() {
        if gone {
            d.insert((server, char_id));
        } else {
            d.remove(&(server, char_id));
        }
    }
}

/// **The payload pacer** (C7 §6.3): books each payload at the earliest
/// instant that keeps every [`PACING_WINDOW`] at or under its budget,
/// `BOT_PAYLOADS_PER_S * PACING_WINDOW` payloads, and says how long to wait
/// for it. Shared across a bot's reconnects, so a reconnect is no reset.
#[derive(Debug, Clone)]
pub struct Pacer {
    budget: usize,
    window: Duration,
    /// The last `budget` booked send instants, oldest first.
    sent: VecDeque<Instant>,
}

impl Pacer {
    /// [`BOT_PAYLOADS_PER_S`] over [`PACING_WINDOW`].
    pub fn standard() -> Pacer {
        let budget = (BOT_PAYLOADS_PER_S * PACING_WINDOW.as_secs_f64()).floor() as usize;
        Pacer {
            budget: budget.max(1),
            window: PACING_WINDOW,
            sent: VecDeque::new(),
        }
    }

    /// FAST-KIT: [`QUICK_PAYLOADS_PER_S`] over [`PACING_WINDOW`], for a dev
    /// spawn whose docked prep is a burst of fits and buys.
    pub fn quick() -> Pacer {
        let budget = (QUICK_PAYLOADS_PER_S * PACING_WINDOW.as_secs_f64()).floor() as usize;
        Pacer {
            budget: budget.max(1),
            window: PACING_WINDOW,
            sent: VecDeque::new(),
        }
    }

    /// Book one payload wanted at `now`: the wait before writing it. The
    /// payload `budget` back must be a whole window old, so no window holds
    /// `budget + 1`.
    pub fn admit(&mut self, now: Instant) -> Duration {
        let mut at = now;
        if let Some(&last) = self.sent.back() {
            at = at.max(last);
        }
        if self.sent.len() >= self.budget {
            at = at.max(self.sent[self.sent.len() - self.budget] + self.window);
        }
        self.sent.push_back(at);
        while self.sent.len() > self.budget {
            self.sent.pop_front();
        }
        at - now
    }
}

/// Surface gap from this ship to `ball`, when both are in the park.
/// Whether a bot adrift (its tether target left the park) skips the
/// `UpdateStateRequest`: only a wing member whose tether target was its FC's
/// ship. `fc` is the FC's character, `lost_owner` the owner of the ball that
/// left ([`crate::park::World::adrift_owner`]). Any other bot, and a wing
/// member that lost something else, still resyncs.
/// **[A]** a ball's slim owner is its pilot's character; falsified by wing
/// followers still logging `UpdateStateRequest: the ego's tether target left`
/// after their FC warps.
fn skips_resync(wing: bool, fc: Option<i64>, lost_owner: Option<i64>) -> bool {
    wing && fc.is_some() && fc == lost_owner
}

fn warp_gap(conn: &mut Conn, ball: Option<i64>) -> Option<f64> {
    let w = conn.world_now();
    w.surface_distance(w.ego?, ball?)
}

/// **Is this pilot a human outside the bot's fleet?** A pilot at all (an NPC
/// has none), not the bot itself, not a bot the marker knows, and not the
/// fleet boss -- the only fleet member a bot knows (`FleetView::boss`)
/// **[A]**: a human fleet member who is not the boss counts as outside.
pub fn is_human(
    pilot: Option<i64>,
    me: Option<i64>,
    boss: Option<i64>,
    bots: &BTreeSet<i64>,
) -> bool {
    pilot.is_some_and(|c| Some(c) != me && Some(c) != boss && !bots.contains(&c))
}

/// The stargate graph: `system -> the systems one jump away`, both ways,
/// from the celestial catalogue's gates (`--celestials`, or the bundle's).
pub type Graph = BTreeMap<i64, BTreeSet<i64>>;

/// [`Graph`] from a catalogue: every served gate links its own system to
/// its partner's.
pub fn gate_graph(cat: &eve_server::celestial::Catalogue) -> Graph {
    let mut g = Graph::new();
    for gate in cat
        .served()
        .filter(|c| c.kind == eve_server::celestial::Kind::Gate)
    {
        let (Some(from), Some((_, to))) = (cat.system_of(gate.id), cat.jump_of(gate.id)) else {
            continue;
        };
        if from == 0 || from == to {
            continue;
        }
        g.entry(from).or_default().insert(to);
        g.entry(to).or_default().insert(from);
    }
    g
}

/// Every system within `jumps` of `home`, `home` included, ascending.
pub fn within(g: &Graph, home: i64, jumps: u32) -> Vec<i64> {
    let mut seen = BTreeSet::from([home]);
    let mut edge = vec![home];
    for _ in 0..jumps {
        let mut next = Vec::new();
        for s in edge {
            for &n in g.get(&s).into_iter().flatten() {
                if seen.insert(n) {
                    next.push(n);
                }
            }
        }
        edge = next;
    }
    seen.into_iter().collect()
}

/// **The bots' home systems** (`--home`, `--bot-systems`): the explicit
/// list, each checked to be within [`HOME_RANGE_JUMPS`] of the humans' home
/// when the gate graph is known; else every system that is. With no graph
/// and no list, the humans' home alone.
pub fn bot_homes(
    g: Option<&Graph>,
    home: i64,
    explicit: &[i64],
) -> std::result::Result<Vec<i64>, String> {
    let near = g.map(|g| within(g, home, HOME_RANGE_JUMPS));
    if explicit.is_empty() {
        return Ok(near.unwrap_or_else(|| vec![home]));
    }
    if let Some(near) = &near
        && let Some(far) = explicit.iter().find(|s| !near.contains(s))
    {
        return Err(format!(
            "--bot-systems {far}: more than {HOME_RANGE_JUMPS} jumps from --home {home}"
        ));
    }
    Ok(explicit.to_vec())
}

/// One bot's home: the list's entry at its name's hash, so a rerun places
/// it the same way and N bots spread over the list.
pub fn pick_home(name: &str, homes: &[i64]) -> Option<i64> {
    (!homes.is_empty()).then(|| homes[(brain::name_hash(name) % homes.len() as u64) as usize])
}

// ---- society mode (SOC-A2) ------------------------------------------------------------

/// The society seed `eve-bots --society` draws traits under unless
/// `--society-seed` says otherwise (`SOCIETY_SEED`, R-SOCIETY §2.1)
/// **[A]**: any fixed number; a stored mind keeps its own.
pub const SOCIETY_SEED: u64 = 1;
/// How often a docked bot weighs its career **[A]**: with
/// [`mind::SWITCH_CONFIRMATIONS`] this is how long a better career must
/// stay better before a switch. Falsified with `HYSTERESIS` (flapping or
/// stuck in `[soc]`).
pub const DECIDE_EVERY: Duration = Duration::from_secs(10 * 60);
/// Hours a day a bot plays at `diligence` 0 and at 1 **[A]**: falsified by
/// the owner finding the server empty (too few) or no coming and going
/// (too many).
pub const SESSION_HOURS: (f64, f64) = (2.0, 10.0);
/// Each weekday's hours vary by up to this fraction either way **[A]**.
pub const SESSION_JITTER: f64 = 0.25;
/// Each weekday's start moves by up to this many hours either way **[A]**.
pub const START_JITTER_H: f64 = 2.0;
/// A Saturday or Sunday window is this much longer than a weekday's **[A]**:
/// falsified by the weekend server looking no fuller than a Tuesday's.
pub const WEEKEND_FACTOR: f64 = 1.3;
/// The cycle's days that are a Saturday or a Sunday: the cycle starts on a
/// Unix-epoch day, a Thursday, so day 2 is Saturday and day 3 Sunday.
const WEEKEND_DAYS: [u64; 2] = [2, 3];
/// Hours in a day, and so entries in a start-hour profile.
pub const PROFILE_HOURS: usize = 24;
/// Days in a timetable's cycle: a week.
pub const WEEK_DAYS: u64 = 7;
/// A day, seconds: [`Timetable::weekly`]'s default day.
pub const DAY_SECS: u64 = 86_400;
/// How long a bot whose session ended in space may take to dock before it
/// logs off where it is **[A]**: falsified by bots logging off in space
/// in `[soc]`-era logs (too short) or playing far past their windows.
pub const LOGOFF_GRACE: Duration = Duration::from_secs(15 * 60);
/// The service and method that set the medical clone (`home_station`,
/// `set_home_station(station, allow_remote)`) **[V]**
/// (`clones::set_home_station`).
const CLONE_SERVICE: &str = "home_station";
const CLONE_METHOD: &str = "set_home_station";
/// How long a summoned bot that is not in a gang keeps coming back to the
/// summoner's grid, seconds **[A]**. Falsified by a bot that wanders off
/// while its owner still wants it, or one that cannot get back to work.
pub const SUMMON_HOLD_S: u64 = 600;
/// The warp to a summoner is repeated no sooner than this.
const SUMMON_REPEAT: Duration = Duration::from_secs(20);
/// A summoned bot gives up after this many fly-ins or warps the server
/// refused in a row.
const SUMMON_TRIES: u32 = 3;

/// A call (`/bots come`) a bot that is not in a gang is answering.
#[derive(Debug, Clone)]
struct Summoned {
    system: i64,
    char_id: i64,
    since: Instant,
    warped: Option<Instant>,
    failed: u32,
}

/// How many times a stranded bot tries the way home before it says
/// `MISSING` and plays where it is **[A]**: falsified by a bot stuck on a
/// route that is fine.
pub const RETURN_TRIES: u32 = 3;
/// The shortest wait a bot in society mode makes while its window is
/// closing or closed, so it notices the end promptly without spinning.
const SESSION_TICK: Duration = Duration::from_millis(500);
/// The longest one wait lasts for a bot the bubble watches **[A]**: a
/// release is noticed within this, at the price of a few more turns a
/// minute. Falsified by a release taking longer than this to start.
pub const BUBBLE_WAIT: Duration = Duration::from_secs(3);
/// How often `eve-bots --society` prints the population's `[soc]` summary.
pub const SOC_SUMMARY_EVERY: Duration = Duration::from_secs(60);

/// Unix seconds now: the mind's and the timetable's clock.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Parse a `population.tsv` text (`hour TAB weight`, `#` comments) into 24
/// weights; a missing or bad hour weighs 0, and a profile with no weight at
/// all is flat.
pub fn parse_profile(text: &str) -> [f64; PROFILE_HOURS] {
    let mut w = [0.0; PROFILE_HOURS];
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let mut c = line.split('\t');
        let (Some(h), Some(v)) = (c.next(), c.next()) else {
            continue;
        };
        if let (Ok(h), Ok(v)) = (h.trim().parse::<usize>(), v.trim().parse::<f64>())
            && h < PROFILE_HOURS
            && v.is_finite()
            && v > 0.0
        {
            w[h] = v;
        }
    }
    if w.iter().sum::<f64>() <= 0.0 {
        return [1.0; PROFILE_HOURS];
    }
    w
}

/// The shipped start-hour profile, parsed once.
fn start_profile() -> [f64; PROFILE_HOURS] {
    static P: std::sync::OnceLock<[f64; PROFILE_HOURS]> = std::sync::OnceLock::new();
    *P.get_or_init(|| parse_profile(include_str!("../../../../content/bots/population.tsv")))
}

/// A start time in hours `[0, 24)` drawn from `profile` by two uniform
/// numbers: `a` picks the hour by weight, `b` the point inside it.
fn pick_hour(profile: &[f64; PROFILE_HOURS], a: f64, b: f64) -> f64 {
    let total: f64 = profile.iter().sum();
    let mut left = a.clamp(0.0, 1.0) * total;
    for (h, w) in profile.iter().enumerate() {
        if left < *w {
            return h as f64 + b.clamp(0.0, 0.999_999);
        }
        left -= w;
    }
    PROFILE_HOURS as f64 - 1e-6
}

/// **A bot's sessions** (R-SOCIETY §2.2, R-ECON §1.5): windows of a
/// repeating cycle in which it is logged on. Pure: Unix seconds in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timetable {
    /// The cycle, seconds; 0 means always online.
    pub period: u64,
    /// `(start, length)`, seconds into the cycle; a window may wrap.
    pub windows: Vec<(u64, u64)>,
}

impl Timetable {
    /// Always online: `--sessions always`, and every bot outside society
    /// mode.
    pub fn always() -> Timetable {
        Timetable {
            period: 0,
            windows: Vec::new(),
        }
    }

    /// **The weekly timetable**: one window a weekday. Its length is
    /// [`SESSION_HOURS`] at the bot's `diligence`, varied by
    /// [`SESSION_JITTER`]; its start is the bot's own time of day (a hash
    /// of its name and seed) moved by [`START_JITTER_H`]. `day` is a day's
    /// length in seconds: [`DAY_SECS`], or less to compress a week into a
    /// soak (`eve-bots --day-secs`).
    pub fn weekly(name: &str, seed: u64, diligence: f64, day: u64) -> Timetable {
        Timetable::weekly_with(&start_profile(), name, seed, diligence, day)
    }

    /// [`Timetable::weekly`] with an explicit 24-weight UTC start-hour
    /// profile (`content/bots/population.tsv`): the bot's own start hour is
    /// drawn from it, and a weekend window is [`WEEKEND_FACTOR`] longer.
    pub fn weekly_with(
        profile: &[f64; PROFILE_HOURS],
        name: &str,
        seed: u64,
        diligence: f64,
        day: u64,
    ) -> Timetable {
        let day = day.max(1);
        let dayf = day as f64;
        let period = WEEK_DAYS * day;
        let mut rng = (brain::name_hash(name) ^ seed.rotate_left(29) ^ 0x5e55_1015) | 1;
        let mut dice = || brain::unit(brain::xorshift(&mut rng));
        let offset = pick_hour(profile, dice(), dice()) / 24.0 * dayf;
        let d = if diligence.is_finite() {
            diligence.clamp(0.0, 1.0)
        } else {
            0.5
        };
        let hours = SESSION_HOURS.0 + (SESSION_HOURS.1 - SESSION_HOURS.0) * d;
        let windows = (0..WEEK_DAYS)
            .map(|k| {
                let mut len_h = hours * (1.0 + SESSION_JITTER * (2.0 * dice() - 1.0));
                if WEEKEND_DAYS.contains(&k) {
                    len_h *= WEEKEND_FACTOR;
                }
                let shift_h = START_JITTER_H * (2.0 * dice() - 1.0);
                let start = (k as f64 * dayf + offset + shift_h / 24.0 * dayf)
                    .rem_euclid(period as f64) as u64;
                let len = ((len_h / 24.0 * dayf) as u64).clamp(1, day);
                (start.min(period - 1), len)
            })
            .collect();
        Timetable { period, windows }
    }

    /// Is the bot logged on at `now`?
    pub fn online(&self, now: u64) -> bool {
        if self.period == 0 {
            return true;
        }
        let t = now % self.period;
        self.windows
            .iter()
            .any(|&(s, l)| (t + self.period - s % self.period) % self.period < l)
    }

    /// Seconds from `now` until a window opens; 0 while one is open.
    pub fn until_online(&self, now: u64) -> u64 {
        if self.online(now) {
            return 0;
        }
        let t = now % self.period;
        self.windows
            .iter()
            .map(|&(s, _)| (s % self.period + self.period - t) % self.period)
            .min()
            .unwrap_or(0)
    }

    /// Seconds from `now` until the open window closes (a later window
    /// that overlaps it is not counted); 0 while none is open, `None` for
    /// [`Timetable::always`].
    pub fn until_offline(&self, now: u64) -> Option<u64> {
        if self.period == 0 {
            return None;
        }
        let t = now % self.period;
        Some(
            self.windows
                .iter()
                .filter_map(|&(s, l)| {
                    let into = (t + self.period - s % self.period) % self.period;
                    (into < l).then(|| l - into)
                })
                .max()
                .unwrap_or(0),
        )
    }

    /// Hours online in one cycle, for the log.
    pub fn hours(&self) -> f64 {
        self.windows.iter().map(|&(_, l)| l as f64).sum::<f64>() / 3600.0
    }
}

/// **A docked decision point** (R-SOCIETY §2.2): docked, not stranded
/// away from home, and [`Society::decide_every`] since the last decision.
/// Never in space or in transit, so a trip keeps its role.
pub fn decision_point(
    place: Place,
    stranded: bool,
    last: Option<Instant>,
    now: Instant,
    every: Duration,
) -> bool {
    matches!(place, Place::Docked { .. })
        && !stranded
        && last.is_none_or(|t| now.saturating_duration_since(t) >= every)
}

/// The role that plays career `name` in `table`; the default role for a
/// career no row names or no role plays.
pub fn role_of(table: &[Career], name: &str) -> RoleRef {
    career::find(table, name)
        .and_then(|c| c.role)
        .unwrap_or_else(roles::default)
}

/// **A bot's society**: its mind, shared so `eve-bots` can print the
/// population's `[soc]` summary; the career table and the signals it
/// decides on; where its state file goes; its sessions.
#[derive(Debug, Clone)]
pub struct Society {
    pub mind: Arc<Mutex<Mind>>,
    pub table: &'static [Career],
    pub signals: Arc<Signals>,
    /// `eve-bots --state DIR`; `None`: the mind lives for this run only.
    pub state: Option<PathBuf>,
    pub timetable: Timetable,
    /// [`DECIDE_EVERY`]; tests use zero.
    pub decide_every: Duration,
    /// Bots logged on now, across the process (the `[soc]` line's
    /// `online`).
    pub online: Arc<AtomicUsize>,
    /// The process's corp minds (SOC-C4); `None`: no corps, no diplomacy.
    pub hall: Option<Arc<crate::corpmind::Hall>>,
    /// DOC-3: where members post what they see (sightings, kill
    /// evidence) for a leader's picture; `None`: nobody posts. One shared
    /// board per process, set beside `hall`.
    pub board: Option<Arc<crate::doctrine_sense::Board>>,
}

impl Society {
    /// A society for `mind`: always online, [`DECIDE_EVERY`], no state
    /// directory, its own online count.
    pub fn new(mind: Mind, table: &'static [Career], signals: Signals) -> Society {
        Society {
            mind: Arc::new(Mutex::new(mind)),
            table,
            signals: Arc::new(signals),
            state: None,
            timetable: Timetable::always(),
            decide_every: DECIDE_EVERY,
            online: Arc::new(AtomicUsize::new(0)),
            hall: None,
            board: None,
        }
    }

    /// The mind of `name` from `state` ([`crate::state::load_or_seed`]),
    /// or seeded when there is no directory, and where it came from.
    pub fn open(
        name: &str,
        seed: u64,
        table: &'static [Career],
        signals: Signals,
        state: Option<&Path>,
        now: u64,
    ) -> (Society, crate::state::Source) {
        let (mind, source) = match state {
            Some(dir) => crate::state::load_or_seed(dir, name, seed, table, &signals, now),
            None => (
                Mind::seeded(name, seed, table, &signals, now),
                crate::state::Source::Fresh,
            ),
        };
        let mut soc = Society::new(mind, table, signals);
        soc.state = state.map(Path::to_path_buf);
        (soc, source)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Mind> {
        self.mind.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The role the mind's career plays.
    pub fn role(&self) -> RoleRef {
        role_of(self.table, &self.lock().career)
    }

    /// The mind's career now.
    pub fn career(&self) -> String {
        self.lock().career.clone()
    }

    /// The home station the mind keeps (SOC-SIG).
    pub fn home(&self) -> Option<i64> {
        self.lock().home
    }

    /// The mind's home is now `station` (PROG-PVE2: the bot moved to richer
    /// ground and a restart must not send it back).
    pub fn move_home(&self, station: i64) {
        self.lock().home = Some(station);
    }

    /// Keep `station` as the mind's home if it has none; true if it did not.
    pub fn adopt_home(&self, station: i64) -> bool {
        let mut m = self.lock();
        if m.home.is_some() {
            return false;
        }
        m.home = Some(station);
        true
    }

    /// Write the mind to its state file, if there is a directory.
    pub fn save(&self) -> std::io::Result<()> {
        match &self.state {
            Some(dir) => crate::state::save(dir, &self.lock()).map(|_| ()),
            None => Ok(()),
        }
    }
}

/// One bot's settings.
#[derive(Debug, Clone)]
pub struct BotConfig {
    /// The character to play, by name. **Never sent as a token** (ruling 2):
    /// a visible name is not a secret.
    pub name: String,
    /// The SSO token the bot sends: its line in `eve-bots --tokens FILE`,
    /// else [`DEV_TOKEN`] (IP41, `account-spec.md` §7 step 3).
    pub token: String,
    /// The server, as an IP literal: nothing here resolves a name.
    pub addr: SocketAddr,
    pub policy: Policy,
    pub pace: Pace,
    /// Wait this long before the first login.
    pub start_delay: Duration,
    /// REL-FIX: the `--friends` list; contacts are set at each login.
    pub friends: Option<Arc<crate::friends::Friends>>,
    /// Stop after this long; `None` runs until [`Handle::stop`].
    pub run_for: Option<Duration>,
    /// Print each act-log line and the summary to stdout.
    pub echo: bool,
    /// Try the server's chat after login (it may be off; that is fine).
    pub chat: bool,
    /// The chat server's port: [`crate::chat::PORT`], as the client dials,
    /// except in tests.
    pub chat_port: u16,
    /// `--chat-host`: dial this address for chat instead of the one the
    /// server advertises (bots in the server's network namespace cannot
    /// reach the jail-side address a client is told). `None`: advertised.
    pub chat_host: Option<std::net::IpAddr>,
    /// The bot marker this bot adds itself to and reads (ruling 5).
    pub marker: BotMarker,
    /// The system this bot belongs in ([`bot_homes`]); `None`: wherever
    /// its character is.
    pub home_system: Option<i64>,
    /// The client's static data, `eve-bots --content DIR`
    /// ([`crate::hulls::open`]): where hull (FX10) and module (FX11) numbers come
    /// from when the server's own row is not read. `None`: the invented fixture.
    pub content: Option<Arc<eve_server::content::Content>>,
    /// Society mode (SOC-A2): the mind sets the role, and the timetable
    /// the sessions. `None`: the role is [`Policy::role`] for good.
    pub society: Option<Society>,
    /// The station this bot belongs at; `None`: the first it docks at in
    /// this run. A bot that logs on anywhere else flies home first.
    pub home_station: Option<i64>,
    /// The price index every bot of the process shares
    /// ([`crate::pricing::PriceIndexCache`], `eve-bots` makes one): 300
    /// logins make one `GetAverageMarketPrices` call. `None` (tests): each
    /// bot asks the server itself.
    pub price_cache: Option<Arc<crate::pricing::PriceIndexCache>>,
}

impl BotConfig {
    /// A bot at `pace` with that pace's policy, no delay, no deadline,
    /// quiet.
    pub fn new(name: &str, addr: SocketAddr, pace: Pace) -> BotConfig {
        BotConfig {
            name: name.to_owned(),
            token: DEV_TOKEN.to_owned(),
            addr,
            policy: pace.policy(),
            pace,
            start_delay: Duration::ZERO,
            friends: None,
            run_for: None,
            echo: false,
            chat: true,
            chat_port: crate::chat::PORT,
            chat_host: None,
            marker: BotMarker::default(),
            home_system: None,
            content: None,
            society: None,
            home_station: None,
            price_cache: None,
        }
    }

    /// The same bot in society mode: its role is the mind's career's.
    pub fn with_society(mut self, society: Society) -> BotConfig {
        self.policy.role = society.role();
        self.society = Some(society);
        self
    }

    /// The same bot, sending `token` as its SSO token.
    pub fn with_token(mut self, token: &str) -> BotConfig {
        self.token = token.to_owned();
        self
    }

    /// The same bot, playing `role`.
    pub fn with_role(mut self, role: RoleRef) -> BotConfig {
        self.policy.role = role;
        self
    }
}

/// What an act came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    /// [`Error::NotSent`]: the client's own precondition said no.
    NotSent(String),
    /// [`Error::Refused`]: sent, answered, not done.
    Refused(String),
    /// `Io` or `Protocol`: the connection went with it.
    Failed(String),
}

impl Verdict {
    pub fn is_ok(&self) -> bool {
        *self == Verdict::Ok
    }
}

/// One act-log line.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Since the bot started.
    pub at: Duration,
    pub kind: ActKind,
    /// The act and its operands, for humans and for tests that need an id:
    /// [`Entry::ids`] reads them back.
    pub what: String,
    /// The ids the act named, in order (`Mine` is `[laser, rock]`,
    /// `WarpToMember` is `[character]`).
    pub ids: Vec<i64>,
    pub verdict: Verdict,
}

/// The outside's hold on a running bot: its log, its off switch, and its
/// socket.
#[derive(Clone)]
pub struct Handle {
    pub name: String,
    log: Arc<Mutex<VecDeque<Entry>>>,
    stop: Arc<AtomicBool>,
    socket: Arc<Mutex<Option<TcpStream>>>,
    seen: Arc<Mutex<Option<Observation>>>,
    /// BOT-CTL: a station the GM sent this bot to (`/bots bring`); the
    /// bot takes it on its next turn and flies there.
    brought: Arc<Mutex<Option<i64>>>,
    /// GANG-SUMMON (`/bots come`): `(system, character)` the bot is called
    /// to; it takes it on its next turn.
    summon_cmd: Arc<Mutex<Option<(i64, i64)>>>,
    /// BP-CAMP (`camp <group> <system>`): the system whose gate the bot's
    /// gang is told to hold; taken on its next turn.
    camp_cmd: Arc<Mutex<Option<i64>>>,
    /// FC-3: commands for a wing bot (`/bots hold`, `/bots join`), taken
    /// on its next turn.
    orders: Arc<Mutex<VecDeque<eve_server::wing_words::WingCmd>>>,
    /// FC-3: this bot has a wing seat, and the FC it was spawned for.
    wing_seat: bool,
    wing_fc: Option<i64>,
    /// FC-3: the fleet boss the driver last saw, for a seat with no FC.
    boss_pub: Arc<Mutex<Option<i64>>>,
    /// PULL-LEASE: a lend (`Some(Some(station))`) or a recall (`Some(None)`)
    /// the bot has not yet taken; see [`Handle::lend`].
    lend_cmd: Arc<Mutex<Option<Option<i64>>>>,
    /// PULL-LEASE: the station the bot is on loan to now, as the driver
    /// last saw it (the planner reads it; the driver owns it).
    lease_pub: Arc<Mutex<Option<i64>>>,
    /// PULL-LEASE: the name of the role the bot plays now (a society mind
    /// may change it), written by the driver each turn.
    role_pub: Arc<Mutex<&'static str>>,
    /// NAMES: the character the bot plays, once selected (`/bots list`).
    /// Empty until the first login; `name` above is the control key.
    character: Arc<Mutex<Option<String>>>,
    /// BOT-ADMIN: a GM asked this bot to pause (`/bots pause`); the driver
    /// parks at its next docked turn and stays until it is cleared.
    paused: Arc<AtomicBool>,
    /// BOT-ADMIN: the driver is parked now (docked and idle by request).
    parked: Arc<AtomicBool>,
    /// BUBBLE: the bubble planner released this bot (`eve_bot::bubble`): it
    /// goes home, docks and logs off, and stays off until [`Handle::wake`].
    released: Arc<AtomicBool>,
    /// BUBBLE: the planner watches this bot, so its waits are short enough
    /// to notice a release ([`BUBBLE_WAIT`]).
    bubbled: Arc<AtomicBool>,
    /// BUBBLE: the bot is logged on now (set after its login, cleared when
    /// the connection ends).
    live: Arc<AtomicBool>,
    /// BUBBLE: the bot's sessions, to tell a wake it cannot answer.
    timetable: Option<Arc<Timetable>>,
}

impl Handle {
    /// The character name the bot plays (`None` before its first login).
    pub fn character(&self) -> Option<String> {
        self.character.lock().ok().and_then(|c| c.clone())
    }

    /// BOT-ADMIN (`/bots pause`): park at the next docked turn. A bot in
    /// space or in warp carries on until it docks, never stops mid-warp.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    /// BOT-ADMIN (`/bots resume`): take the bot's turns again.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    /// Whether a pause was asked for and not yet lifted.
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Whether the bot has actually parked (docked, doing nothing).
    pub fn is_parked(&self) -> bool {
        self.parked.load(Ordering::SeqCst)
    }

    /// BUBBLE: release the bot (log it off at the next safe point). The
    /// absence is settled as any log-off is; see `bubble.rs`, "No paper".
    pub fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
    }

    /// BUBBLE: the planner takes this bot in (`Runner::new`).
    pub fn watch(&self) {
        self.bubbled.store(true, Ordering::SeqCst);
    }

    /// BUBBLE: let a released bot log on again.
    pub fn wake(&self) {
        self.released.store(false, Ordering::SeqCst);
    }

    /// BUBBLE: whether the bubble has released this bot.
    pub fn released(&self) -> bool {
        self.released.load(Ordering::SeqCst)
    }

    /// BUBBLE: whether the bot is logged on.
    pub fn is_live(&self) -> bool {
        self.live.load(Ordering::SeqCst)
    }

    /// BUBBLE: whether a session window is open at `unix_now` (always, for a
    /// bot without a timetable).
    pub fn in_window(&self, unix_now: u64) -> bool {
        self.timetable.as_ref().is_none_or(|t| t.online(unix_now))
    }

    /// BOT-CTL (`/bots bring`): send the bot to `station`. It becomes its
    /// home, and the bot flies there on its next turn (the way home needs
    /// the gate graph to cross systems).
    pub fn bring(&self, station: i64) {
        if let Ok(mut b) = self.brought.lock() {
            *b = Some(station);
        }
    }

    /// GANG-SUMMON (`/bots come`): call the bot to `character`, who is in
    /// `system`. A gang bot hands it to its gang (the FC leads everyone
    /// there); any other bot undocks, flies and warps to him on its own
    /// ([`SUMMON_HOLD_S`]).
    pub fn summon(&self, system: i64, character: i64) {
        if let Ok(mut s) = self.summon_cmd.lock() {
            *s = Some((system, character));
        }
    }

    /// BP-CAMP: tell the bot's gang to camp the gate of `system`. A bot
    /// outside a gang ignores it (a lone camper holds the gate of the system
    /// it is in).
    pub fn camp(&self, system: i64) {
        if let Ok(mut s) = self.camp_cmd.lock() {
            *s = Some(system);
        }
    }

    /// FC-3: whether this bot is a wing bot.
    pub fn is_wing(&self) -> bool {
        self.wing_seat
    }

    /// FC-3: whether this wing bot follows `fc`: it was spawned for him, or
    /// its fleet's boss is him.
    pub fn follows(&self, fc: i64) -> bool {
        self.wing_seat
            && (self.wing_fc == Some(fc) || self.boss_pub.lock().ok().and_then(|b| *b) == Some(fc))
    }

    /// FC-3: queue a command for the wing bot's next turn (bounded).
    pub fn order(&self, cmd: eve_server::wing_words::WingCmd) {
        if let Ok(mut q) = self.orders.lock() {
            if q.len() >= crate::wing::CMD_QUEUE {
                q.pop_front();
            }
            q.push_back(cmd);
        }
    }

    pub(crate) fn take_orders(&self) -> Vec<eve_server::wing_words::WingCmd> {
        self.orders
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default()
    }

    /// PULL-LEASE: lend the bot to `station` for a while. It flies there
    /// like a brought bot, but its home station stays what it was, and
    /// [`Handle::recall`] sends it back. Unlike [`Handle::bring`], this is
    /// the bubble's, not a GM's.
    pub fn lend(&self, station: i64) {
        if let Ok(mut c) = self.lend_cmd.lock() {
            *c = Some(Some(station));
        }
    }

    /// PULL-LEASE: end a loan; the bot flies home on its next turn.
    pub fn recall(&self) {
        if let Ok(mut c) = self.lend_cmd.lock() {
            *c = Some(None);
        }
    }

    /// PULL-LEASE: the station the bot is on loan to, if any.
    pub fn leased_to(&self) -> Option<i64> {
        self.lease_pub.lock().ok().and_then(|l| *l)
    }

    /// PULL-LEASE: the role the bot plays now.
    pub fn role_name(&self) -> &'static str {
        self.role_pub.lock().map(|r| *r).unwrap_or("")
    }

    fn take_lend(&self) -> Option<Option<i64>> {
        self.lend_cmd.lock().ok().and_then(|mut c| c.take())
    }

    fn take_summon(&self) -> Option<(i64, i64)> {
        self.summon_cmd.lock().ok().and_then(|mut s| s.take())
    }

    fn take_camp(&self) -> Option<i64> {
        self.camp_cmd.lock().ok().and_then(|mut s| s.take())
    }

    fn take_brought(&self) -> Option<i64> {
        self.brought.lock().ok().and_then(|mut b| b.take())
    }

    /// The act log's last line, without copying the log (`/bots list`).
    pub fn last_entry(&self) -> Option<Entry> {
        self.log.lock().ok().and_then(|l| l.back().cloned())
    }

    /// What the bot saw at its last turn: for tests and for a human
    /// wondering what it thinks it is doing.
    pub fn observation(&self) -> Option<Observation> {
        self.seen.lock().ok().and_then(|o| o.clone())
    }

    /// The act log so far.
    pub fn log(&self) -> Vec<Entry> {
        self.log
            .lock()
            .map(|l| l.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Ask the bot to stop; it does at its next turn.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.kick();
    }

    /// **Cut the bot's connection** from outside, as a network fault
    /// would: its next read fails and the supervisor takes over (T7).
    pub fn kick(&self) {
        if let Ok(guard) = self.socket.lock()
            && let Some(s) = guard.as_ref()
        {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

/// Start a bot on its own thread.
pub fn spawn(config: BotConfig) -> (Handle, JoinHandle<()>) {
    let handle = Handle {
        name: config.name.clone(),
        log: Arc::new(Mutex::new(VecDeque::new())),
        stop: Arc::new(AtomicBool::new(false)),
        socket: Arc::new(Mutex::new(None)),
        seen: Arc::new(Mutex::new(None)),
        brought: Arc::new(Mutex::new(None)),
        summon_cmd: Arc::new(Mutex::new(None)),
        camp_cmd: Arc::new(Mutex::new(None)),
        orders: Arc::new(Mutex::new(VecDeque::new())),
        wing_seat: config.policy.wing.is_some(),
        wing_fc: config.policy.wing.as_ref().and_then(|w| w.fc),
        boss_pub: Arc::new(Mutex::new(None)),
        lend_cmd: Arc::new(Mutex::new(None)),
        lease_pub: Arc::new(Mutex::new(None)),
        role_pub: Arc::new(Mutex::new(config.policy.role.name)),
        character: Arc::new(Mutex::new(None)),
        paused: Arc::new(AtomicBool::new(false)),
        parked: Arc::new(AtomicBool::new(false)),
        released: Arc::new(AtomicBool::new(false)),
        bubbled: Arc::new(AtomicBool::new(false)),
        live: Arc::new(AtomicBool::new(false)),
        timetable: config
            .society
            .as_ref()
            .map(|s| Arc::new(s.timetable.clone())),
    };
    let inner = handle.clone();
    let thread = std::thread::Builder::new()
        .name(config.name.clone())
        .spawn(move || Bot::new(config, inner).run())
        .expect("a thread for the bot");
    (handle, thread)
}

/// Why a connection's inner loop ended without an error.
enum Exit {
    /// Stopped, or out of time.
    Done,
    /// The brain asked for a fresh connection.
    Reconnect,
    /// The session's window is over (society mode): log on at the next.
    LogOff,
}

/// What the driver tracks beside the [`Observation`]: the parts of the
/// world that become park and module views, rebuilt on every connection.
#[derive(Default)]
struct Tracking {
    /// MOVE-DEDUPE: the last follow or orbit sent, `(ship, orbit, target,
    /// range)`; a repeat of it while the ship's ball is still tethered to
    /// that target is not sent again ([`repeats_move`]). Rebuilt with the
    /// connection, so a reconnect sends afresh.
    last_move: Option<(i64, bool, i64, f64)>,
    /// The session-change count last seen, to notice a dock or a board.
    changes: u64,
    /// The hangar and ship need listing again.
    docked_dirty: bool,
    /// The next skill-queue top-up ([`crate::skill_plan::FILL_EVERY`]).
    skills_next: Option<Instant>,
    /// The hold changed in space: list the ship again.
    cargo_dirty: bool,
    /// Other ships' lasers: `ship -> rock`, from `OnSpecialFX`.
    lasers: BTreeMap<i64, i64>,
    /// The select screen, by id, for an inviter's name.
    names: BTreeMap<i64, String>,
    /// NPC guns: `NPC -> the ship it is shooting`, from `OnSpecialFX`.
    aggressors: BTreeMap<i64, i64>,
    /// The park state (its `states` count) an adrift resync was last tried
    /// for: one try per state, so a refusal does not repeat every turn.
    adrift_tried: Option<u64>,
    /// The ship's attribute 76 as the server served it (FX10,
    /// [`crate::hulls`]): `(ship, metres)`, read once per ship.
    served_lock_range: Option<(i64, Option<f64>)>,
    /// The lock range last logged and its source, so a change is one line.
    lock_logged: Option<(i64, crate::hulls::Source)>,
    /// FE-DEMAND: ISK per m3 of each ore at today's mineral values, lifted
    /// by the bids on them; read at the career decision, used to pick a rock.
    ore_values: BTreeMap<i64, f64>,
    /// Each module's numbers' source as last logged (FX11): one line per
    /// module and source.
    module_logged: BTreeMap<i64, crate::hulls::Source>,
    /// The small rounds each gun type takes (BOT-TUNE,
    /// [`crate::ammo::rounds_for`]), worked out once per type.
    round_types: BTreeMap<i64, Vec<i64>>,
    /// R-REL-3: the last `NeedBackup` heard, not yet weighed.
    backup: Option<crate::fleet::BackupCall>,
    /// THREAT: other pilots' offensive guns: `shooter ship -> (target, when
    /// last heard)`, from `OnSpecialFX` (NPCs are in `aggressors`).
    player_fire: BTreeMap<i64, (i64, Instant)>,
    /// CLOAK-2: `effects.Uncloak` heard for a ship, and when
    /// ([`crate::threat::decloak_cause`]).
    decloaked: BTreeMap<i64, Instant>,
    /// THREAT: the ships locking this one, as `GetTargeters` last said, and
    /// when it was asked.
    lockers: BTreeSet<i64>,
    lockers_at: Option<Instant>,
    /// BOT-CAP: the capacitor, the activation hold and the neuts on us.
    cap: crate::cap::Watch,
    /// BOT-SCRAM: who warp-scrambles this ship, and when a held Reconnect
    /// was last logged.
    scram: crate::scram::Sense,
    /// BP-COUNTER: smartbombs and drives seen on other ships.
    counter: crate::counter::Sense,
    /// BP-HOLD: the last warp refused inside a warp bubble.
    warp_bubble: crate::warp_bubble::Sense,
    reconnect_held_at: Option<Instant>,
    /// FC-2: the Entosis Link module types (effect 6063), read once from
    /// the content bundle; only a wing bot asks.
    link_types: Option<BTreeSet<i64>>,
}

struct Bot {
    config: BotConfig,
    handle: Handle,
    memory: Memory,
    obs: Observation,
    track: Tracking,
    started: Instant,
    rng: u64,
    /// Every stop ever seen, numbered across connections so the brain acts
    /// on each once.
    stops: u64,
    reconnects: u32,
    ore_unloaded: i64,
    /// ISK the miner's sales brought in (FX6).
    isk_sold: f64,
    /// Society mode: when the career was last weighed.
    last_decision: Option<Instant>,
    /// Society mode: the wallet and the Unix second at the last decision
    /// on this connection, for the income ledger.
    ledger: crate::earnings::Tracker,
    /// The mining hull's income multiplier the miner ledger was last scaled
    /// to (PROG-MINE, [`crate::mine_ladder::income_mult`]).
    mining_mult: f64,
    /// Society mode: when the bot first saw its window closed in space.
    logoff_since: Option<Instant>,
    /// DEV-MODE: the dev setup ran on this connection.
    dev_done: bool,
    /// DEV-MODE: `/devhome` was sent (once a run).
    dev_homed: bool,
    /// DEV-MODE: a boss's fleet bookkeeping.
    fleeter: crate::dev::Fleeter,
    /// This bot counts in [`Society::online`] now.
    counted: bool,
    /// The first station this bot docked at in this run: home.
    home_station: Option<i64>,
    /// An errand ended away from home: `(home, tries so far)`.
    stranded: Option<(i64, u32)>,
    /// GANG-SUMMON: a character this (non-gang) bot was called to.
    summoned: Option<Summoned>,
    /// PULL-LEASE: the station the bot is on loan to ([`Handle::lend`]);
    /// `home_station` stays its base meanwhile.
    lease: Option<i64>,
    /// What the act just executed did, for its log line (FX6).
    note: Option<String>,
    /// SKILLGATE: the act just executed asks that the rest of this turn's
    /// acts be dropped (an explorer with no launcher it may use must not
    /// undock).
    defer_rest: bool,
    last_summary: Instant,
    chat: Option<Chat>,
    last_drain: Instant,
    /// Character names learned for the chat's phrase slots (CHATTER-2),
    /// and the ids already asked about, so a nameless one is asked once.
    chat_names: BTreeMap<i64, String>,
    /// This bot's character, once connected: the key of its killmail fights.
    my_char: Option<i64>,
    /// CLONE-HOME: the medical clone was set at the base this session.
    clone_set: bool,
    /// BP-KIT: the kit drill's GM lines were said this session.
    drilled: bool,
    /// Numbers the fights handed to the chatter ([`crate::chatter::Outcome`]).
    fight_seq: u64,
    names_asked: BTreeSet<i64>,
    /// DOC-3: damage taken by type and the kill reports read.
    sensor: crate::doctrine_sense::Sensor,
    /// DOC-3: when the last sighting went to the board.
    last_posted: Option<Instant>,
    pacer: Arc<Mutex<Pacer>>,
}

impl Bot {
    fn new(mut config: BotConfig, handle: Handle) -> Bot {
        if let Some(soc) = &config.society {
            config.policy.role = soc.role();
        }
        let memory = Memory::new(&config.name, config.policy.clone());
        let now = Instant::now();
        Bot {
            rng: memory.seed.rotate_left(17) | 1,
            memory,
            obs: Observation::empty(now),
            track: Tracking::default(),
            started: now,
            stops: 0,
            reconnects: 0,
            ore_unloaded: 0,
            isk_sold: 0.0,
            last_decision: None,
            ledger: Default::default(),
            mining_mult: 1.0,
            logoff_since: None,
            dev_done: false,
            dev_homed: false,
            fleeter: crate::dev::Fleeter::default(),
            counted: false,
            home_station: config
                .home_station
                .or_else(|| config.society.as_ref().and_then(Society::home)),
            stranded: None,
            summoned: None,
            lease: None,
            note: None,
            defer_rest: false,
            last_summary: now,
            chat: None,
            last_drain: now,
            chat_names: BTreeMap::new(),
            my_char: None,
            clone_set: false,
            drilled: false,
            fight_seq: 0,
            names_asked: BTreeSet::new(),
            sensor: Default::default(),
            last_posted: None,
            pacer: Arc::new(Mutex::new(if config.policy.quick {
                Pacer::quick()
            } else {
                Pacer::standard()
            })),
            config,
            handle,
        }
    }

    fn expired(&self) -> bool {
        self.handle.stopped()
            || self
                .config
                .run_for
                .is_some_and(|d| self.started.elapsed() >= d)
    }

    fn jitter(&mut self, (lo, hi): (Duration, Duration)) -> Duration {
        let span = hi.saturating_sub(lo);
        if span.is_zero() {
            return lo;
        }
        lo + span.mul_f64(brain::unit(brain::xorshift(&mut self.rng)))
    }

    /// Sleep `d` unless stopped first. Only for when there is no socket.
    fn idle(&self, d: Duration) {
        let until = Instant::now() + d;
        while !self.expired() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            std::thread::sleep(left.min(Duration::from_millis(100)));
        }
    }

    fn record(&mut self, kind: ActKind, what: String, ids: Vec<i64>, verdict: Verdict) {
        self.record_as(kind, what, ids, verdict, self.config.echo);
    }

    /// [`Bot::record`], printing only when `echo` (a `[soc]` line prints
    /// itself instead).
    fn record_as(
        &mut self,
        kind: ActKind,
        what: String,
        ids: Vec<i64>,
        verdict: Verdict,
        echo: bool,
    ) {
        let t0 = Instant::now();
        let at = self.started.elapsed();
        if echo {
            let secs = at.as_secs_f64();
            let tail = match &verdict {
                Verdict::Ok => "ok".to_owned(),
                Verdict::NotSent(w) => format!("not sent: {w}"),
                Verdict::Refused(w) => format!("refused: {w}"),
                Verdict::Failed(w) => format!("FAILED: {w}"),
            };
            let tag = if kind == ActKind::Missing {
                format!("MISSING: {what}")
            } else {
                what.clone()
            };
            println!(
                "{} +{:02}:{:04.1} {:<11} {} -> {}",
                self.config.name,
                (secs / 60.0).floor() as u64,
                secs % 60.0,
                kind.name(),
                tag,
                tail
            );
        }
        if let Ok(mut log) = self.handle.log.lock() {
            if log.len() >= LOG_CAP {
                log.pop_front();
            }
            log.push_back(Entry {
                at,
                kind,
                what,
                ids,
                verdict,
            });
        }
        crate::stats::LOG.add(t0.elapsed());
    }

    /// The supervisor.
    fn run(mut self) {
        // SKILLGATE: the requirement reads (`acquire::flyable`) are this
        // thread's, wherever the first act of the run is.
        if let Some(c) = &self.config.content {
            crate::acquire::use_content(c);
        }
        self.idle(self.config.start_delay);
        let mut backoff = BACKOFF_FIRST;
        while !self.expired() {
            if !self.await_window() {
                break;
            }
            let began = Instant::now();
            let ended = self.session();
            if let Some(me) = self.my_char {
                set_departed(self.config.addr, me, true);
            }
            if began.elapsed() >= HEALTHY {
                backoff = BACKOFF_FIRST;
            }
            *self.handle.socket.lock().expect("socket slot") = None;
            self.handle.live.store(false, Ordering::SeqCst);
            if self.counted {
                self.counted = false;
                if let Some(soc) = &self.config.society {
                    soc.online.fetch_sub(1, Ordering::SeqCst);
                }
            }
            match ended {
                Ok(Exit::Done) => break,
                Ok(Exit::LogOff) => {
                    self.save_mind();
                    backoff = BACKOFF_FIRST;
                    continue;
                }
                Ok(Exit::Reconnect) => {
                    self.reconnects += 1;
                }
                Err(e) => {
                    if self.expired() {
                        break;
                    }
                    self.reconnects += 1;
                    self.record(
                        ActKind::Lost,
                        "the connection".to_owned(),
                        vec![],
                        Verdict::Failed(e.to_string()),
                    );
                }
            }
            let wait = backoff + self.jitter((Duration::ZERO, backoff / 2));
            self.idle(wait);
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
        self.save_mind();
    }

    /// **Outside a session, wait for the next** (society mode): one
    /// `session` line saying how long, then idle. `false` when the bot was
    /// stopped or ran out of time meanwhile.
    fn await_window(&mut self) -> bool {
        // BUBBLE: a released bot stays off until woken.
        let mut said_released = false;
        while self.handle.released() && !self.expired() {
            if !said_released {
                said_released = true;
                self.record(
                    ActKind::Session,
                    "offline: released by the bubble until woken".to_owned(),
                    vec![],
                    Verdict::Ok,
                );
            }
            self.idle(Duration::from_secs(1));
        }
        let Some(table) = self.config.society.as_ref().map(|s| s.timetable.clone()) else {
            return !self.expired();
        };
        let mut said = false;
        while !self.expired() {
            let wait = table.until_online(unix_now());
            if wait == 0 {
                return true;
            }
            if !said {
                said = true;
                self.record(
                    ActKind::Session,
                    format!("offline: the next session opens in {wait} s"),
                    vec![],
                    Verdict::Ok,
                );
            }
            self.idle(Duration::from_secs(wait));
        }
        false
    }

    /// Write the mind to its state file (society mode with `--state`); a
    /// failure is a `MISSING` line, never fatal.
    fn save_mind(&mut self) {
        let Some(soc) = &self.config.society else {
            return;
        };
        if let Err(e) = soc.save() {
            self.record(
                ActKind::Missing,
                format!("a state file that saves ({e})"),
                vec![],
                Verdict::Ok,
            );
        }
    }

    /// One connection, from login to its end.
    fn session(&mut self) -> Result<Exit> {
        let mut conn = Conn::connect(self.config.addr, READ_TIMEOUT)?;
        *self.handle.socket.lock().expect("socket slot") = Some(conn.socket()?);
        conn.keep_pushes();
        if let Some(cache) = self.config.price_cache.clone() {
            conn.share_price_index(cache);
        }
        if self.config.pace.paced() {
            let pacer = Arc::clone(&self.pacer);
            conn.gate_sends(move || {
                let wait = pacer
                    .lock()
                    .map(|mut p| p.admit(Instant::now()))
                    .unwrap_or_default();
                if !wait.is_zero() {
                    std::thread::sleep(wait);
                }
            });
        }
        // The token, never the name (ruling 2).
        conn.handshake_as(&self.config.token)?;
        let slots = conn.characters()?;
        // NAMES: the character is the key's own, else the account's only one
        // (a bot's character has a pool name; its key is the account's).
        let character = slots
            .iter()
            .find(|s| s.name == self.config.name)
            .or(match slots.as_slice() {
                [only] => Some(only),
                _ => None,
            })
            .map_or_else(|| self.config.name.clone(), |s| s.name.clone());
        conn.select_character_named(&character)?;
        // A reconnect keeps the hunter's status for the police net.
        conn.set_police_status(self.memory.hunt.status);
        if let Ok(mut c) = self.handle.character.lock() {
            *c = Some(character.clone());
        }
        self.record(
            ActKind::Connect,
            if character == self.config.name {
                format!("as {}", self.config.name)
            } else {
                format!("as {} ({character})", self.config.name)
            },
            conn.session().char_id().into_iter().collect(),
            Verdict::Ok,
        );
        if let (Some(me), Ok(mut bots)) = (conn.session().char_id(), self.config.marker.lock()) {
            bots.insert(me);
            set_departed(self.config.addr, me, false);
        }
        self.my_char = conn.session().char_id();
        self.clone_set = false;
        self.drilled = false;
        // REL-FIX: personal contacts on the owner's friends, so the client
        // shows them blue; the hall stops hunting them.
        if let Some(friends) = self.config.friends.clone() {
            let hall = self.config.society.as_ref().and_then(|s| s.hall.clone());
            let mind = self.config.society.as_ref().map(|s| s.mind.clone());
            match crate::friends::apply_with_mind(
                &mut conn,
                &friends,
                hall.as_deref(),
                mind.as_deref(),
                unix_now(),
            ) {
                Ok(n) => self.record(
                    ActKind::Session,
                    format!("friends: {n} personal contact(s) set"),
                    vec![],
                    Verdict::Ok,
                ),
                Err(e) => self.record(
                    ActKind::Missing,
                    format!("friends set ({e})"),
                    vec![],
                    Verdict::Ok,
                ),
            }
        }
        if let Some(soc) = &self.config.society {
            soc.online.fetch_add(1, Ordering::SeqCst);
            self.counted = true;
        }
        self.handle.live.store(true, Ordering::SeqCst);
        // The income ledger counts this connection's time only.
        self.ledger.reset();
        self.logoff_since = None;

        // **From scratch**: nothing of the last connection's world survives.
        self.obs = Observation::empty(Instant::now());
        self.track = Tracking {
            names: slots
                .into_iter()
                .map(|s| (s.character_id, s.name))
                .collect(),
            ..Tracking::default()
        };
        self.memory.setup_done = false;
        self.dev_done = false;
        self.observe_from_scratch(&mut conn)?;
        // Logged on away from home (a trip cut off with its connection, or
        // a home it was given): the way home comes first.
        // Unless a roaming trip's state waits (Y-ROAM): that carries on
        // from where the bot is, and flies home itself at the end.
        let resumes = crate::explore_roam::attach(
            &self.config.name,
            self.config
                .society
                .as_ref()
                .and_then(|s| s.state.as_deref()),
        );
        if let Some(home) = self.work_station()
            && conn.session().station_id() != Some(home)
            && self.stranded.is_none()
            && !resumes
        {
            self.stranded = Some((home, 0));
        }
        self.open_chat(&mut conn)?;
        // A home elsewhere: said once a connection. No act routes through
        // gates yet, so the bot plays where its character is.
        if let (Some(home), Some(here)) =
            (self.config.home_system, conn.session().solar_system_id())
            && home != here
        {
            self.record(
                ActKind::Missing,
                format!("a gate route to home system {home} (here {here})"),
                vec![home, here],
                Verdict::Ok,
            );
        }

        loop {
            if self.expired() {
                return Ok(Exit::Done);
            }
            self.summary();
            crate::stats::REFRESH.time(|| self.refresh(&mut conn))?;
            self.weigh_backup(&mut conn)?;
            // BOT-ADMIN: a paused bot parks here once it is docked (a bot
            // in space or in warp finishes its trip first). Its connection
            // is kept alive by the wait.
            if self.handle.is_paused() && matches!(self.obs.place, Place::Docked { .. }) {
                if !self.handle.is_parked() {
                    self.handle.parked.store(true, Ordering::SeqCst);
                    self.record(
                        ActKind::Wait,
                        "parked by a GM (/bots pause)".to_owned(),
                        vec![],
                        Verdict::Ok,
                    );
                }
                self.wait(&mut conn, Duration::from_secs(1))?;
                continue;
            }
            if self.handle.parked.swap(false, Ordering::SeqCst) {
                self.record(
                    ActKind::Wait,
                    "resumed by a GM (/bots resume)".to_owned(),
                    vec![],
                    Verdict::Ok,
                );
            }
            self.scan_threat(&mut conn)?;
            self.weigh_distress(&mut conn);
            // BOT-CTL: a GM sent this bot to a station (`/bots bring`).
            if let Some(station) = self.handle.take_brought() {
                self.home_station = Some(station);
                self.lease = None;
                if let Ok(mut l) = self.handle.lease_pub.lock() {
                    *l = None;
                }
                self.stranded = Some((station, 0));
                self.record(
                    ActKind::Travel,
                    format!("brought to station {station} by a GM"),
                    vec![station],
                    Verdict::Ok,
                );
            }
            // GANG-SUMMON: a GM called this bot to him (`/bots come`).
            if let Some((system, char_id)) = self.handle.take_summon() {
                self.take_summon(system, char_id);
            }
            // BP-CAMP: a GM told the gang to hold a gate.
            if let Some(system) = self.handle.take_camp()
                && let Some(seat) = self.memory.policy.gang.clone()
            {
                crate::roam_gang::camp(&seat, system);
            }
            // FC-3: the owner's words for the wing.
            for cmd in self.handle.take_orders() {
                crate::wing::queue(&mut self.memory, cmd);
            }
            // PULL-LEASE: the bubble lent this bot out, or called it back.
            if let Some(cmd) = self.handle.take_lend() {
                self.take_lend(cmd);
            }
            if self.summon_turn(&mut conn)? {
                continue;
            }
            if self.return_home(&mut conn)? {
                continue;
            }
            if let Some(seat) = &self.memory.policy.gang
                && let Some(me) = conn.session().char_id()
            {
                crate::roam_gang::register(seat, &self.memory.name, me);
            }
            if let Some(exit) = self.dev_turn(&mut conn)? {
                return Ok(exit);
            }
            self.wing_turn(&mut conn);
            if let Some(exit) = self.society_turn(&mut conn)? {
                return Ok(exit);
            }
            self.obs.cap = self.track.cap.scene(Instant::now());
            self.track.scram.on_ship(conn.session().ship_id());
            self.obs.scram = self.track.scram.view(Instant::now());
            // BP-COUNTER: the enemy's utility, and what the hunter does of it.
            {
                let k = self.memory.policy.counter;
                self.obs.counter = self.track.counter.view(Instant::now(), &k);
                let arm = self
                    .memory
                    .policy
                    .pvp
                    .as_ref()
                    .map_or(crate::hunt::Arm::Unknown, |d| {
                        d.arm_of(self.obs.ship.and_then(|s| s.type_id))
                    });
                crate::counter::steer(&mut self.obs, arm, &k);
            }
            (self.obs.warp_bubble.held, self.obs.warp_bubble.field) =
                self.track.warp_bubble.state(Instant::now());
            let (memory, acts) =
                crate::stats::DECIDE.time(|| brain::decide(&self.memory, &self.obs));
            self.memory = memory;
            // A call answered once: the warp is the answer.
            self.obs.help = None;
            self.post_distress(&mut conn);
            // MOVE-DEDUPE: a turn that only steers (follow/orbit) and never
            // waits would re-decide at once; it ends with the short wait.
            let steer_only = !acts.is_empty()
                && acts
                    .iter()
                    .all(|a| matches!(a, Act::Follow { .. } | Act::Orbit { .. }));
            for act in acts {
                match act {
                    Act::Wait(d) => self.wait(&mut conn, d)?,
                    Act::Reconnect if !crate::scram::reconnect_allowed(&self.memory, &self.obs) => {
                        // BOT-SCRAM: the server answers a dropped session in
                        // space with an emergency warp, so a reconnect under
                        // a scramble or fire is an escape. Hold, go home
                        // when it can.
                        let now = Instant::now();
                        if self
                            .track
                            .reconnect_held_at
                            .is_none_or(|t| now.saturating_duration_since(t) >= RECONNECT_HELD_LOG)
                        {
                            self.track.reconnect_held_at = Some(now);
                            self.record(
                                ActKind::Wait,
                                "reconnect held: scrambled or under fire".to_owned(),
                                vec![],
                                Verdict::Ok,
                            );
                        }
                        self.memory.homing = Some(brain::Homing::Recovery);
                        self.wait(&mut conn, brain::SHORT_WAIT)?;
                    }
                    Act::Reconnect => {
                        self.record(
                            ActKind::Reconnect,
                            "fresh connection".to_owned(),
                            vec![],
                            Verdict::Ok,
                        );
                        return Ok(Exit::Reconnect);
                    }
                    Act::Missing(what) => {
                        self.record(ActKind::Missing, what.to_owned(), vec![], Verdict::Ok);
                    }
                    act => {
                        // FAST-KIT: a quick bot thinks no longer than its prep.
                        let react = if self.memory.policy.quick {
                            Duration::ZERO
                        } else {
                            self.jitter(self.config.pace.reaction())
                        };
                        if !react.is_zero() {
                            self.wait_all(&mut conn, react)?;
                        }
                        self.act(&mut conn, act)?;
                    }
                }
                self.absorb_all(&mut conn);
                if std::mem::take(&mut self.defer_rest) {
                    break;
                }
            }
            if steer_only {
                self.wait(&mut conn, brain::SHORT_WAIT)?;
            }
        }
    }

    /// **DEV-MODE** (`eve-bots --roster`, [`crate::dev`]): once a connection,
    /// docked, grant the skills, join the side's corporation, move to the
    /// roster's system (one reconnect) and buy the hull and its fit from the
    /// dev market the closed economy's way; and a boss's fleet invites.
    /// Every step is a line in the act log; a refusal is a line, never a
    /// stop.
    fn dev_turn(&mut self, conn: &mut Conn) -> Result<Option<Exit>> {
        let Some(dev) = self.memory.policy.dev.clone() else {
            return Ok(None);
        };
        if let Some(c) = &self.config.content {
            crate::acquire::use_content(c);
        }
        let me = conn.session().char_id().unwrap_or(0);
        if me != 0 {
            dev.hall.register(&dev.group, me);
        }
        let docked = matches!(self.obs.place, Place::Docked { .. });
        if !self.dev_done && docked {
            // FAST-KIT: a home that is the system the character already sits
            // in (a numeric id) is no move, so no relog.
            let here = conn.session().solar_system_id();
            if let Some(home) = dev
                .home
                .clone()
                .filter(|_| !self.dev_homed)
                .filter(|h| h.parse::<i64>().ok().is_none_or(|id| Some(id) != here))
            {
                self.dev_homed = true;
                let line = format!("/devhome {home}");
                let out = crate::gm::slash(conn, &line)?;
                let verdict = match out.refusal() {
                    None => Verdict::Ok,
                    Some(key) => Verdict::Refused(key),
                };
                self.record(ActKind::Slash, line, vec![], verdict);
                // The new station is home; the connection starts again there.
                self.home_station = None;
                return Ok(Some(Exit::Reconnect));
            }
            self.dev_done = true;
            let mut lines = dev.skill_lines();
            lines.push(format!("/devcorp {}", dev.ticker));
            // A dev-spawned hunter bought its fit here and never reaches
            // `hunt::setup` (`/bots` bots run closed), so the tackle comes by
            // the same GM hand, as `camp_dock` does for probes. A market bot
            // never gets it: this path runs only under `policy.dev`.
            lines.extend(tackle_lines(
                &self.memory.policy,
                std::env::var(crate::pvp_kit::DRILL_ENV).ok().as_deref(),
                dev.is_foe(),
            ));
            for line in lines {
                let out = crate::gm::slash(conn, &line)?;
                let verdict = match out.refusal() {
                    None => Verdict::Ok,
                    Some(key) => Verdict::Refused(key),
                };
                self.record(ActKind::Slash, line, vec![], verdict);
            }
            // The hull and its fit, bought: the closed economy's own order.
            let role = self.memory.policy.role;
            let kit0 = crate::acquire::kit_of(role);
            let target = dev.hull.or(kit0.hull);
            let mut kit = crate::fit_plan::kit_for_hull(
                kit0,
                target,
                crate::progression::weapon_for(target, self.memory.seed),
            );
            kit.hull = target;
            // A barge or exhumer takes its own laser (PROG-MINE).
            if kit0 == crate::acquire::MINER_KIT
                && let Some(h) = target
            {
                kit.modules = Some((crate::mine_ladder::laser_of(h), 1));
            }
            let in_capsule = self
                .obs
                .ship
                .is_none_or(|s| s.type_id.is_none_or(|t| t == brain::CAPSULE));
            let lacking = crate::acquire::Lacking {
                ship: true,
                module: true,
                ammo: true,
                in_capsule,
            };
            let got = crate::acquire::run_order(
                conn,
                &crate::acquire::Order::from_kit(&kit),
                lacking,
                None,
                &crate::acquire::Rounds::default(),
            );
            self.track.docked_dirty = true;
            match got {
                Ok(got) => {
                    for what in &got.missing {
                        self.record(ActKind::Missing, what.clone(), vec![], Verdict::Ok);
                    }
                    self.record(ActKind::Acquire, got.summary(), vec![], Verdict::Ok);
                }
                Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                Err(e) => self.record(
                    ActKind::Missing,
                    format!("dev kit ({e})"),
                    vec![],
                    Verdict::Ok,
                ),
            }
        }
        // The boss forms the group's fleet; a foreman does it itself, and miners
        // follow the foreman's.
        let now = Instant::now();
        if dev.boss
            && self.dev_done
            // FAST-KIT: the boss forms the fleet once it is out, so its own
            // prep is not starved by the invites (or after FLEET_DEFER_S);
            // a bot at a person's pace invites at once, as before.
            && (!self.memory.policy.quick
                || matches!(self.obs.place, Place::InSpace { .. })
                || now.saturating_duration_since(*self.fleeter.since.get_or_insert(now))
                    >= Duration::from_secs(crate::dev::FLEET_DEFER_S))
            && !std::ptr::eq(self.memory.policy.role, &crate::foreman::ROLE)
            && !std::ptr::eq(self.memory.policy.role, &roles::MINE)
            && self.fleeter.next.is_none_or(|t| now >= t)
        {
            self.fleeter.next = Some(now + Duration::from_secs(crate::dev::FLEET_EVERY_S));
            let todo =
                crate::dev::to_invite(&dev.hall.members(&dev.group), me, &self.fleeter.invited);
            if !todo.is_empty() {
                let ship_type = self
                    .obs
                    .ship
                    .and_then(|s| s.type_id)
                    .or(dev.hull)
                    .unwrap_or(0);
                let sent: Result<Vec<i64>> = (|| {
                    let fleet = match conn.session().fleet_id() {
                        Some(f) => f,
                        None => conn.create_fleet(ship_type)?,
                    };
                    let mut sent = Vec::new();
                    for c in todo {
                        let seat = conn.fleet_seat(
                            fleet,
                            crate::fleet::squad_of(self.fleeter.invited.len()),
                        )?;
                        if conn.fleet_invite(fleet, c, seat).is_ok() {
                            self.fleeter.invited.insert(c);
                            sent.push(c);
                        }
                    }
                    Ok(sent)
                })();
                match sent {
                    Ok(sent) if !sent.is_empty() => self.record(
                        ActKind::Fleet,
                        format!("dev fleet: invited {sent:?}"),
                        sent,
                        Verdict::Ok,
                    ),
                    Ok(_) => {}
                    Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                    Err(e) => self.record(
                        ActKind::Missing,
                        format!("dev fleet ({e})"),
                        vec![],
                        Verdict::Ok,
                    ),
                }
            }
        }
        Ok(None)
    }

    fn summary(&mut self) {
        if self.last_summary.elapsed() < SUMMARY_EVERY || !self.config.echo {
            return;
        }
        self.last_summary = Instant::now();
        let fighting = self.obs.guns.iter().any(|g| g.active);
        let phase = match (self.obs.place, self.memory.homing, self.obs.miner) {
            (Place::Docked { .. }, _, _) => "docked",
            (_, Some(_), _) => "homing",
            (_, _, Some(l)) if l.active => "mining",
            _ if fighting => "fighting",
            (Place::InSpace { .. }, _, _) => "in space",
            (Place::Transit, _, _) => "transit",
        };
        let line = (self.memory.policy.role.banner)(&roles::Summary {
            name: &self.config.name,
            phase,
            ore_unloaded: self.ore_unloaded,
            isk_sold: self.isk_sold,
            kills: self.memory.kills,
            reconnects: self.reconnects,
        });
        println!("{line}");
    }

    /// **Chat, if the server has it**: [`Chat::open`], logged once as a
    /// `chat` entry either way. Only a machoNet fault is an error here.
    fn open_chat(&mut self, conn: &mut Conn) -> Result<()> {
        self.chat = None;
        self.obs.chat = false;
        if !self.config.chat {
            return Ok(());
        }
        let verdict = match Chat::open(conn, self.config.chat_port, self.config.chat_host) {
            Ok(Ok(chat)) => {
                let rooms = chat.rooms().join(" ");
                self.chat = Some(chat);
                self.obs.chat = true;
                self.record(ActKind::Chat, format!("on: {rooms}"), vec![], Verdict::Ok);
                return Ok(());
            }
            Ok(Err(why)) => why,
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(other) => other.to_string(),
        };
        self.record(
            ActKind::Chat,
            "off".to_owned(),
            vec![],
            Verdict::NotSent(verdict),
        );
        Ok(())
    }

    /// Read what the chat server sent, now and then; drop a dead chat.
    fn drain_chat(&mut self) {
        if self.last_drain.elapsed() < CHAT_DRAIN_EVERY {
            return;
        }
        self.last_drain = Instant::now();
        if let Some(chat) = &mut self.chat
            && let Err(why) = chat.drain()
        {
            self.chat = None;
            self.obs.chat = false;
            self.record(
                ActKind::Chat,
                "lost".to_owned(),
                vec![],
                Verdict::Failed(why),
            );
        }
        self.feed_chatter();
    }

    /// S1: what the room said and who came in, into the observation, with
    /// the bot-side human test (`is_human`: the bot marker, the fleet boss;
    /// nothing the server says about a character).
    fn feed_chatter(&mut self) {
        use crate::chatter::{Arrival, Line};
        let Some(chat) = &mut self.chat else {
            self.obs.talk = crate::chatter::Scene::default();
            return;
        };
        let bots = self
            .config
            .marker
            .lock()
            .map(|b| b.clone())
            .unwrap_or_default();
        let boss = self.obs.fleet.as_ref().and_then(|f| f.boss);
        let human = |c: i64| is_human(Some(c), None, boss, &bots);
        let talk = &mut self.obs.talk;
        for h in chat.take_heard() {
            talk.heard.push(Line {
                seq: h.seq,
                room: h.room,
                from: h.from_char,
                text: h.text,
                at: h.at,
                human: human(h.from_char),
            });
        }
        if let Some(me) = self.my_char {
            for (other, won, at) in crate::chatter::take_fights(me) {
                if human(other) {
                    self.fight_seq += 1;
                    talk.results.push(crate::chatter::Outcome {
                        seq: self.fight_seq,
                        won,
                        at,
                    });
                }
            }
        }
        for a in chat.take_arrived() {
            if human(a.char_id) {
                talk.arrived.push(Arrival {
                    seq: a.seq,
                    char_id: a.char_id,
                    at: a.at,
                });
            }
        }
        // Old lines are not answered: keep a minute and a bounded queue.
        let now = Instant::now();
        let keep = |at: Instant| now.saturating_duration_since(at) < CHAT_KEEP;
        talk.heard.retain(|l| keep(l.at));
        talk.arrived.retain(|a| keep(a.at));
        talk.results.retain(|r| keep(r.at));
        let over = talk.heard.len().saturating_sub(crate::chat::HEARD_CAP);
        talk.heard.drain(..over);
        let over = talk.arrived.len().saturating_sub(crate::chat::HEARD_CAP);
        talk.arrived.drain(..over);
        talk.names.clone_from(&self.chat_names);
        let place_system = match self.obs.place {
            Place::InSpace { system, .. } => Some(system),
            Place::Docked { station } => self
                .memory
                .policy
                .travel
                .as_deref()
                .and_then(|m| m.system_of(station)),
            Place::Transit => None,
        };
        talk.system_name = place_system.and_then(|s| {
            let map = self.memory.policy.travel.as_deref()?;
            Some(map.name_of(s)?.to_owned())
        });
        talk.local_humans = chat.local().members().filter(|m| human(m.char_id)).count();
        // Chattiness is the bot's sociability: its society mind's trait, or
        // the one its name draws under the default seed.
        talk.sociability = match &self.config.society {
            Some(soc) => soc
                .mind
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .traits
                .get(crate::mind::Trait::Sociability),
            None => crate::mind::Traits::draw(&self.memory.name, SOCIETY_SEED)
                .get(crate::mind::Trait::Sociability),
        };
    }

    /// **Names for the chat's phrase slots**: the humans in the lines and
    /// arrivals the bot holds, once each, over `config.GetMultiOwnersEx` --
    /// the call the client itself makes for any owner id. A failed or
    /// empty answer is not retried (the line that wanted the name stays
    /// excluded); only a dead connection is an error, and the refresh that
    /// follows finds it.
    fn resolve_chat_names(&mut self, conn: &mut Conn) {
        if self.chat.is_none() {
            return;
        }
        let want: Vec<i64> = self
            .obs
            .talk
            .heard
            .iter()
            .filter(|l| l.human)
            .map(|l| l.from)
            .chain(self.obs.talk.arrived.iter().map(|a| a.char_id))
            .filter(|c| !self.names_asked.contains(c))
            .collect::<BTreeSet<i64>>()
            .into_iter()
            .take(NAMES_PER_ASK)
            .collect();
        if want.is_empty() || self.names_asked.len() >= NAMES_KEPT {
            return;
        }
        self.names_asked.extend(want.iter().copied());
        let ids = PyValue::List(want.iter().map(|c| PyValue::Int(*c)).collect());
        if let Ok(out) = conn.call("config", "GetMultiOwnersEx", vec![ids])
            && let Some(v) = out.value()
        {
            for (id, name) in crate::chat::owner_names(&v) {
                self.chat_names.insert(id, name);
            }
        }
        self.obs.talk.names.clone_from(&self.chat_names);
    }

    /// **The observation, rebuilt from the server**: the session says where;
    /// docked, `List` the hangar and the ship; in space, `List` the ship
    /// and bind. Logged, because T7 reads what the server returned.
    fn observe_from_scratch(&mut self, conn: &mut Conn) -> Result<()> {
        self.track.changes = conn.session().changes;
        self.refresh_place(conn);
        let what = match self.obs.place {
            Place::Docked { station } => {
                self.refresh_docked(conn)?;
                format!("docked in {station}")
            }
            Place::InSpace { system, .. } => {
                self.refresh_ship(conn)?;
                format!("in space in {system}")
            }
            Place::Transit => "nowhere yet".to_owned(),
        };
        let ship = self.obs.ship.map(|s| s.ship_id);
        self.record(
            ActKind::Observe,
            format!("{what}, ship {ship:?}"),
            ship.into_iter().collect(),
            Verdict::Ok,
        );
        Ok(())
    }

    fn refresh_place(&mut self, conn: &Conn) {
        let s = conn.session();
        let place = if let Some(station) = s.station_id() {
            Place::Docked { station }
        } else if let Some(system) = s.solar_system_id() {
            Place::InSpace {
                system,
                bound: conn.ballpark().is_some() && conn.world().has_state(),
            }
        } else {
            Place::Transit
        };
        let was_docked = matches!(self.obs.place, Place::Docked { .. });
        let docked = matches!(place, Place::Docked { .. });
        if docked != was_docked {
            // A new place: every consecutive-failure count was about the
            // old one.
            self.obs.failures.clear();
            self.obs.locked.clear();
            self.obs.pending.clear();
            self.obs.lock_jam = None;
            self.track.lasers.clear();
            self.track.aggressors.clear();
            self.track.player_fire.clear();
            self.track.lockers.clear();
            self.obs.threat.foes.clear();
            self.obs.wing.links.clear();
            self.obs.health = None;
            self.obs.hold_full = false;
            if let Some(l) = &mut self.obs.miner {
                l.active = false;
                l.target = None;
            }
            for g in &mut self.obs.guns {
                g.active = false;
                g.target = None;
            }
            if docked {
                self.track.docked_dirty = true;
                self.obs.park = None;
                self.obs.humans.clear();
                self.obs.logi.bars.clear();
                self.obs.logi.mates.clear();
                self.obs.human_types.clear();
                self.obs.hunt.pilots.clear();
                self.obs.hunt.wrecks.clear();
                self.obs.hunt.sweep = None;
            } else {
                self.obs.hangar.clear();
            }
        }
        if let Place::Docked { station } = place {
            self.obs.home = Some(station);
            // On loan, the station docked at is not the bot's home.
            if self.lease.is_none() {
                let home = *self.home_station.get_or_insert(station);
                if self
                    .config
                    .society
                    .as_ref()
                    .is_some_and(|soc| soc.adopt_home(home))
                {
                    self.save_mind();
                }
            }
        }
        self.obs.place = place;
        self.obs.fleet = match (s.fleet_id(), self.obs.fleet) {
            (None, _) => None,
            (Some(id), Some(f)) if f.fleet_id == id => Some(f),
            (Some(id), _) => Some(FleetView {
                fleet_id: id,
                boss: None,
            }),
        };
        if self.obs.fleet.is_none() {
            self.obs.primary = None;
        }
        // FC-1: a wing bot's boss is its FC, whoever invited it.
        if let Some(fc) = self.memory.policy.wing.as_ref().and_then(|w| w.fc)
            && let Some(f) = &mut self.obs.fleet
        {
            f.boss = Some(fc);
        }
        if self.memory.policy.wing.is_some()
            && let Ok(mut b) = self.handle.boss_pub.lock()
        {
            *b = self.obs.fleet.and_then(|f| f.boss);
        }
        if let Some(ship_id) = s.ship_id()
            && self.obs.ship.is_none_or(|sh| sh.ship_id != ship_id)
        {
            self.obs.ship = Some(ShipInfo {
                ship_id,
                type_id: None,
            });
            self.obs.miner = None;
            self.obs.salvager = None;
            self.obs.logi.reps.clear();
            self.obs.guns.clear();
            self.track.docked_dirty = true;
        }
    }

    /// Everything a turn needs, from what the connection already holds --
    /// plus fresh listings when docked and something moved.
    /// **The park adrift** (`park.rs`): the ball the ego was tethered to
    /// left, so the ego's believed place may be far from the server's. One
    /// `UpdateStateRequest` per park state, logged as `resync`.
    fn resync_if_adrift(&mut self, conn: &mut Conn) -> Result<()> {
        let world = conn.world();
        if !world.adrift || self.track.adrift_tried == Some(world.states) {
            return Ok(());
        }
        self.track.adrift_tried = Some(world.states);
        // A wing follower's FC warped off grid: the follower re-binds on the
        // FC's landing, and 200 of them asking at once cost the server a
        // full state each (lane fight-stall-fix, 200v200: ~7 ms x 220).
        let fc = self
            .memory
            .policy
            .wing
            .as_ref()
            .and_then(|w| w.fc)
            .or_else(|| self.obs.fleet.and_then(|f| f.boss));
        if skips_resync(self.memory.policy.wing.is_some(), fc, world.adrift_owner) {
            return Ok(());
        }
        let verdict = match conn.request_state() {
            Ok(_) => Verdict::Ok,
            Err(Error::NotSent(w)) => Verdict::NotSent(w),
            Err(Error::Refused(w)) => Verdict::Refused(w),
            Err(e) => return Err(e),
        };
        self.record(
            ActKind::Resync,
            "UpdateStateRequest: the ego's tether target left the park".to_owned(),
            vec![],
            verdict,
        );
        Ok(())
    }

    fn refresh(&mut self, conn: &mut Conn) -> Result<()> {
        self.absorb_all(conn);
        self.drain_chat();
        self.resolve_chat_names(conn);
        self.obs.now = Instant::now();
        self.obs.me = conn.session().char_id().unwrap_or(0);
        if conn.session().changes != self.track.changes {
            self.track.changes = conn.session().changes;
            self.track.docked_dirty = true;
        }
        self.refresh_place(conn);
        self.set_clone_home(conn)?;
        self.kit_drill(conn)?;
        self.camp_dock(conn)?;
        match self.obs.place {
            Place::Docked { .. } if self.track.docked_dirty => self.refresh_docked(conn)?,
            Place::InSpace { bound: true, .. } => {
                // The hold window's refresh: the client re-lists on the
                // cache invalidation a mining cycle sends.
                if self.track.cargo_dirty {
                    self.track.cargo_dirty = false;
                    self.refresh_ship(conn)?;
                }
                self.resync_if_adrift(conn)?;
                self.refresh_park(conn);
                self.run_directives(conn)?;
            }
            _ => {}
        }
        if let Ok(mut seen) = self.handle.seen.lock() {
            *seen = Some(self.obs.clone());
        }
        if let Ok(mut r) = self.handle.role_pub.lock() {
            *r = self.memory.policy.role.name;
        }
        Ok(())
    }

    /// The hangar, the hold and the laser, listed.
    fn refresh_docked(&mut self, conn: &mut Conn) -> Result<()> {
        self.track.docked_dirty = false;
        let hangar = match conn.hangar_items() {
            Ok(h) => h,
            Err(e) => return self.soft(ActKind::Observe, "List the hangar", e),
        };
        if let Some(ship) = &mut self.obs.ship {
            ship.type_id = hangar
                .iter()
                .find(|r| r.item_id == ship.ship_id)
                .map(|r| r.type_id);
        }
        self.obs.hangar = hangar;
        self.refresh_ship(conn)?;
        self.sync_doctrine(conn);
        // BP-CAMP: the role is known only now; grant and load before the
        // turn's decision undocks (`camp::kit_pending` holds it meanwhile).
        self.camp_dock(conn)?;
        self.skill_plan_tick(conn)
    }

    /// **DOC-4/5: the commander's turn.** In space, the actions the hall's
    /// theatre machine queued for this character ([`crate::corpmind::Hall::take_directives`])
    /// become fleet orders ([`crate::fleet::orders_for`]) and are sent on
    /// this connection, each its own act-log line. A refusal is a line; only
    /// a dead connection is the error. Nothing queued is the common case and
    /// costs one lock.
    fn run_directives(&mut self, conn: &mut Conn) -> Result<()> {
        let Some(hall) = self.config.society.as_ref().and_then(|s| s.hall.clone()) else {
            return Ok(());
        };
        let (Some(me), Some(ship)) = (conn.session().char_id(), conn.session().ship_id()) else {
            return Ok(());
        };
        for action in hall.take_directives(me) {
            for order in crate::fleet::orders_for(&mut self.memory.fleet, &action, ship, me) {
                let result = crate::fleet::execute(conn, &self.memory.fleet, &order);
                self.memory.fleet.on_done(&order, &result);
                let (line, ids) = order.describe();
                let verdict = match &result {
                    Ok(_) => Verdict::Ok,
                    Err(Error::NotSent(w)) => Verdict::NotSent(w.clone()),
                    Err(Error::Refused(w)) => Verdict::Refused(w.clone()),
                    Err(Error::Io(_) | Error::Protocol(_)) => {
                        return result.map(|_| ());
                    }
                };
                self.record(ActKind::Fleet, line, ids, verdict);
            }
        }
        Ok(())
    }

    /// **DOC-5: the fit the leader's doctrine gives this bot**, read from
    /// the hall into the combat memory each docked turn (closed economy
    /// only): the hull chooser, the skill plan, the purchase and the fit all
    /// read it from there. `None` (no hall, no doctrine, not a fighter)
    /// leaves every one of them on its old path.
    fn sync_doctrine(&mut self, conn: &Conn) {
        let plan = self
            .config
            .society
            .as_ref()
            .and_then(|s| s.hall.clone())
            .filter(|_| self.memory.policy.closed)
            .and_then(|h| conn.session().char_id().and_then(|me| h.plan_for(me)));
        // BP-CAMP: the hall's role; with none (open mode, a bot spawned
        // `/bots 1 hostile dictor`), the flown hull's: an interdictor or a
        // heavy interdictor camps.
        let by_hull = self
            .memory
            .policy
            .pvp
            .as_ref()
            .is_some_and(|g| g.is_bubbler(self.obs.ship.and_then(|s| s.type_id)));
        self.memory.combat.role = plan
            .as_ref()
            .map(|p| p.role)
            .or(by_hull.then_some(crate::doctrine::Role::Interdict));
        self.memory.combat.doctrine = plan.map(|p| p.fit);
    }

    /// **The dictor's dock-side work** (BP-CAMP): docked with the Interdict
    /// role, at most once a minute, an open-mode bot is given its launcher
    /// and probes once a session ([`crate::camp::gm_lines`]; only a
    /// dev-spawned bot, never a market bot, which buys or builds the part), and the hangar's
    /// probes are loaded ([`crate::camp::load_probes`]). A refusal is a log
    /// line; only a dead connection is the error.
    fn camp_dock(&mut self, conn: &mut Conn) -> Result<()> {
        if self.memory.combat.role != Some(crate::doctrine::Role::Interdict)
            || !matches!(self.obs.place, Place::Docked { .. })
        {
            return Ok(());
        }
        let now = Instant::now();
        if self.memory.camp.dock_next.is_some_and(|t| now < t) {
            return Ok(());
        }
        self.memory.camp.dock_next = Some(now + crate::camp::DOCK_EVERY);
        // `/bots` bots always run closed; a dev-spawned bot (`policy.dev`) is
        // the open-mode preset that may be granted its part.
        if self.memory.policy.dev.is_some()
            && !self.memory.camp.granted
            && !self.obs.hangar.iter().any(crate::camp::is_launcher)
            && self.obs.kit.launcher.is_none()
            && let Some(content) = self.config.content.clone()
        {
            self.memory.camp.granted = true;
            for line in crate::camp::gm_lines(&content) {
                let outcome = crate::gm::slash(conn, &line)?;
                let verdict = match outcome.refusal() {
                    None => Verdict::Ok,
                    Some(key) => Verdict::Refused(key),
                };
                self.record(ActKind::Slash, line, vec![], verdict);
            }
            self.track.docked_dirty = true;
        }
        match crate::camp::load_unit(conn) {
            Ok(note) if !note.is_empty() => self.note = Some(note),
            Ok(_) => {}
            Err(e) => return self.soft(ActKind::Observe, "camp unit", e),
        }
        match crate::camp::load_probes(conn) {
            Ok(note) if !note.is_empty() => {
                self.track.docked_dirty = true;
                self.note = Some(note);
                Ok(())
            }
            Ok(_) => Ok(()),
            Err(e) => self.soft(ActKind::Observe, "camp probes", e),
        }
    }

    /// **The skill plan** (X-SKILL, [`crate::skill_plan`]): docked in the
    /// closed economy, at most once per [`crate::skill_plan::FILL_EVERY`],
    /// the queue is topped up with the next steps of the role's plans. A
    /// refusal is a log line; only a dead connection is the error.
    fn skill_plan_tick(&mut self, conn: &mut Conn) -> Result<()> {
        let Some(content) = self.config.content.clone() else {
            return Ok(());
        };
        let now = Instant::now();
        if !self.memory.policy.closed || self.track.skills_next.is_some_and(|t| now < t) {
            return Ok(());
        }
        self.track.skills_next = Some(now + crate::skill_plan::FILL_EVERY);
        // PROG-PVE: a fighter's plan ends with the next class's skills.
        let rung = crate::progression::rung(
            self.obs.ship.and_then(|s| s.type_id),
            &self.obs.hangar,
            self.memory.seed,
        );
        let before = conn.cash_balance().ok();
        let done = match crate::skill_plan::fill_with(
            conn,
            &content,
            self.memory.policy.role,
            Some(rung),
            self.memory.combat.doctrine.as_ref(),
        ) {
            Ok(d) => d,
            Err(e) => return self.soft(ActKind::Acquire, "skill plan", e),
        };
        // SKILLGATE: a queued skill may open a hull set aside as untrained.
        self.memory.combat.untrained.clear();
        if let (Some(b), Ok(a)) = (before, conn.cash_balance()) {
            self.ledger.invest(b - a);
        }
        for what in &done.missing {
            self.record(ActKind::Missing, what.clone(), vec![], Verdict::Ok);
        }
        if !done.targets.is_empty() {
            self.record(
                ActKind::Acquire,
                format!(
                    "skill plan: targets {:?}, queued {:?}",
                    done.targets, done.learned
                ),
                vec![],
                Verdict::Ok,
            );
        }
        // WS-3: after the fill, so the head and the new entries are funded.
        // The world's free points go onto the queue as the human button
        // does; they reach only books the bot bought.
        let spent = match crate::acquire::apply_free_to_queue(conn) {
            Ok(n) => n,
            Err(e) => return self.soft(ActKind::Acquire, "apply free skill points", e),
        };
        if spent > 0 {
            let left = crate::acquire::free_points(conn).unwrap_or(0);
            self.track.skills_next =
                Some(Instant::now() + crate::skill_plan::next_fill(spent, left));
            self.record(
                ActKind::Acquire,
                format!("free skill points: {spent} applied, {left} left"),
                vec![],
                Verdict::Ok,
            );
        }
        Ok(())
    }

    /// The active ship's hold and laser, listed: docked or in space.
    fn refresh_ship(&mut self, conn: &mut Conn) -> Result<()> {
        let Some(ship) = self.obs.ship else {
            return Ok(());
        };
        let rows = match conn.ship_items(ship.ship_id, None) {
            Ok(r) => r,
            Err(e) => return self.soft(ActKind::Observe, "List the ship", e),
        };
        self.refresh_guns(conn, ship.ship_id, &rows)?;
        self.obs.salvager = match crate::salvage::read(conn, &rows, self.config.content.as_deref())
        {
            Ok(s) => s,
            Err(e) => {
                self.soft(ActKind::Observe, "ItemGetInfo (salvager)", e)?;
                None
            }
        };
        self.obs.logi.reps = match crate::logi::read(conn, &rows, &self.obs.logi.reps) {
            Ok(r) => r,
            Err(e) => {
                self.soft(ActKind::Observe, "ItemGetInfo (remote repairers)", e)?;
                Vec::new()
            }
        };
        self.refresh_served_lock_range(conn, ship.ship_id);
        self.obs.cargo = rows
            .iter()
            .filter(|r| r.flag_id == crate::FLAG_CARGO)
            .copied()
            .collect();
        self.obs.ore_hold = rows
            .iter()
            .filter(|r| r.flag_id == brain::FLAG_MINING_HOLD)
            .copied()
            .collect();
        let laser = rows
            .iter()
            .find(|r| r.is_fitted() && brain::is_mining_laser(r.type_id))
            .copied();
        self.obs.miner = match laser {
            None => None,
            Some(row) => {
                let info = match conn.module_row(row.item_id) {
                    Ok(r) => r,
                    Err(e) => {
                        self.soft(ActKind::Observe, "ItemGetInfo", e)?;
                        crate::ModuleRow::default()
                    }
                };
                let effects = &info.effects;
                // `maxRange` off the laser's own row, else the bundle's,
                // else the invented fixture's (FX11, `crate::modules`).
                let (range, source) = crate::modules::laser_range(
                    |a| info.attribute(a),
                    self.config.content.as_deref(),
                    row.type_id,
                );
                self.log_module(row.item_id, row.type_id, source, || {
                    format!("laser range {range:.0} m")
                });
                // Running, by the server's word: after a reconnect this is
                // the only way to know (the laser outlives a dropped
                // connection by up to a cycle; the P8b note).
                let active = effects.contains(&crate::MINING_LASER.1);
                let was = self.obs.miner.filter(|m| m.item_id == row.item_id);
                Some(ModuleInfo {
                    item_id: row.item_id,
                    flag_id: row.flag_id,
                    online: effects.contains(&crate::EFFECT_ONLINE),
                    active,
                    target: was.and_then(|m| m.target).filter(|_| active),
                    range,
                })
            }
        };
        Ok(())
    }

    /// **The ship's lock range, by the server's word** (FX10,
    /// [`crate::hulls`]): attribute 76 off the ship's `ItemGetInfo` row,
    /// once per ship. A failed read is not cached (the next listing tries
    /// again) and costs nothing else: [`Self::refresh_park`] falls back to
    /// the bundle, then the fixture.
    fn refresh_served_lock_range(&mut self, conn: &mut Conn, ship_id: i64) {
        if self
            .track
            .served_lock_range
            .is_some_and(|(s, _)| s == ship_id)
        {
            return;
        }
        if let Ok(r) = conn.ship_attribute(ship_id, eve_server::typedata::attr::MAX_TARGET_RANGE) {
            self.track.served_lock_range = Some((ship_id, r));
            // BP-EWAR-AIM: the lock slots, for the extra locks an ewar module needs.
            self.obs.lock_max = conn
                .ship_attribute(ship_id, eve_server::typedata::attr::MAX_LOCKED_TARGETS)
                .ok()
                .flatten()
                .map_or(0, |n| n.max(0.0) as usize);
            self.refresh_cap_model(conn, ship_id);
        }
    }

    /// **The hull's capacitor numbers** (BOT-CAP, [`crate::cap`]): capacity,
    /// recharge time and current charge off the ship's `ItemGetInfo` row,
    /// once per ship. A failed read leaves the tracking on the pushes alone.
    fn refresh_cap_model(&mut self, conn: &mut Conn, ship_id: i64) {
        use crate::cap::{ATTR_CAPACITY, ATTR_CHARGE, ATTR_RECHARGE};
        let mut read = |a| conn.ship_attribute(ship_id, a).ok().flatten();
        let (capacity, recharge, charge) =
            (read(ATTR_CAPACITY), read(ATTR_RECHARGE), read(ATTR_CHARGE));
        if let (Some(c), Some(r)) = (capacity, recharge) {
            self.track
                .cap
                .cap
                .set_model(ship_id, c, r, charge, Instant::now());
        }
    }

    /// **The guns**: every fitted module the server's table calls a weapon,
    /// read as godma reads a module -- `ItemGetInfo` for its running effects
    /// -- and its loaded rounds off the `(ship, flag, type)` key.
    fn refresh_guns(
        &mut self,
        conn: &mut Conn,
        ship_id: i64,
        rows: &[crate::ItemRow],
    ) -> Result<()> {
        let mut guns = Vec::new();
        let content = self.config.content.clone();
        let content = content.as_deref();
        for row in rows.iter().filter(|r| r.is_fitted()) {
            let Some((_, effect_id)) = crate::weapon_effect(content, row.type_id) else {
                continue;
            };
            let info = match conn.module_row(row.item_id) {
                Ok(r) => r,
                Err(e) => {
                    self.soft(ActKind::Observe, "ItemGetInfo", e)?;
                    crate::ModuleRow::default()
                }
            };
            let effects = &info.effects;
            // Optimal, falloff and reload off the gun's own row, else the
            // bundle's, else the invented fixture's (FX11).
            let served = |a| info.attribute(a);
            let (range, from) = crate::modules::gun_range(served, content, row.type_id);
            let (reload, reload_from) = crate::modules::reload_time(served, content, row.type_id);
            self.log_module(row.item_id, row.type_id, from, || {
                format!(
                    "gun range {range:.0} m, reload {} ms ({reload_from})",
                    reload.as_millis()
                )
            });
            let types = self
                .track
                .round_types
                .entry(row.type_id)
                .or_insert_with(|| {
                    content.map_or_else(Vec::new, |c| crate::ammo::rounds_for(c, row.type_id))
                })
                .clone();
            let mut range = range;
            let rounds = match crate::ammo::loaded_round(conn, ship_id, row.flag_id, &types) {
                Ok(r) => {
                    // A launcher's row has no range: its round's reach is
                    // the gun range, else the orbit has nothing to hold.
                    if effect_id == eve_server::modules::EFFECT_USE_MISSILES
                        && let Some(reach) = r
                            .zip(content)
                            .and_then(|((t, _), c)| crate::modules::missile_reach(c, t))
                    {
                        range = reach;
                    }
                    Some(r.map_or(0, |(_, n)| n))
                }
                Err(e) => {
                    self.soft(ActKind::Observe, "ItemGetInfo (rounds)", e)?;
                    None
                }
            };
            // BP-EWAR-AIM: what the module does, and how far it reaches.
            let kind = crate::brain::GunKind::of_effect(effect_id);
            let reach = if kind == crate::brain::GunKind::Weapon {
                range
            } else {
                crate::modules::ewar_reach(served, content, row.type_id)
            };
            let active = effects.contains(&effect_id);
            let was = self.obs.guns.iter().find(|g| g.item_id == row.item_id);
            guns.push(GunInfo {
                item_id: row.item_id,
                type_id: row.type_id,
                flag_id: row.flag_id,
                online: effects.contains(&crate::EFFECT_ONLINE),
                active,
                target: was.and_then(|g| g.target).filter(|_| active),
                rounds,
                // PROG-PVE: a medium or large gun is measured with its own
                // size's round.
                capacity: crate::modules::rounds_capacity(
                    content,
                    row.type_id,
                    crate::fit_plan::round_of_gun(row.type_id),
                ),
                range,
                reload,
                kind,
                reach,
            });
        }
        self.obs.guns = guns;
        // CLOAK-2: a cloaking device in the fit (only ever from stock).
        let mut kit = None;
        for row in rows.iter().filter(|r| r.is_fitted()) {
            let Some(effect) = crate::modules::find(content, row.type_id)
                .and_then(|(t, _)| t.effect)
                .filter(|e| eve_server::cloak::is_cloak_effect(e.effect_id))
            else {
                continue;
            };
            let info = match conn.module_row(row.item_id) {
                Ok(r) => r,
                Err(e) => {
                    self.soft(ActKind::Observe, "ItemGetInfo (cloak)", e)?;
                    continue;
                }
            };
            let ms =
                |a| Duration::from_millis(info.attribute(a).unwrap_or(0.0).max(0.0).round() as u64);
            kit = Some(crate::hunt::CloakKit {
                item_id: row.item_id,
                type_id: row.type_id,
                effect: (effect.name, effect.effect_id),
                warp_safe: effect.warp_safe == Some(true),
                targeting_delay: ms(560),
                reactivation: ms(669),
                active: info.effects.contains(&effect.effect_id),
            });
            break;
        }
        self.obs.hunt.cloak = kit;
        // BP-KIT: smartbombs and the micro jump drive, by default effect.
        let mut pvp = crate::pvp_kit::Kit::default();
        for row in rows.iter().filter(|r| r.is_fitted()) {
            let Some(effect) = crate::modules::find(content, row.type_id)
                .and_then(|(t, _)| t.effect)
                .filter(|e| crate::pvp_kit::is_kit_effect(e.effect_id))
            else {
                continue;
            };
            let info = match conn.module_row(row.item_id) {
                Ok(r) => r,
                Err(e) => {
                    self.soft(ActKind::Observe, "ItemGetInfo (kit)", e)?;
                    continue;
                }
            };
            let from_type = crate::modules::find(content, row.type_id).map(|(t, _)| t);
            pvp.push(crate::pvp_kit::KitModule::read(
                row.item_id,
                row.type_id,
                &effect,
                info.effects.contains(&effect.effect_id),
                info.effects.contains(&crate::EFFECT_ONLINE),
                |a| {
                    info.attribute(a)
                        .or_else(|| from_type.and_then(|t| t.attribute(a)))
                },
            ));
        }
        // BP-CAMP: the sphere launcher, found by group (its default effect
        // is the missile launcher's), and the probes loaded in it.
        let launcher_row = rows
            .iter()
            .find(|r| r.is_fitted() && crate::camp::is_launcher(r));
        if let Some(row) = launcher_row
            && let Some(effect) =
                crate::modules::find(content, row.type_id).and_then(|(t, _)| t.effect)
        {
            match conn.module_row(row.item_id) {
                Err(e) => self.soft(ActKind::Observe, "ItemGetInfo (launcher)", e)?,
                Ok(info) => {
                    let remembered = self.obs.kit.launcher.and_then(|l| l.probe);
                    // The probe types to ask for: the hold's, the last one
                    // loaded, the fit's.
                    let mut kinds: Vec<i64> = self
                        .obs
                        .cargo
                        .iter()
                        .filter(|r| crate::camp::is_probe(r))
                        .map(|r| r.type_id)
                        .collect();
                    kinds.extend(remembered);
                    kinds.extend(
                        self.memory
                            .combat
                            .doctrine
                            .iter()
                            .flat_map(|d| d.charges.iter().map(|c| c.0)),
                    );
                    let mut loaded = None;
                    for t in kinds {
                        match conn.loaded_rounds(ship_id, row.flag_id, t) {
                            Ok(n) if n > 0 => {
                                loaded = Some((t, n));
                                break;
                            }
                            Ok(_) => loaded = loaded.or(Some((t, 0))),
                            Err(e) => {
                                self.soft(ActKind::Observe, "ItemGetInfo (probes)", e)?;
                                loaded = None;
                                break;
                            }
                        }
                    }
                    pvp.launcher = Some(crate::camp::launcher_of(
                        row,
                        (effect.name, effect.effect_id),
                        loaded,
                        remembered,
                        info.effects.contains(&effect.effect_id),
                    ));
                }
            }
        }
        self.obs.kit = pvp;
        Ok(())
    }

    /// **One `observe` line per module and source** (FX11, as FX10 logs
    /// the lock range): where a module's numbers came from, `server`,
    /// `bundle`, `invented` or `unknown`.
    fn log_module(
        &mut self,
        item_id: i64,
        type_id: i64,
        source: crate::hulls::Source,
        what: impl FnOnce() -> String,
    ) {
        if self.track.module_logged.get(&item_id) == Some(&source) {
            return;
        }
        self.track.module_logged.insert(item_id, source);
        self.record(
            ActKind::Observe,
            format!("module {item_id} type {type_id}: {} ({source})", what()),
            vec![item_id],
            Verdict::Ok,
        );
    }

    /// **The kit drill** (BP-KIT, CI only): with `EVE_BOTS_KIT_DRILL` set to
    /// `smartbomb` or `mjd`, a docked open-mode hunter is given the skill and
    /// the part once a session, so the stock rule fits it
    /// ([`crate::pvp_kit::stock_to_fit`]). Only a dev-spawned bot
    /// (`policy.dev`; `/bots` bots are `closed` too), never a market bot.
    fn kit_drill(&mut self, conn: &mut Conn) -> Result<()> {
        if self.drilled
            || self.memory.policy.dev.is_none()
            || self.memory.policy.pvp.is_none()
            || !matches!(self.obs.place, Place::Docked { .. })
        {
            return Ok(());
        }
        self.drilled = true;
        let Ok(which) = std::env::var(crate::pvp_kit::DRILL_ENV) else {
            return Ok(());
        };
        // `/bots` roamers have no `--hostile` rules; the foes are the FOES side.
        let foe = self.memory.policy.dev.as_ref().is_some_and(|d| d.is_foe());
        for line in crate::pvp_kit::drill_lines(&which, foe) {
            let outcome = crate::gm::slash(conn, &line)?;
            let verdict = match outcome.refusal() {
                None => Verdict::Ok,
                Some(key) => Verdict::Refused(key),
            };
            self.record(ActKind::Slash, line, vec![], verdict);
        }
        // A drill part that names its hulls: fly one it fits (FIT-REFUSE).
        if let Some(hull) = crate::pvp_kit::drill_hull(&which, foe)
            && let Some(row) = conn.hangar_items()?.into_iter().find(|r| r.type_id == hull)
        {
            let verdict = match conn.board(row.item_id) {
                Ok(_) => Verdict::Ok,
                Err(Error::Refused(key)) => Verdict::Refused(key),
                Err(e) => return Err(e),
            };
            self.record(
                ActKind::Board,
                format!("Board {}", row.item_id),
                vec![],
                verdict,
            );
        }
        self.track.docked_dirty = true;
        Ok(())
    }

    /// **CLONE-HOME**: once a session, docked at the bot's own base (never
    /// at a lease station, whose `home_station` is not the bot's), set the
    /// medical clone there, so a podding wakes it at its base and not where
    /// its first dock happened. The server accepts only the station the
    /// pilot is docked in and answers an already-set clone with nothing, so
    /// no read of `get_home_station` is needed **[A]**: falsified by a bot
    /// pod waking at another station. A refusal is logged and not retried.
    fn set_clone_home(&mut self, conn: &mut Conn) -> Result<()> {
        let Place::Docked { station } = self.obs.place else {
            return Ok(());
        };
        if self.clone_set || self.lease.is_some() || self.home_station != Some(station) {
            return Ok(());
        }
        self.clone_set = true;
        match conn.call(
            CLONE_SERVICE,
            CLONE_METHOD,
            vec![PyValue::Int(station), PyValue::Bool(false)],
        ) {
            Ok(_) => self.record(
                ActKind::Observe,
                format!("clone home at {station}"),
                vec![station],
                Verdict::Ok,
            ),
            Err(Error::Refused(w)) => self.record(
                ActKind::Observe,
                format!("clone home at {station}"),
                vec![station],
                Verdict::Refused(w),
            ),
            Err(Error::NotSent(w)) => self.record(
                ActKind::Observe,
                format!("clone home at {station}"),
                vec![station],
                Verdict::NotSent(w),
            ),
            Err(e) => return Err(e),
        }
        Ok(())
    }

    /// A refusal while observing is logged and survived; a transport fault
    /// is not.
    fn soft(&mut self, kind: ActKind, what: &str, e: Error) -> Result<()> {
        match e {
            Error::NotSent(w) => {
                self.record(kind, what.to_owned(), vec![], Verdict::NotSent(w));
                Ok(())
            }
            Error::Refused(w) => {
                self.record(kind, what.to_owned(), vec![], Verdict::Refused(w));
                Ok(())
            }
            other => Err(other),
        }
    }

    /// The park view, carried forward to now.
    fn refresh_park(&mut self, conn: &mut Conn) {
        let ship_id = conn.session().ship_id();
        let home = self.obs.home;
        let boss = self.obs.fleet.as_ref().and_then(|f| f.boss);
        let me = conn.session().char_id();
        let (my_corp, my_alliance) = (
            conn.session().get_i64("corpid"),
            conn.session().get_i64("allianceid"),
        );
        let my_fleet = conn.session().fleet_id();
        let here_system = conn.session().solar_system_id2();
        let world = conn.world_now();
        let ego = world
            .ego_ball()
            .or_else(|| ship_id.and_then(|s| world.get(s)));
        let Some(ego) = ego else {
            self.obs.park = None;
            return;
        };
        let ball = |t: &crate::Tracked| Ball {
            id: t.ball_id,
            position: t.motion.position,
            radius: t.radius,
        };
        let station = home
            .and_then(|h| world.get(h))
            .or_else(|| world.balls().find(|b| STATION_IDS.contains(&b.ball_id)));
        let mut taken: BTreeMap<i64, i64> = BTreeMap::new();
        for (&ship, &rock) in &self.track.lasers {
            if ship == ego.ball_id || world.get(rock).is_none() {
                continue;
            }
            let lowest = taken.entry(rock).or_insert(ship);
            *lowest = (*lowest).min(ship);
        }
        let hostiles: Vec<Hostile> = world
            .balls()
            .filter(|b| is_npc(b))
            // The law is not a hostile unless it is shooting this ship, so a
            // clean escort never opens fire on police (NW-7).
            .filter(|b| {
                !is_law(self.config.content.as_deref(), b)
                    || self.track.aggressors.get(&b.ball_id) == Some(&ego.ball_id)
            })
            .map(|b| {
                let type_id = b.type_id().unwrap_or(0);
                Hostile {
                    ball: ball(b),
                    type_id,
                    name: eve_server::npc::find(type_id)
                        .map_or_else(|| format!("NPC {type_id}"), |t| t.name.to_owned()),
                    shooting: self.track.aggressors.get(&b.ball_id).copied(),
                }
            })
            .collect();
        let wrecks: Vec<Ball> = world
            .balls()
            .filter(|b| is_npc_wreck(b) && may_loot(b, me, my_corp, my_fleet))
            .map(ball)
            .collect();
        // The hull, when the hangar never said: the ship's own slim.
        if let Some(sh) = &mut self.obs.ship
            && sh.type_id.is_none()
        {
            sh.type_id = ego.type_id();
        }
        // The lock range: the server's attribute 76, else the bundle's
        // hull, else the invented fixture's (FX10, `crate::hulls`).
        let ship = self.obs.ship;
        let served = ship.and_then(|s| {
            self.track
                .served_lock_range
                .filter(|(id, _)| *id == s.ship_id)
                .and_then(|(_, r)| r)
        });
        let (range, source) = crate::hulls::lock_range(
            served,
            self.config.content.as_deref(),
            ship.and_then(|s| s.type_id),
        );
        self.obs.lock_range = range;
        let ship_id = ship.map_or(0, |s| s.ship_id);
        if self.track.lock_logged != Some((ship_id, source)) {
            self.track.lock_logged = Some((ship_id, source));
            self.record(
                ActKind::Observe,
                format!("lock range {range:.0} m ({source})"),
                vec![ship_id],
                Verdict::Ok,
            );
        }
        let view = ParkView {
            ego: ball(ego),
            ego_warping: ego.warp_commanded(),
            // MN-D: the free moon rocks of a lesser ore are not shown while a
            // rarer one is free (`crate::moon_pref::scanned`).
            rocks: crate::demand::by_value(
                crate::moon_pref::scanned(
                    world.rocks().collect(),
                    &taken,
                    self.config.content.as_deref(),
                ),
                &taken,
                &self.track.ore_values,
            )
            .into_iter()
            .map(ball)
            .collect(),
            belts: world.balls().filter(|b| is_belt(b)).map(ball).collect(),
            station: station.map(ball),
            boss: boss.and_then(|c| world.ship_of(c)).map(ball),
            taken,
            hostiles,
            wrecks,
            ego_tether: ego.motion.tether().map(|(t, _)| t),
        };
        // BP-HOLD: the probe spheres and the gates in the park.
        let content = self.config.content.as_deref();
        self.obs.warp_bubble.zones = world
            .balls()
            .filter(|b| {
                (eve_server::bubble::ID_BASE..eve_server::bubble::ID_END).contains(&b.ball_id)
                    // NW-11: an anchored mobile disruptor (group 361) is a
                    // sphere too. The client does not say whether it is
                    // anchored yet **[A]**: a free one counts.
                    || content.zip(b.type_id()).is_some_and(|(c, t)| {
                        eve_server::typedata::installed_in(c)
                            .table()
                            .get(t)
                            .map(|r| r.group_id)
                            .or_else(|| eve_server::item::find_in(c, t).map(|i| i.group_id))
                            == Some(eve_server::anchor::GROUP_MOBILE_DISRUPTOR)
                    })
            })
            .map(|b| {
                let radius = content
                    .zip(b.type_id())
                    .and_then(|(c, t)| {
                        eve_server::typedata::attribute_in(
                            c,
                            t,
                            crate::warp_bubble::ATTRIBUTE_RADIUS,
                        )
                    })
                    .filter(|r| *r > 0.0)
                    .unwrap_or(crate::warp_bubble::DEFAULT_RADIUS);
                (b.motion.position, radius)
            })
            .collect();
        let gate_ids: BTreeSet<i64> = here_system
            .zip(self.memory.policy.travel.as_ref())
            .map(|(here, map)| {
                map.neighbours(here)
                    .into_iter()
                    .filter_map(|n| map.gate(here, n).map(|(g, _)| g))
                    .collect()
            })
            .unwrap_or_default();
        self.obs.warp_bubble.gates = world
            .balls()
            .filter(|b| gate_ids.contains(&b.ball_id))
            .map(ball)
            .collect();
        let bots = self
            .config
            .marker
            .lock()
            .map(|b| b.clone())
            .unwrap_or_default();
        let humans: Vec<&crate::Tracked> = world
            .balls()
            .filter(|b| b.ball_id != ego.ball_id)
            .filter(|b| is_human(b.slim.and_then(|s| s.char_id), me, boss, &bots))
            .collect();
        // K-LOGI: the other bots in the park are the logi's wards, and only
        // the bars of ships still here are kept.
        self.obs.logi.mates = world
            .balls()
            .filter(|b| b.ball_id != ego.ball_id)
            .filter(|b| {
                b.slim
                    .and_then(|s| s.char_id)
                    .is_some_and(|c| bots.contains(&c))
            })
            .map(ball)
            .collect();
        self.obs
            .logi
            .bars
            .retain(|id, _| world.balls().any(|b| b.ball_id == *id));
        self.obs.humans = humans.iter().map(|b| ball(b)).collect();
        // CHATTER-2: the ore the laser is on, by name, for the phrase slot.
        self.obs.talk.ore = self
            .obs
            .miner
            .and_then(|m| m.target)
            .and_then(|t| world.get(t))
            .and_then(|b| b.slim?.type_id)
            .and_then(|t| {
                let content = self.config.content.as_deref()?;
                Some(eve_server::typedata::item_in(content, t)?.name.to_owned())
            });
        // R-REL-4: a bot in a fleet shares its grid's hours with the humans.
        if self.obs.fleet.is_some()
            && let (Some(soc), Some(char_id)) = (&self.config.society, me)
            && let Some(hall) = &soc.hall
        {
            let aff = |c: i64, corp: Option<i64>, al: Option<i64>| {
                eve_server::group_contacts::Affiliation {
                    character_id: c,
                    corporation_id: corp.unwrap_or(0),
                    alliance_id: al,
                }
            };
            let on_grid: Vec<_> = world
                .balls()
                .filter_map(|b| {
                    let s = b.slim?;
                    let c = s.char_id?;
                    (c != char_id && !bots.contains(&c)).then(|| aff(c, s.corp_id, s.alliance_id))
                })
                .collect();
            // REVIEW-FIX: only pilots in the fleet, not whoever shares the grid.
            let mates = crate::human::fleet_mates(
                on_grid,
                &self.memory.fleet.joined,
                self.obs.fleet.as_ref().and_then(|f| f.boss),
            );
            crate::human::presence(
                hall,
                &soc.mind,
                &aff(char_id, my_corp, my_alliance),
                &mates,
                unix_now(),
            );
        }
        self.obs.human_types = humans
            .iter()
            .filter_map(|b| Some((b.ball_id, b.slim?.type_id?)))
            .collect();
        // SOC-C5: a hunter sees every pilot, hostile or not.
        // FC-2: a wing sees them too (who shoots the FC, who runs a link).
        if (self.memory.policy.pvp.is_some() || self.memory.policy.wing.is_some())
            && let Some(char_id) = me
        {
            let hall = self.config.society.as_ref().and_then(|s| s.hall.clone());
            let who = eve_server::group_contacts::Affiliation {
                character_id: char_id,
                corporation_id: my_corp.unwrap_or(0),
                alliance_id: my_alliance,
            };
            // R-REL-3: the hunter's own regard counts in `R_eff`.
            let (regard, loyalty) = match &self.config.society {
                Some(soc) => {
                    let m = soc.mind.lock().unwrap_or_else(|e| e.into_inner());
                    (m.regard.clone(), m.traits.get(crate::mind::Trait::Loyalty))
                }
                None => (Vec::new(), 0.5),
            };
            let (pilots, wrecks) = crate::hunt::observe(
                world,
                &crate::hunt::Me {
                    char_id,
                    who,
                    boss,
                    ego_ball: ego.ball_id,
                    hall: hall.as_deref(),
                    regard: &regard,
                    loyalty,
                },
            );
            // A bot's ship with no session is held pilotless: not a foe.
            let server = self.config.addr;
            self.obs.hunt.pilots = pilots
                .into_iter()
                .filter(|p| !departed(server, p.char_id))
                .collect();
            self.obs.hunt.wrecks = wrecks;
            // R-REL-5: a war of mine is hostile now.
            self.obs.hunt.war_on = hall
                .as_deref()
                .is_some_and(|h| h.lock().war_on(&who, eve_server::pyutil::filetime_now()));
            if let Some(soc) = &self.config.society {
                let mind = soc.mind.lock().unwrap_or_else(|e| e.into_inner());
                self.obs.hunt.temper = crate::hunt::Temper {
                    aggression: mind.traits.get(crate::mind::Trait::Aggression),
                    risk: mind.traits.get(crate::mind::Trait::Risk),
                };
                self.obs.hunt.mood = crate::filament::Mood {
                    curiosity: mind.traits.get(crate::mind::Trait::Curiosity),
                    greed: mind.traits.get(crate::mind::Trait::Greed),
                };
            }
            // BRAWL: the reckless end, so a hunter takes a hull one size up too.
            if self.config.policy.brawl {
                self.obs.hunt.temper.aggression = 1.0;
            }
        }
        if self.obs.home.is_none() {
            self.obs.home = view.station.map(|s| s.id);
        }
        self.obs.j_space = conn
            .session()
            .solar_system_id()
            .is_some_and(crate::wh_dive::in_j_space);
        self.obs.park = Some(view);
        self.post_sighting(me);
    }

    /// **DOC-3: tell the leader what this bot sees**, once a battle tick: the
    /// grid's pilots, a fresh full-sphere scan, Local's head count. A
    /// docked or chatless bot posts what it has; nothing is acted on here.
    fn post_sighting(&mut self, me: Option<i64>) {
        use crate::doctrine_sense as ds;
        let Some(board) = self.config.society.as_ref().and_then(|s| s.board.clone()) else {
            return;
        };
        let (Some(me), Place::InSpace { system, .. }) = (me, self.obs.place) else {
            return;
        };
        let now = Instant::now();
        if self.last_posted.is_some_and(|t| {
            now.saturating_duration_since(t) < Duration::from_secs(ds::BATTLE_TICK_S)
        }) {
            return;
        }
        self.last_posted = Some(now);
        let scan = self
            .obs
            .hunt
            .sweep
            .as_ref()
            .filter(|w| {
                w.system == system
                    && now.saturating_duration_since(w.at) < Duration::from_secs(ds::SCAN_STALE_S)
            })
            .map(|w| w.hulls.clone());
        board.post(ds::Sighting {
            observer: me,
            system,
            at_s: unix_now(),
            scan,
            grid: ds::grid_of(&self.obs.hunt.pilots),
            local: self.chat.as_ref().map(|c| ds::local_counts(c.local())),
            spotted: false,
        });
    }

    /// THREAT: **who is locking or shooting this ship** (or a corp mate on
    /// the grid), and who is in Local, for [`crate::threat`]. Only a civilian
    /// that has the sense on pays for it. An aggressor is a pilot who is
    /// not this bot, not the fleet boss, not a friend (`R_eff` at or above
    /// [`crate::threat::FRIEND_AT`]; a bot only when the relations call it
    /// hostile) and not an NPC. Locks are asked of the server
    /// (`GetTargeters`) only while some such pilot is in the park, and at
    /// most every [`crate::threat::LOCK_POLL`].
    fn scan_threat(&mut self, conn: &mut Conn) -> Result<()> {
        let scene = &mut self.obs.threat;
        scene.foes.clear();
        scene.local = self
            .chat
            .as_ref()
            .filter(|c| !c.local().is_empty())
            .map(|c| c.local().members().map(|m| m.char_id).collect());
        let (risk, greed, diligence) = match &self.config.society {
            Some(soc) => {
                let m = soc.mind.lock().unwrap_or_else(|e| e.into_inner());
                (
                    m.traits.get(crate::mind::Trait::Risk),
                    m.traits.get(crate::mind::Trait::Greed),
                    m.traits.get(crate::mind::Trait::Diligence),
                )
            }
            None => {
                let t = crate::mind::Traits::draw(&self.memory.name, SOCIETY_SEED);
                (
                    t.get(crate::mind::Trait::Risk),
                    t.get(crate::mind::Trait::Greed),
                    t.get(crate::mind::Trait::Diligence),
                )
            }
        };
        self.obs.threat.diligence = diligence;
        self.obs.threat.risk = risk;
        self.obs.threat.greed = greed;
        let policy = &self.memory.policy;
        // FC-2: a wing bot reads the foes too (never acts on them here:
        // `threat::act` stays off for a fighter); it is a dev bot as well.
        let wing = policy.wing.is_some();
        if !policy.threat
            || !(policy.role.civilian || wing)
            || (policy.dev.is_some() && !wing)
            || !matches!(self.obs.place, Place::InSpace { bound: true, .. })
        {
            return Ok(());
        }
        let (Some(me), Some(ship)) = (conn.session().char_id(), conn.session().ship_id()) else {
            return Ok(());
        };
        let now = Instant::now();
        self.track
            .player_fire
            .retain(|_, (_, at)| now.saturating_duration_since(*at) < crate::threat::FIRE_STALE);
        let (my_corp, _my_alliance) = (
            conn.session().get_i64("corpid"),
            conn.session().get_i64("allianceid"),
        );
        let bots = self
            .config
            .marker
            .lock()
            .map(|b| b.clone())
            .unwrap_or_default();
        let boss = self.obs.fleet.as_ref().and_then(|f| f.boss);
        // Mind and hall for the friend test.
        let rel = self.relations();
        let (regard, loyalty) = match &rel {
            Some((_, mind)) => {
                let m = mind.lock().unwrap_or_else(|e| e.into_inner());
                (m.regard.clone(), m.traits.get(crate::mind::Trait::Loyalty))
            }
            None => (Vec::new(), 0.5),
        };
        // `(ball, char)` of every other pilot in the park who may be a foe.
        let world = conn.world_now();
        let mut pilots: BTreeMap<i64, (i64, Option<i64>, Option<i64>)> = BTreeMap::new();
        for b in world.balls() {
            let Some(s) = b.slim else { continue };
            let Some(c) = s.char_id else { continue };
            if b.ball_id == ship || c == me || Some(c) == boss || is_npc(b) {
                continue;
            }
            // A pilot of our own player corporation is never a foe (the owner
            // and an allied dev roster share one, `/bots N allied ...`); an
            // NPC starter corporation says nothing about who is a friend.
            if s.corp_id.is_some_and(|c| c >= 98_000_000) && s.corp_id == my_corp {
                continue;
            }
            pilots.insert(b.ball_id, (c, s.corp_id, s.alliance_id));
        }
        let corp_of_ball = |ball: i64| {
            world
                .balls()
                .find(|b| b.ball_id == ball)
                .and_then(|b| b.slim)
                .and_then(|s| s.corp_id)
        };
        let is_foe = |c: i64, corp: Option<i64>, al: Option<i64>| -> bool {
            let who = eve_server::group_contacts::Affiliation {
                character_id: c,
                corporation_id: corp.unwrap_or(0),
                alliance_id: al,
            };
            match &rel {
                Some((hall, _)) => {
                    let r = hall.lock().r_eff_of(
                        my_corp.unwrap_or(0),
                        &regard,
                        loyalty,
                        &who,
                        unix_now(),
                    );
                    crate::threat::is_foe(Some(r), bots.contains(&c))
                }
                None => crate::threat::is_foe(None, bots.contains(&c)),
            }
        };
        // CLOAK-2: a foe who just decloaked within range.
        self.track
            .decloaked
            .retain(|_, at| now.saturating_duration_since(*at) < crate::threat::DECLOAK_FRESH);
        let here = world.get(ship).map(|b| b.motion.position);
        let mut decloaks: Vec<(i64, crate::threat::Cause)> = Vec::new();
        for (&ball, &at) in &self.track.decloaked {
            let (Some(&(c, corp, al)), Some(here), Some(there)) =
                (pilots.get(&ball), here, world.get(ball))
            else {
                continue;
            };
            let d = eve_server::space_state::distance(here, there.motion.position);
            if let Some(cause) = crate::threat::decloak_cause(
                now.saturating_duration_since(at),
                d,
                is_foe(c, corp, al),
            ) {
                decloaks.push((c, cause));
            }
        }
        let mut foes: BTreeMap<i64, crate::threat::Cause> = BTreeMap::new();
        let mut add = |c: i64, cause: crate::threat::Cause| {
            let e = foes.entry(c).or_insert(cause);
            *e = (*e).max(cause);
        };
        for (c, cause) in decloaks {
            add(c, cause);
        }
        // Guns on this ship, and on a corp mate's.
        let player_corp = my_corp.is_some_and(|c| c >= 98_000_000);
        let fc_ship = boss.and_then(|c| world.ship_of(c)).map(|t| t.ball_id);
        let wing_mates: BTreeSet<i64> = self.obs.logi.mates.iter().map(|b| b.id).collect();
        for (&shooter, &(target, _)) in &self.track.player_fire {
            let Some(&(c, corp, al)) = pilots.get(&shooter) else {
                continue;
            };
            if !is_foe(c, corp, al) {
                continue;
            }
            if target == ship {
                add(c, crate::threat::Cause::Shot);
            } else if player_corp && corp_of_ball(target) == my_corp && pilots.contains_key(&target)
            {
                add(c, crate::threat::Cause::Mate);
            } else if wing && (Some(target) == fc_ship || wing_mates.contains(&target)) {
                // FC-2: the FC and the wing's own ships, whoever's corp.
                add(c, crate::threat::Cause::Mate);
            }
        }
        // Locks: asked of the server while a candidate is in the park.
        let any_candidate = pilots.values().any(|&(c, corp, al)| is_foe(c, corp, al));
        if any_candidate
            && self
                .track
                .lockers_at
                .is_none_or(|t| now.saturating_duration_since(t) >= crate::threat::LOCK_POLL)
        {
            self.track.lockers_at = Some(now);
            match conn.targeters() {
                Ok(out) => {
                    self.track.lockers = match out.value().unwrap_or(PyValue::None) {
                        PyValue::List(l) => l
                            .iter()
                            .filter_map(|v| match v {
                                PyValue::Int(n) => Some(*n),
                                _ => None,
                            })
                            .collect(),
                        _ => BTreeSet::new(),
                    };
                }
                Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                Err(_) => self.track.lockers.clear(),
            }
        } else if !any_candidate {
            self.track.lockers.clear();
        }
        for &l in &self.track.lockers {
            if let Some(&(c, corp, al)) = pilots.get(&l)
                && is_foe(c, corp, al)
            {
                add(c, crate::threat::Cause::Locked);
            }
        }
        self.obs.threat.foes = foes
            .into_iter()
            .map(|(char_id, cause)| crate::threat::Foe { char_id, cause })
            .collect();
        Ok(())
    }

    /// THREAT: the brain asked for guards and this bot has no fleet to
    /// broadcast to: put the call on the hall's board.
    fn post_distress(&mut self, conn: &mut Conn) {
        let Some(alert) = &mut self.memory.threat.alert else {
            return;
        };
        if alert.call != crate::threat::Call::Pending {
            return;
        }
        alert.call = crate::threat::Call::Sent;
        let (Some(hall), Some(caller), Some(corp), Some(system)) = (
            self.config.society.as_ref().and_then(|s| s.hall.clone()),
            conn.session().char_id(),
            conn.session().get_i64("corpid"),
            conn.session().solar_system_id(),
        ) else {
            return;
        };
        hall.lock().distress_post(crate::threat::Distress {
            caller,
            corp,
            system,
            at: Instant::now(),
            taken: Vec::new(),
        });
        self.record(
            ActKind::Corp,
            format!("guard call from {caller} in {system}"),
            vec![],
            Verdict::Ok,
        );
    }

    /// THREAT: **an armed corp mate answers a call on the board** (at most
    /// [`crate::threat::MAX_GUARDS`] do): the same `Observation::help` a
    /// friend's `NeedBackup` sets, so the brain's own rule warps it to the
    /// caller. The caller is a corp mate, so there is no `R_eff` to weigh.
    fn weigh_distress(&mut self, conn: &mut Conn) {
        if self.obs.guns.is_empty()
            || self.obs.help.is_some()
            || self.memory.homing.is_some()
            || self.memory.hunt.prey.is_some()
            || !matches!(self.obs.place, Place::InSpace { bound: true, .. })
        {
            return;
        }
        let (Some(hall), Some(me), Some(corp), Some(system)) = (
            self.config.society.as_ref().and_then(|s| s.hall.clone()),
            conn.session().char_id(),
            conn.session().get_i64("corpid"),
            conn.session().solar_system_id(),
        ) else {
            return;
        };
        let caller = hall.lock().distress_claim(me, corp, system, Instant::now());
        if let Some(caller) = caller {
            self.obs.help = Some(caller);
            self.record(
                ActKind::Corp,
                format!("guard for {caller}"),
                vec![],
                Verdict::Ok,
            );
        }
    }

    /// R-REL-3: **weigh a `NeedBackup`** heard since the last turn
    /// ([`crate::effects::help_decision`]): the caller's `R_eff`, the bot's
    /// aggression and loyalty, and whether it is already in a fight of its
    /// own. Same system only (`jumps` 0); the odds are not known until the
    /// bot is on grid, so its own fight rules take over there.
    fn weigh_backup(&mut self, conn: &mut Conn) -> Result<()> {
        let Some(call) = self.track.backup.take() else {
            return Ok(());
        };
        let (Some((hall, mind)), Some(me)) = (self.relations(), conn.session().char_id()) else {
            return Ok(());
        };
        // An unarmed bot has nothing to bring to a fight; one already on its
        // way (stranded) keeps its trip (BOT-FLEET-ORDERS).
        if call.caller == me
            || self.memory.hunt.prey.is_some()
            || self.obs.guns.is_empty()
            || self.stranded.is_some()
        {
            return Ok(());
        }
        let aff = match conn.affiliation_of(call.caller) {
            Ok(a) => a,
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => return Ok(()),
        };
        let corp = conn.session().get_i64("corpid").unwrap_or(0);
        let (regard, loyalty, aggression) = {
            let m = mind.lock().unwrap_or_else(|e| e.into_inner());
            (
                m.regard.clone(),
                m.traits.get(crate::mind::Trait::Loyalty),
                m.traits.get(crate::mind::Trait::Aggression),
            )
        };
        let who = eve_server::group_contacts::Affiliation {
            character_id: call.caller,
            corporation_id: aff.corp,
            alliance_id: aff.alliance,
        };
        let r = hall
            .lock()
            .r_eff_of(corp, &regard, loyalty, &who, unix_now());
        let go = crate::effects::help_decision(&crate::effects::HelpIn {
            r,
            odds: None,
            required: crate::hunt::required_odds(aggression),
            jumps: self.jumps_to(conn, call.system),
            busy: false,
            loyalty,
        });
        if go {
            let here = conn.session().solar_system_id() == Some(call.system);
            self.record(
                ActKind::Corp,
                format!("help {}: R {r:.1}", call.caller),
                vec![],
                Verdict::Ok,
            );
            if here {
                self.obs.help = Some(call.caller);
            } else {
                // Within HELP_JUMPS by the gates: fly there (the bot docks
                // at the first station and the help warps from there).
                self.begin_travel(call.system, "backup");
            }
        }
        Ok(())
    }

    /// **Gate jumps to `system`** by the map and the bot's security floor:
    /// `Some(0)` here, `None` when there is no map or no route
    /// (BOT-FLEET-ORDERS widens a call for help from the same system to
    /// [`crate::effects::HELP_JUMPS`]).
    fn jumps_to(&self, conn: &Conn, system: i64) -> Option<u32> {
        let here = conn.session().solar_system_id()?;
        if here == system {
            return Some(0);
        }
        let map = self.memory.policy.travel.as_ref()?;
        map.jumps(here, system, self.memory.policy.security_floor)
    }

    /// **Start a trip to the first station of `system`** without rehoming:
    /// the stranded way flies it over the gates (`return_home`). `false`
    /// when the map has no station there.
    ///
    /// TODO(PULL-LEASE): this is where `Handle::lend(station)` slots in --
    /// the same trip, recorded as a lease so `work_station()` names the
    /// station while the order stands and `recall()` ends it. Until then
    /// the bot docks there, plays from it, and `stranded` is cleared on
    /// arrival; its `home_station` is never changed, and a reconnect flies
    /// it home.
    fn begin_travel(&mut self, system: i64, why: &str) -> bool {
        let station = self
            .memory
            .policy
            .travel
            .as_ref()
            .and_then(|m| m.stations_in(system).first().copied());
        let Some(station) = station else {
            return false;
        };
        self.stranded = Some((station, 0));
        self.record(
            ActKind::Order,
            format!("travel to system {system} ({why}): station {station}"),
            vec![system, station],
            Verdict::Ok,
        );
        true
    }

    /// R-REL-3: the society's hall and this bot's mind, when it has them.
    fn relations(&self) -> Option<(Arc<crate::corpmind::Hall>, Arc<Mutex<Mind>>)> {
        let soc = self.config.society.as_ref()?;
        Some((soc.hall.clone()?, Arc::clone(&soc.mind)))
    }

    /// R-REL-3: [`crate::contracts::offer_surplus`], with an ally in mind
    /// when the bot has a society ([`crate::effects::offer_surplus_as`]).
    fn offer_surplus(&self, conn: &mut Conn, career: crate::contracts::Career) -> Result<String> {
        match self.relations() {
            Some((hall, mind)) => {
                crate::effects::offer_surplus_as(conn, &hall, &mind, career, unix_now())
            }
            None => crate::contracts::offer_surplus(conn, career),
        }
    }

    /// R-REL-3: [`crate::contracts::take_underpriced`] through the
    /// counterparty gate when the bot has a society.
    fn take_underpriced(&self, conn: &mut Conn) -> Result<crate::contracts::Taken> {
        match self.relations() {
            Some((hall, mind)) => {
                crate::effects::take_underpriced_as(conn, &hall, &mind, unix_now())
            }
            None => crate::contracts::take_underpriced(conn),
        }
    }

    /// One `Wait`: sleep on the socket until the first push or `d`.
    fn wait(&mut self, conn: &mut Conn, d: Duration) -> Result<()> {
        // A session's end is a turn of its own: no wait sleeps through it.
        let d = match self
            .config
            .society
            .as_ref()
            .and_then(|s| s.timetable.until_offline(unix_now()))
        {
            Some(left) => d.min(Duration::from_secs(left).max(SESSION_TICK)),
            None => d,
        };
        // BUBBLE: nor through a release, once the bubble watches this bot.
        let d = if self.handle.bubbled.load(Ordering::SeqCst) {
            d.min(BUBBLE_WAIT)
        } else {
            d
        };
        // Nor through a social turn (SOC-C4).
        let d = match self.config.society.as_ref().and_then(|s| s.hall.as_ref()) {
            Some(h) => d.min(h.every().max(SESSION_TICK)),
            None => d,
        };
        conn.pump(d)?;
        self.absorb_all(conn);
        Ok(())
    }

    /// Sleep on the socket for the whole of `d`, absorbing as it goes.
    fn wait_all(&mut self, conn: &mut Conn, d: Duration) -> Result<()> {
        let until = Instant::now() + d;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() || self.expired() {
                return Ok(());
            }
            conn.pump(left)?;
            self.absorb_all(conn);
        }
    }

    fn absorb_all(&mut self, conn: &mut Conn) {
        for (event, args) in conn.take_pushes() {
            crate::stats::ABSORB.time(|| self.absorb(conn, &event, &args));
        }
    }

    /// **One push, folded into the observation.**
    fn absorb(&mut self, conn: &Conn, event: &str, args: &[PyValue]) {
        // BOT-CAP: the charge and the neuts on this ship.
        self.track
            .cap
            .on_push(event, args, conn.session().ship_id(), Instant::now());
        // BOT-SCRAM: a warp scrambler on this ship.
        self.track
            .scram
            .on_push(event, args, conn.session().ship_id(), Instant::now());
        // The lock range and lock count move under a system's effect beacon
        // (a Black Hole, a Magnetar), a booster or a fitted module: take the
        // server's new number rather than hold the one read at the listing.
        if let Some(ship) = conn.session().ship_id() {
            for (attribute, new) in crate::hulls::ship_attribute_changes(event, args, ship) {
                if attribute == eve_server::typedata::attr::MAX_TARGET_RANGE {
                    self.track.served_lock_range = Some((ship, Some(new)));
                } else if attribute == eve_server::typedata::attr::MAX_LOCKED_TARGETS {
                    self.obs.lock_max = new.max(0.0) as usize;
                }
            }
        }
        match event {
            "OnGodmaShipEffect" => {
                let Some(e) = GodmaEffect::from_push(event, args) else {
                    return;
                };
                // BOT-CAP: a cycle that stopped for want of capacitor.
                if !e.start
                    && let Some(r) = e.error.as_deref().and_then(crate::cap::Refusal::of)
                {
                    self.track.cap.refused(r, Instant::now());
                }
                if let Some(gun) = self.obs.guns.iter_mut().find(|g| g.item_id == e.item_id) {
                    let content = self.config.content.as_deref();
                    if crate::weapon_effect(content, gun.type_id).map(|w| w.1) == Some(e.effect_id)
                    {
                        gun.active = e.start;
                        gun.target = e.target.filter(|_| e.start);
                    }
                    return;
                }
                // T3-SALVAGE: a salvager's cycle ends with a stop push (the
                // server stops it on every outcome); until it is taken in,
                // the module reads as running for good and the ratter waits
                // on it for ever.
                if let Some(s) = &mut self.obs.salvager
                    && e.item_id == s.item_id
                    && e.effect_id == crate::salvage::EFFECT.1
                {
                    s.active = e.start;
                    s.target = e.target.filter(|_| e.start);
                    return;
                }
                let Some(laser) = &mut self.obs.miner else {
                    return;
                };
                if e.item_id != laser.item_id || e.effect_id != crate::MINING_LASER.1 {
                    return;
                }
                if e.start {
                    laser.active = true;
                    laser.target = e.target;
                } else {
                    laser.active = false;
                    laser.target = None;
                    self.stops += 1;
                    self.obs.last_stop = Some(Stop {
                        seq: self.stops,
                        reason: StopReason::from_key(e.error.as_deref()),
                    });
                }
            }
            // L7 BOT-ECON: the server's motherlode message, to every pilot in
            // the system. A foreman who gets it says one line in Local.
            "OnServerMessage" => {
                if let Some(text) = args.first().and_then(crate::text)
                    && let Some(system) = conn.session().solar_system_id()
                {
                    self.memory.told.note(&text, system, Instant::now());
                }
            }
            "OnTarget" => {
                let what = args.first().and_then(crate::text);
                let target = match args.get(1) {
                    Some(PyValue::Int(t)) => Some(*t),
                    _ => None,
                };
                match (what.as_deref(), target) {
                    // Only a lock this bot is waiting for. A completion is
                    // a push queued at `AddTarget` time and cannot be
                    // recalled, so one can land after its lock was dropped
                    // -- by the range sweep, or by docking -- naming a lock
                    // the server no longer holds (`targeting::lock`, "the
                    // race, stated rather than hidden").
                    (Some("add"), Some(t)) if self.obs.pending.contains(&t) => {
                        self.obs.pending.retain(|&p| p != t);
                        if !self.obs.locked.contains(&t) {
                            self.obs.locked.push(t);
                        }
                    }
                    (Some("lost"), Some(t)) => {
                        self.obs.pending.retain(|&p| p != t);
                        self.obs.locked.retain(|&p| p != t);
                        // **A rock lost "out of range" that the park says
                        // is in range is a desync**, the same disagreement
                        // as a `TargetTooFar` stop -- and the client's cure
                        // for a desync is a fresh state. Seen for real: the
                        // server's range sweep reads a follower where it was
                        // at its last command, not on its tether
                        // (`targeting::sweep_out_of_range`; the P8b note).
                        let out_of_range =
                            args.get(2).and_then(crate::text).as_deref() == Some("OutOfRange");
                        if out_of_range && crate::park::is_rock(t) {
                            self.stops += 1;
                            self.obs.last_stop = Some(Stop {
                                seq: self.stops,
                                reason: StopReason::TooFar,
                            });
                        }
                    }
                    (Some("clear"), _) => {
                        self.obs.pending.clear();
                        self.obs.locked.clear();
                    }
                    _ => {}
                }
            }
            "OnFleetInvite" => {
                if let Some(i) = crate::FleetInvite::from_push(event, args) {
                    self.obs.invite = Some(Invite {
                        fleet_id: i.fleet_id,
                        inviter_id: i.inviter_id,
                        inviter_name: self.track.names.get(&i.inviter_id).cloned(),
                    });
                }
            }
            // CAP-L: the fields the fleet holds, for a capital pair.
            "OnFleetJumpBeaconModuleChange" => {
                self.memory.capital.beacons.on_push(event, args);
            }
            // SOC-C7: the fleet this bot commands, and a primary called.
            "OnFleetJoin" | "OnFleetLeave" | "OnFleetDisbanded" | "OnFleetBroadcast" => {
                self.memory.fleet.on_push(event, args);
                if let Some(p) = crate::fleet::primary_call(event, args, self.obs.fleet) {
                    self.obs.primary = Some(p);
                }
                // BOT-FLEET-ORDERS: the boss's orders, a mate's call for
                // reps, a call for help. Our own broadcasts are not heard.
                let me = conn.session().char_id();
                if let Some((heard, sender)) =
                    crate::fleet_orders::read(event, args, self.obs.fleet)
                    && Some(sender) != me
                {
                    match heard {
                        crate::fleet_orders::Heard::Backup { caller, system, .. } => {
                            self.track.backup = Some(crate::fleet::BackupCall { caller, system });
                        }
                        other => {
                            crate::fleet_orders::hear(&mut self.memory, other, Instant::now());
                        }
                    }
                }
            }
            // FC-2: the fleet's target tags, whole, on every change
            // (`fleet.rs::state_value`).
            "OnFleetStateChange" => {
                if self.memory.policy.wing.is_some()
                    && let Some(tags) = args.first().and_then(crate::wing_fight::parse_tags)
                {
                    self.obs.wing.tags = tags;
                }
            }
            // `OnSpecialFX(shipID, moduleID, moduleTypeID, targetID, ...,
            // start, ...)` (`modules.rs::SpecialFx`) **[V]**: other ships'
            // lasers, so two bots do not share a rock.
            "OnSpecialFX" => {
                let int = |i: usize| match args.get(i) {
                    Some(PyValue::Int(n)) => Some(*n),
                    Some(PyValue::Bool(b)) => Some(i64::from(*b)),
                    _ => None,
                };
                // BP-COUNTER: another ship's smartbomb wave or drive engaging.
                if let Some(PyValue::Bytes(guid)) = args.get(5)
                    && let Ok(guid) = std::str::from_utf8(guid)
                    && let Some(ship) = int(0)
                    && Some(ship) != conn.session().ship_id()
                {
                    self.track.counter.on_fx(guid, ship, Instant::now());
                }
                // CLOAK-2: a ship uncloaked (`cloak::uncloak`'s effect).
                if let Some(PyValue::Bytes(guid)) = args.get(5)
                    && guid.as_slice() == b"effects.Uncloak"
                {
                    if let Some(ship) = int(0) {
                        self.track.decloaked.insert(ship, Instant::now());
                    }
                    return;
                }
                let (Some(ship), Some(type_id)) = (int(0), int(2)) else {
                    return;
                };
                // FC-2: a ship running an Entosis Link, for the wing's
                // contest rule and the logi's wards. **[A]** the server
                // sends the effect for a link as for any module; falsified
                // by a link running with `obs.wing.links` empty.
                if self.memory.policy.wing.is_some() {
                    let content = self.config.content.as_deref();
                    let links = self.track.link_types.get_or_insert_with(|| {
                        content
                            .map(crate::sov_bot::link_types)
                            .unwrap_or_default()
                            .into_iter()
                            .collect()
                    });
                    if links.contains(&type_id) {
                        match (int(3), int(7)) {
                            (Some(target), Some(1)) => {
                                self.obs.wing.links.insert(ship, target);
                            }
                            _ => {
                                self.obs.wing.links.remove(&ship);
                            }
                        }
                        return;
                    }
                }
                // An NPC's gun: `shipID == moduleID == the NPC`
                // (`npc::gun_fx`) **[V]** -- who it is shooting.
                if int(1) == Some(ship) && conn.world().get(ship).is_some_and(is_npc) {
                    match (int(3), int(7)) {
                        (Some(target), Some(1)) => {
                            self.track.aggressors.insert(ship, target);
                        }
                        _ => {
                            self.track.aggressors.remove(&ship);
                        }
                    }
                    return;
                }
                // THREAT: another pilot's offensive effect (a gun, not a
                // mining laser or a remote repair) names a ship.
                if Some(ship) != conn.session().ship_id() && int(6) == Some(1) {
                    match (int(3), int(7)) {
                        (Some(target), Some(1)) => {
                            self.track
                                .player_fire
                                .insert(ship, (target, Instant::now()));
                        }
                        _ => {
                            self.track.player_fire.remove(&ship);
                        }
                    }
                    return;
                }
                if Some(ship) == conn.session().ship_id() || !brain::is_mining_laser(type_id) {
                    return;
                }
                match (int(3), int(7)) {
                    (Some(rock), Some(1)) if crate::park::is_rock(rock) => {
                        self.track.lasers.insert(ship, rock);
                    }
                    _ => {
                        self.track.lasers.remove(&ship);
                    }
                }
            }
            "OnItemChange" | "OnInvalidateCacheForLocations" => {
                self.track.docked_dirty = true;
                self.track.cargo_dirty = true;
            }
            // A charge newly loaded: `OnGodmaPrimeItem(shipID, row)`, the
            // row keyed `(shipID, flagID, typeID)` with its count in 805.
            "OnGodmaPrimeItem" => {
                let Some(row) = args.get(1) else { return };
                let key = crate::dict_get(crate::attributes(row), "itemID");
                if let (Some((_, flag, type_id)), Some(n)) =
                    (key.and_then(sublocation), crate::verbs::quantity_of(row))
                {
                    self.set_rounds(flag, type_id, n);
                }
            }
            // A loaded charge's count moved: a list of 8-tuples
            // `(name, ownerID, key, 805, time, new, old, wallclock)`.
            "OnModuleAttributeChanges" => {
                let Some(PyValue::List(changes)) = args.first() else {
                    return;
                };
                for c in changes {
                    let PyValue::Tuple(t) = c else { continue };
                    if t.get(3) != Some(&PyValue::Int(crate::ATTRIBUTE_QUANTITY)) {
                        continue;
                    }
                    if let (Some((_, flag, type_id)), Some(PyValue::Int(n))) =
                        (t.get(2).and_then(sublocation), t.get(5))
                    {
                        self.set_rounds(flag, type_id, *n);
                    }
                }
            }
            // DOC-3: the combat log line. `damageAttributes` (the damage
            // types) and `weapon` ride on the victim's line
            // (`damage::Shot::victim_message`) **[V]** `uilog.py`.
            "OnDamageMessage" => {
                if let Some(hit) = crate::doctrine_sense::parse_damage(args) {
                    let group = hit
                        .attacker
                        .and_then(|b| conn.world().get(b))
                        .and_then(|b| b.slim)
                        .and_then(|s| {
                            Some(crate::society::group_of(
                                &eve_server::group_contacts::Affiliation {
                                    character_id: s.char_id?,
                                    corporation_id: s.corp_id.unwrap_or(0),
                                    alliance_id: s.alliance_id,
                                },
                            ))
                        });
                    self.sensor.on_damage(&hit, group);
                }
            }
            // The ship's own bars: `OnDamageStateChange(shipID, state)`
            // inside a destiny batch (`destiny::DamageStateChange`) **[V]**.
            "DoDestinyUpdate" => {
                let Some(ship) = conn.session().ship_id() else {
                    return;
                };
                for (_, name, ops) in crate::read_action_list(args) {
                    if name != "OnDamageStateChange" {
                        continue;
                    }
                    let (Some(PyValue::Int(who)), Some(h)) =
                        (ops.first(), ops.get(1).and_then(health_of))
                    else {
                        continue;
                    };
                    if *who == ship {
                        self.obs.health = Some(h);
                    } else if self.obs.logi.bars.len() < crate::logi::MAX_BARS {
                        // K-LOGI: the others' bars, for the wards.
                        self.obs.logi.bars.insert(*who, h);
                    }
                }
            }
            _ => {}
        }
    }

    fn set_rounds(&mut self, flag: i64, _type_id: i64, n: i64) {
        if let Some(g) = self.obs.guns.iter_mut().find(|g| g.flag_id == flag) {
            g.rounds = Some(n);
        }
    }

    /// **Farewell before a gate jump** (CHATTER-2). The trips that jump run
    /// with a `Conn` and no chat, so for an act that may jump in space the
    /// chat and a [`FarewellKit`] move into a hook on the connection
    /// ([`Conn::before_jump`]); the hook asks the chatter's governor and
    /// says the line in the OLD system's Local before `CmdStargateJump` is
    /// sent. [`Self::disarm_farewell`] takes everything back.
    fn arm_farewell(&mut self, conn: &mut Conn, act: &Act) -> Option<Armed> {
        if matches!(
            act,
            Act::Say(_)
                | Act::Wait(_)
                | Act::Missing(_)
                | Act::Reconnect
                | Act::AlignTo(_)
                | Act::Stop
                | Act::TravelTo { .. }
        ) || !matches!(self.obs.place, Place::InSpace { .. })
        {
            return None;
        }
        let chat = self.chat.take()?;
        let shared = Rc::new(RefCell::new(FarewellJob {
            chat: Some(chat),
            kit: FarewellKit::of(&self.memory, &self.obs),
            said: Vec::new(),
            lost: None,
        }));
        let job = shared.clone();
        conn.before_jump(Some(Box::new(move |c: &mut Conn| {
            let mut j = job.borrow_mut();
            let FarewellJob {
                chat,
                kit,
                said,
                lost,
            } = &mut *j;
            let Some(ch) = chat.as_mut() else {
                return;
            };
            let Some(line) = kit.line(Instant::now()) else {
                return;
            };
            match ch.say(c, &line) {
                Ok(Ok(_)) => said.push(line),
                Ok(Err(why)) => {
                    *lost = Some(why);
                    *chat = None;
                }
                // A dead connection: the jump finds it too.
                Err(_) => {}
            }
        })));
        Some(Armed(shared))
    }

    fn disarm_farewell(&mut self, conn: &mut Conn, armed: Option<Armed>) {
        let Some(Armed(shared)) = armed else {
            return;
        };
        conn.before_jump(None);
        // The hook is gone with the connection's reference; this is the last.
        let Ok(cell) = Rc::try_unwrap(shared) else {
            self.obs.chat = false;
            return;
        };
        let job = cell.into_inner();
        self.memory.chatter = job.kit.state;
        match (job.chat, job.lost) {
            (Some(chat), _) => self.chat = Some(chat),
            (None, why) => {
                self.obs.chat = false;
                self.record(
                    ActKind::Chat,
                    "lost".to_owned(),
                    vec![],
                    Verdict::Failed(why.unwrap_or_else(|| "chat dropped".to_owned())),
                );
            }
        }
        for line in job.said {
            self.record(
                ActKind::Say,
                format!("farewell: {line}"),
                vec![],
                Verdict::Ok,
            );
        }
    }

    /// Execute one act, log it, and fold its verdict into `failures`.
    fn act(&mut self, conn: &mut Conn, act: Act) -> Result<()> {
        let kind = act.kind();
        let (mut what, ids) = describe(&act);
        self.note = None;
        let armed = self.arm_farewell(conn, &act);
        let done = self.execute(conn, &act);
        self.disarm_farewell(conn, armed);
        // An errand that ended away from home strands the bot (SOC-A2).
        if matches!(
            kind,
            ActKind::Market | ActKind::Trade | ActKind::Acquire | ActKind::Ice | ActKind::Pi
        ) && let Some(home) = self.work_station()
            && conn.session().station_id() != Some(home)
            && self.stranded.is_none()
        {
            self.stranded = Some((home, 0));
        }
        if let Some(note) = self.note.take() {
            what = format!("{what}: {note}");
        }
        let verdict = match done {
            Ok(()) => Verdict::Ok,
            Err(Error::NotSent(w)) => Verdict::NotSent(w),
            Err(Error::Refused(w)) => Verdict::Refused(w),
            Err(e) => {
                self.record(kind, what, ids, Verdict::Failed(e.to_string()));
                return Err(e);
            }
        };
        if kind == ActKind::WarpToMember {
            // A refused warp to the boss is a fact about him, not a failure
            // of ours (BOT-REDOCK): back off, and work on.
            self.obs.boss_unreachable_until =
                matches!(verdict, Verdict::Refused(_)).then(|| Instant::now() + brain::BOSS_RETRY);
        }
        // BOT-SCRAM: a scrambled warp or jump is a fact about the ship; it
        // marks the scramble and is not a failure.
        let scrambled = matches!(&verdict, Verdict::Refused(w) if crate::scram::is_refusal(w));
        if scrambled {
            self.track.scram.refused(Instant::now());
        }
        // BP-HOLD: a warp refused inside a bubble is a fact about the ship
        // too (and its key must not read as a scramble).
        let bubbled = matches!(&verdict, Verdict::Refused(w) if crate::warp_bubble::is_refusal(w));
        if let Verdict::Refused(w) = &verdict
            && bubbled
        {
            self.track.warp_bubble.refused(w, Instant::now());
        }
        // MINER-PATIENCE: a lock the ECM jam refused is a fact about the ship,
        // not a failure; the brain waits and retries (`brain::LockJam`).
        let jammed = kind == ActKind::Lock
            && matches!(&verdict, Verdict::Refused(w)
                if brain::jam_refusal(w, self.obs.locked.len() + self.obs.pending.len()));
        if kind == ActKind::Lock {
            let now = Instant::now();
            self.obs.lock_jam = match (jammed, self.obs.lock_jam) {
                (true, Some(j)) => Some(brain::LockJam { last: now, ..j }),
                (true, None) => Some(brain::LockJam {
                    since: now,
                    last: now,
                }),
                (false, _) if verdict.is_ok() => None,
                (false, j) => j,
            };
        }
        // WARP-LOCK: a lock refused because the target is in warp means he
        // left; not a failure (no Recovery, the hunt keeps its quarry).
        let target_left = kind == ActKind::Lock
            && matches!(&verdict, Verdict::Refused(w) if brain::target_left_refusal(w));
        if verdict.is_ok() {
            self.obs.failures.remove(&kind);
        } else if !kind.is_incidental() && !scrambled && !bubbled && !jammed && !target_left {
            *self.obs.failures.entry(kind).or_insert(0) += 1;
        }
        self.record(kind, what, ids, verdict);
        Ok(())
    }

    /// FE-TRADE: the trader's desk reads its file once (`--state DIR`) and
    /// takes the difficulty's speculation knobs; a game day is the
    /// society's timetable cycle over [`WEEK_DAYS`] (else [`DAY_SECS`]).
    fn attach_desk(&mut self) {
        let soc = self.config.society.as_ref();
        let day = soc
            .map(|s| s.timetable.period / WEEK_DAYS)
            .filter(|d| *d > 0)
            .unwrap_or(DAY_SECS);
        let dir = soc.and_then(|s| s.state.as_deref());
        if let Some(why) = self.memory.desk.attach(dir, &self.config.name, day) {
            eprintln!("[trade] {}: {why}", self.config.name);
        }
        self.memory.desk.knobs = self.memory.policy.trader.spec();
    }

    /// Write the desk if it changed; a failure is logged, never fatal.
    fn save_desk(&mut self) {
        if let Err(e) = self.memory.desk.save() {
            eprintln!("[trade] {}: desk not saved: {e}", self.config.name);
        }
    }

    /// **The boarding chokepoint's skill check** (SKILLGATE): whether the
    /// hull at `ship` asks for a skill the pilot has not trained. If so, the
    /// lack goes to the front of the skill queue, the hull type is set aside
    /// (`combat.untrained`, never `refused`: it is only untrained), a note
    /// says so, and the caller waits like any missing thing. A bot with no
    /// content, a hull that is not in the hangar or a refused read of its
    /// skills boards as before.
    fn hull_untrained(&mut self, conn: &mut Conn, ship: i64) -> Result<bool> {
        let Some(hull) = conn
            .hangar_items()?
            .iter()
            .find(|r| r.item_id == ship)
            .map(|r| r.type_id)
        else {
            return Ok(false);
        };
        if crate::acquire::direct_requirements(hull).is_empty() {
            return Ok(false);
        }
        let levels = match crate::acquire::skill_levels(conn) {
            Ok(l) => l,
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => return Ok(false),
        };
        let lack = crate::acquire::lacking_of(&levels, hull);
        if lack.is_empty() {
            return Ok(false);
        }
        // Open mode is the GM's: the skill is granted, not waited for, and
        // nothing is left set aside (`skill_plan_tick` does not run there).
        // A grant sets the level, and only what is lacking is sent.
        if !self.memory.policy.closed {
            for (skill, level) in &lack {
                let line = format!("/giveskill {skill} {level}");
                let out = crate::gm::slash(conn, &line)?;
                let verdict = match out.refusal() {
                    None => Verdict::Ok,
                    Some(key) => Verdict::Refused(key),
                };
                self.record(ActKind::Slash, line, vec![], verdict);
            }
            self.memory.combat.untrained.retain(|t| *t != hull);
            return Ok(false);
        }
        for what in crate::acquire::learn_urgent_soft(conn, &lack)? {
            self.record(ActKind::Missing, what, vec![], Verdict::Ok);
        }
        if !self.memory.combat.untrained.contains(&hull) {
            self.memory.combat.untrained.push(hull);
        }
        // A line of its own, at once: the wait that follows is a minute long.
        self.record(
            ActKind::Missing,
            format!("hull {hull} lacks skills {lack:?}; waiting"),
            vec![],
            Verdict::Ok,
        );
        Ok(true)
    }

    /// A fit's answer (SKILLGATE): fitted, or waiting on skills -- a note
    /// and the missing-thing wait, never a failure.
    fn fit_gated(
        &mut self,
        conn: &mut Conn,
        fitted: std::result::Result<Vec<crate::ItemRow>, String>,
    ) -> Result<()> {
        match fitted {
            Ok(_) => {
                self.track.docked_dirty = true;
                Ok(())
            }
            Err(note) => {
                self.record(ActKind::Missing, note, vec![], Verdict::Ok);
                self.wait(conn, brain::MISSING_WAIT)
            }
        }
    }

    fn execute(&mut self, conn: &mut Conn, act: &Act) -> Result<()> {
        match *act {
            Act::GmSetup { ship, module, ammo } => self.setup(conn, ship, module, ammo),
            Act::Acquire { ship, module, ammo } => self.acquire(conn, ship, module, ammo),
            Act::Board(ship) => {
                if self.hull_untrained(conn, ship)? {
                    return self.wait(conn, brain::MISSING_WAIT);
                }
                conn.board(ship)?;
                self.track.docked_dirty = true;
                Ok(())
            }
            Act::FitAuto(item) => {
                let fitted = crate::acquire::fit_auto_gated(conn, &[item])?;
                self.fit_gated(conn, fitted)
            }
            Act::FitAutoN { item, n } => {
                let fitted = crate::acquire::fit_auto_gated(conn, &vec![item; n])?;
                self.fit_gated(conn, fitted)
            }
            Act::OnlineMany(ref modules) => {
                for &module in modules {
                    if let Err(note) = crate::acquire::set_online_gated(conn, module)? {
                        self.record(ActKind::Missing, note, vec![], Verdict::Ok);
                        return self.wait(conn, brain::MISSING_WAIT);
                    }
                }
                Ok(())
            }
            Act::Online(module) => {
                if let Err(note) = crate::acquire::set_online_gated(conn, module)? {
                    self.record(ActKind::Missing, note, vec![], Verdict::Ok);
                    return self.wait(conn, brain::MISSING_WAIT);
                }
                if let Some(l) = &mut self.obs.miner {
                    l.online = true;
                }
                Ok(())
            }
            Act::Undock => {
                // GAP-REPAIR-BOT: docking no longer mends a hull, so a bot
                // pays the repair shop before it leaves; a refusal or a
                // wallet too thin is a note, never a stuck undock.
                let note = match conn.repair_ship() {
                    Ok(note) => note,
                    Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                    Err(e) => format!("repair failed: {e}"),
                };
                if !note.is_empty() {
                    self.note = Some(note);
                }
                // X-INSURE: a combat hull is covered before it leaves the
                // station; a refusal is a note, never a stuck undock.
                let note = crate::insure::cover(conn, self.memory.policy.role.name())?;
                if !note.is_empty() {
                    self.note = Some(match self.note.take() {
                        Some(n) => format!("{n}; {note}"),
                        None => note,
                    });
                }
                // X-RIG: fit the rigs the hangar holds, buy one the career
                // lacks (closed economy); a refusal is a note.
                if let Some(content) = self.config.content.clone() {
                    let note = crate::rig::outfit(
                        conn,
                        &content,
                        self.memory.policy.role,
                        self.memory.policy.closed,
                        &mut self.memory.rig,
                        self.memory.now,
                    )?;
                    if !note.is_empty() {
                        self.note = Some(note);
                    }
                    // PROG-PVE2: a fighter fits its race's tank beside the
                    // rigs (closed economy only, `crate::tank`).
                    // DOC-5: a bot flying its doctrine's hull fits the role
                    // fit instead (one FitAuto), not the race's tank.
                    let doctrine_hull = self
                        .memory
                        .combat
                        .doctrine
                        .clone()
                        .filter(|d| self.obs.ship.and_then(|s| s.type_id) == Some(d.hull));
                    if let Some(fit) = doctrine_hull {
                        let note = crate::doctrine_assign::outfit(conn, &fit)?;
                        if !note.is_empty() {
                            self.track.docked_dirty = true;
                            self.note = Some(match self.note.take() {
                                Some(n) => format!("{n}; {note}"),
                                None => note,
                            });
                        }
                    } else if self.memory.policy.closed
                        && crate::progression::climbs(self.memory.policy.role)
                        && let Some(race) = crate::drone_use::race_of(&self.memory)
                    {
                        let note = crate::tank::outfit(
                            conn,
                            &content,
                            race,
                            true,
                            &mut self.memory.combat.tank,
                            self.memory.now,
                        )?;
                        if !note.is_empty() {
                            self.note = Some(match self.note.take() {
                                Some(n) => format!("{n}; {note}"),
                                None => note,
                            });
                        }
                    }
                }
                let mut online: Vec<(i64, i64)> = self
                    .obs
                    .miner
                    .filter(|l| l.online)
                    .map(|l| (l.flag_id, l.item_id))
                    .into_iter()
                    .collect();
                online.extend(
                    self.obs
                        .guns
                        .iter()
                        .filter(|g| g.online)
                        .map(|g| (g.flag_id, g.item_id)),
                );
                conn.undock_with(&online).map(|_| ())
            }
            Act::Bind => {
                let Some(system) = conn.session().solar_system_id() else {
                    return Err(Error::NotSent("bind: not in space".to_owned()));
                };
                conn.bind_ballpark(system)?;
                if !conn.world().has_state() {
                    return Err(Error::Refused("bind: no SetState followed".to_owned()));
                }
                Ok(())
            }
            // **The backstop for the minimum warp distance** (WARP-MIN): the
            // server refuses a warp that lands inside 150 km, so a decision
            // that slipped through (a stale park, a caller without a ball
            // in hand) approaches instead of issuing it.
            Act::WarpToItem { id, range } => {
                if warp_gap(conn, Some(id))
                    .is_some_and(|g| crate::escort::warp_lands_short(g, range))
                {
                    return self.execute(
                        conn,
                        &Act::Follow {
                            id,
                            range: range.max(crate::escort::WRECK_APPROACH),
                        },
                    );
                }
                conn.warp_to_item(id, range)?;
                self.await_warp(conn)
            }
            Act::WarpToMember { char_id, range } => {
                let ship = conn.world().ship_of(char_id).map(|b| b.ball_id);
                if let Some(id) = ship
                    && warp_gap(conn, ship)
                        .is_some_and(|g| crate::escort::warp_lands_short(g, range))
                {
                    return self.execute(
                        conn,
                        &Act::Follow {
                            id,
                            range: range.max(crate::escort::WRECK_APPROACH),
                        },
                    );
                }
                conn.warp_to_member(char_id, range).map(|_| ())
            }
            Act::Follow { id, range } => {
                if self.move_repeats(conn, false, id, range) {
                    return Ok(());
                }
                conn.follow(id, range)?;
                self.track.last_move = conn.session().ship_id().map(|s| (s, false, id, range));
                full_throttle(conn)?;
                self.await_tether(conn, id)
            }
            Act::Lock(target) => {
                let state = match conn.lock_target(target) {
                    Ok(s) => s,
                    // CLOAK-2: still recalibrating after a decloak: hold the
                    // lock back by the module's own delay, keep the prey.
                    Err(Error::Refused(why)) => {
                        let now = Instant::now();
                        return match crate::hunt::lock_hold(&why, self.obs.hunt.cloak.as_ref(), now)
                        {
                            Some(at) => {
                                self.memory.hunt.lock_hold = Some(at);
                                self.note = Some("held: DeniedTargetAfterCloak".to_owned());
                                Ok(())
                            }
                            None => Err(Error::Refused(why)),
                        };
                    }
                    Err(e) => return Err(e),
                };
                match state {
                    LockState::Locked => {
                        if !self.obs.locked.contains(&target) {
                            self.obs.locked.push(target);
                        }
                    }
                    LockState::Pending => {
                        if !self.obs.pending.contains(&target) {
                            self.obs.pending.push(target);
                        }
                    }
                }
                Ok(())
            }
            Act::Unlock(target) => {
                conn.unlock_target(target)?;
                self.obs.locked.retain(|&t| t != target);
                Ok(())
            }
            Act::Mine { module, rock } => match conn.mine(module, rock) {
                Ok(e) => {
                    self.track.cap.ran();
                    if let Some(l) = &mut self.obs.miner {
                        l.active = true;
                        l.target = e.target.or(Some(rock));
                    }
                    Ok(())
                }
                Err(Error::Refused(why)) => {
                    // BOT-CAP: no capacitor, a delay or a crowd is a hold,
                    // not a failure; the brain's gate keeps the laser off.
                    if let Some(r) = crate::cap::Refusal::of(&why) {
                        self.track.cap.refused(r, Instant::now());
                        self.note = Some(format!("held: {r:?}"));
                        return Ok(());
                    }
                    // Already running -- a cycle that outlived a dropped
                    // connection: read the module again, so the brain sees
                    // it running and stops it rather than ask again.
                    if why.contains("EffectAlreadyActive2") {
                        self.track.cargo_dirty = true;
                        self.track.docked_dirty = true;
                    }
                    // Not locked, by the server's word: believe it, so the
                    // brain locks again rather than activate again.
                    if why.contains("DeniedActivateTargetNotPresent") {
                        self.obs.locked.retain(|&t| t != rock);
                    }
                    // A refusal whose key is a stop reason is that stop:
                    // `NotEnoughCargoSpace` is a full hold to the brain.
                    if let Some(key) = known_key(&why) {
                        self.stops += 1;
                        self.obs.last_stop = Some(Stop {
                            seq: self.stops,
                            reason: StopReason::from_key(Some(key)),
                        });
                    }
                    Err(Error::Refused(why))
                }
                Err(e) => Err(e),
            },
            Act::StopMining(module) => conn.deactivate(module, crate::MINING_LASER.0).map(|_| ()),
            Act::Dock(station) => {
                // BOT-CHURN: every dock names its reason in the act log.
                self.note = Some(format!("why: {}", self.memory.dock_reason()));
                conn.dock(station)?;
                self.track.docked_dirty = true;
                Ok(())
            }
            Act::Unload(ref stacks) => {
                let mut moved = 0;
                let mut last = Ok(());
                for &(item, quantity) in stacks {
                    match conn.unload(item, None) {
                        Ok(_) => moved += quantity,
                        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                        Err(e) => last = Err(e),
                    }
                }
                self.ore_unloaded += moved;
                self.track.docked_dirty = true;
                last
            }
            Act::AcceptInvite { fleet } => {
                let ship_type = self
                    .obs
                    .ship
                    .and_then(|s| s.type_id)
                    .unwrap_or(brain::CAPSULE);
                let inviter = self.obs.invite.as_ref().map(|i| i.inviter_id);
                // SOC-C7: `--accept-fleet allied` asks who the inviter is.
                if self.memory.policy.accept_fleet == brain::AcceptFleet::Allied
                    && let Some(who) = inviter
                    && !crate::fleet::inviter_is_allied(conn, who)?
                {
                    self.obs.invite = None;
                    self.memory.declined.push(fleet);
                    return Err(Error::NotSent(format!(
                        "fleet {fleet}: inviter {who} is not of my corp or alliance"
                    )));
                }
                let result = conn.accept_fleet_invite(fleet, ship_type);
                if result.is_ok() || self.obs.failures.get(&ActKind::AcceptInvite) >= Some(&2) {
                    self.obs.invite = None;
                }
                result?;
                self.obs.fleet = Some(FleetView {
                    fleet_id: fleet,
                    boss: inviter,
                });
                Ok(())
            }
            Act::Courier { agent } => {
                let hauled = crate::courier::run(conn, agent);
                self.track.docked_dirty = true;
                self.track.cargo_dirty = true;
                let d = hauled?;
                if !d.completed {
                    return Err(Error::Refused(format!(
                        "courier: agent {agent} did not call the mission complete"
                    )));
                }
                Ok(())
            }
            Act::Sites => crate::site_runner::scout(conn, &crate::site_runner::BOARD).map(|_| ()),
            Act::Ore(op) => {
                if op == crate::ore::OreAct::Stock {
                    self.track.docked_dirty = true;
                }
                match crate::ore::act(conn, &mut self.memory, op) {
                    Err(Error::Refused(why)) if crate::cap::Refusal::of(&why).is_some() => {
                        // BOT-CAP: a hold, not a failure.
                        if let Some(r) = crate::cap::Refusal::of(&why) {
                            self.track.cap.refused(r, Instant::now());
                            self.note = Some(format!("held: {r:?}"));
                        }
                        Ok(())
                    }
                    done => done,
                }
            }
            Act::Drone(op) => self.drone(conn, op),
            Act::Huff => {
                // Y-WH: a huffer dives when the market pays.
                let trip = if crate::wh_dive::prefer(conn, self.memory.seed) {
                    crate::wh_dive::run_gas(conn, &self.memory.name, self.memory.seed)
                } else {
                    crate::huffer::run(
                        conn,
                        crate::huffer::FINDER,
                        &self.memory.name,
                        self.memory.seed,
                    )
                };
                self.track.docked_dirty = true;
                self.track.cargo_dirty = true;
                let trip = trip?;
                self.isk_sold += trip.after - trip.before;
                self.note = Some(trip.summary());
                Ok(())
            }
            Act::Dive => {
                let trip = crate::wh_dive::run_gas(conn, &self.memory.name, self.memory.seed);
                self.track.docked_dirty = true;
                self.track.cargo_dirty = true;
                let trip = trip?;
                self.isk_sold += trip.after - trip.before;
                self.note = Some(trip.summary());
                Ok(())
            }
            Act::Raid(op) => {
                let done = crate::wh_raid::act(conn, &mut self.memory, op);
                if let Some(note) = self.memory.raid.note.take() {
                    self.note = Some(note);
                }
                done
            }
            Act::Ice => {
                let map = self.memory.policy.travel.clone();
                let trips = map.as_deref().map(|map| crate::travel::Trips {
                    map,
                    floor: self.memory.policy.security_floor,
                });
                let trip = crate::ice::run(conn, trips, &self.memory.name, self.memory.seed);
                self.track.docked_dirty = true;
                self.track.cargo_dirty = true;
                let trip = trip?;
                self.isk_sold += trip.after - trip.before;
                self.note = Some(trip.summary());
                Ok(())
            }
            Act::Mission { agent, level } => {
                let map = self.memory.policy.travel.clone();
                let errand = crate::missioner::Errand {
                    agent,
                    level,
                    trips: map.as_deref().map(|map| crate::travel::Trips {
                        map,
                        floor: self.memory.policy.security_floor,
                    }),
                };
                let done = crate::missioner::run(conn, &errand);
                self.track.docked_dirty = true;
                self.track.cargo_dirty = true;
                let report = done?;
                self.note = Some(report.summary());
                Ok(())
            }
            Act::Explore => {
                let map = self.memory.policy.travel.clone();
                let trips = map.as_deref().map(|map| crate::travel::Trips {
                    map,
                    floor: self.memory.policy.security_floor,
                });
                let nerve = self.roam_ctx(map.as_deref()).nerve;
                let done = crate::probe::explore_with(conn, &crate::probe::LEDGER, trips, nerve);
                self.track.docked_dirty = true;
                self.track.cargo_dirty = true;
                let done = done?;
                self.note = Some(done.summary());
                if done.waits_on_skills {
                    // SKILLGATE: no launcher fits yet; the acts that follow
                    // (the undock) are dropped and the turn waits like for
                    // any missing thing.
                    self.defer_rest = true;
                    return self.wait(conn, brain::MISSING_WAIT);
                }
                Ok(())
            }
            Act::Plex => {
                let map = self.memory.policy.travel.clone();
                let trips = map.as_deref().map(|map| crate::travel::Trips {
                    map,
                    floor: self.memory.policy.security_floor,
                });
                let risk = self.config.society.as_ref().map(|s| {
                    let mind = s.mind.lock().unwrap_or_else(|e| e.into_inner());
                    mind.traits.get(crate::mind::Trait::Risk)
                });
                let went = crate::plexer::sortie(conn, &self.config.name, risk, None, trips);
                self.track.docked_dirty = true;
                self.track.cargo_dirty = true;
                self.note = Some(went?.summary());
                Ok(())
            }
            Act::Pi => {
                let content = self.config.content.clone();
                let tend = crate::pi::run(
                    conn,
                    content.as_deref(),
                    &self.memory.name,
                    self.memory.seed,
                );
                self.track.docked_dirty = true;
                self.track.cargo_dirty = true;
                let tend = tend?;
                self.isk_sold += tend.after - tend.before;
                self.note = Some(tend.summary());
                Ok(())
            }
            Act::Hack => {
                let map = self.memory.policy.travel.clone();
                let ctx = self.roam_ctx(map.as_deref());
                // T3-SALVAGE: a raider cleared the guards of a J-space site
                // behind this system's hole: the hacker goes in after them.
                let trip = if crate::wh_raid::hack_dive_due(conn) {
                    crate::wh_raid::hack_dive(conn, &self.memory.name, self.memory.seed)
                } else {
                    crate::hacker::run_with(conn, &self.memory.name, self.memory.seed, &ctx)
                };
                self.track.docked_dirty = true;
                self.track.cargo_dirty = true;
                let trip = trip?;
                self.isk_sold += trip.after - trip.before;
                self.note = Some(trip.summary());
                Ok(())
            }
            Act::Sell => {
                let sold = crate::builder::sell_goods(conn);
                self.track.docked_dirty = true;
                let sold = sold?;
                let earned = sold.after - sold.before;
                self.isk_sold += earned;
                let units: i64 = sold.lines.iter().map(|l| l.quantity).sum();
                let refined = match (&sold.not_refined, sold.refined.len()) {
                    (Some(why), _) => format!("not refined ({why})"),
                    (None, 0) => "no ore".to_owned(),
                    (None, n) => format!("refined into {n} types"),
                };
                self.note = Some(format!(
                    "{refined}; offered {} stacks, {units} units; wallet {:.2} -> {:.2} ISK ({earned:+.2})",
                    sold.lines.len(),
                    sold.before,
                    sold.after
                ));
                Ok(())
            }
            Act::SellLoot => {
                let (current, seed) = (crate::drone_use::weapon(&self.memory), self.memory.seed);
                let sale = crate::escort::sell_loot_keeping(conn, &|hangar| {
                    crate::escort::plan_weapons_of(
                        current,
                        crate::progression::hull_to_fly(None, hangar),
                        seed,
                    )
                });
                self.track.docked_dirty = true;
                let sale = sale?;
                self.isk_sold += sale.after - sale.before;
                // X-CONTRACT: the explorer's decision point. What no bid
                // took is offered by contract; a refusal is a note.
                let offered = self.offer_surplus(conn, crate::contracts::Career::Explorer);
                self.track.docked_dirty = true;
                self.note = Some(format!(
                    "{}; {}",
                    sale.summary(),
                    offered.unwrap_or_else(|e| format!("contracts failed: {e}"))
                ));
                Ok(())
            }
            Act::Market => {
                let wants = Arc::clone(&self.memory.policy.wants);
                let content = self.config.content.clone();
                let map = self.memory.policy.travel.clone();
                let trips = map.as_deref().map(|map| crate::travel::Trips {
                    map,
                    floor: self.memory.policy.security_floor,
                });
                let cycle = if std::ptr::eq(self.memory.policy.role, &crate::reactor::ROLE) {
                    crate::reactor::run(conn, content.as_deref(), trips)
                } else {
                    crate::builder::run_with(conn, &wants, content.as_deref(), trips)
                };
                self.track.docked_dirty = true;
                let cycle = cycle?;
                // X-CONTRACT: the builder's decision point (the reactor
                // has no blueprints to spare).
                let offered = if std::ptr::eq(self.memory.policy.role, &crate::reactor::ROLE) {
                    String::new()
                } else {
                    self.offer_surplus(conn, crate::contracts::Career::Builder)
                        .unwrap_or_else(|e| format!("contracts failed: {e}"))
                };
                self.track.docked_dirty = true;
                self.note = Some(format!("{}; {offered}", cycle.summary()));
                Ok(())
            }
            Act::Hunt(ref op) => self.hunt_act(conn, op),
            Act::Kit(ref op) => self.kit_act(conn, op),
            Act::Gang(ref op) => self.gang_act(conn, op),
            Act::Fleet(ref order) => {
                let result = crate::fleet::execute(conn, &self.memory.fleet, order);
                self.memory.fleet.on_done(order, &result);
                result.map(|_| ())
            }
            Act::Capital => {
                let content = self.config.content.clone();
                let map = self.memory.policy.travel.clone();
                let ctx = crate::capital::Ctx {
                    map: map.as_deref(),
                    floor: self.memory.policy.security_floor,
                    content: content.as_deref(),
                    alt: crate::capital::is_alt(self.memory.policy.role),
                };
                let turned = crate::capital::turn(conn, &mut self.memory, &ctx);
                self.track.docked_dirty = true;
                self.note = Some(turned?);
                Ok(())
            }
            Act::Foreman => {
                let turned = crate::foreman::turn(conn, &mut self.memory);
                self.track.docked_dirty = true;
                self.note = Some(turned?);
                Ok(())
            }
            Act::Rare => {
                let content = self.config.content.clone();
                let map = self.memory.policy.travel.clone();
                let note = match (content.as_deref(), map.as_deref()) {
                    (Some(content), Some(map)) => {
                        let hazards = crate::supply_run::Hazards::none();
                        let env = crate::rare_ore::Env {
                            content,
                            map,
                            hazards: &hazards,
                            hull: self.obs.ship.and_then(|s| s.type_id),
                        };
                        let name = self.memory.name.clone();
                        let turned =
                            crate::rare_ore::turn(conn, &env, &name, &mut self.memory.rare);
                        self.track.docked_dirty = true;
                        self.track.cargo_dirty = true;
                        turned?
                    }
                    _ => "rare ore: no content or map".to_owned(),
                };
                self.note = Some(note);
                Ok(())
            }
            Act::Econ => {
                let table = self.memory.policy.econ.clone();
                let content = self.config.content.clone();
                let map = self.memory.policy.travel.clone();
                let trader = self.memory.policy.role.name() == crate::trader::ROLE.name();
                let mut cx = crate::nes_use::Ctx {
                    table: &table,
                    content: content.as_deref(),
                    map: map.as_deref(),
                    trader,
                    desk: &mut self.memory.desk,
                };
                let turned = crate::nes_use::turn(conn, &mut self.memory.econ, &mut cx);
                self.track.docked_dirty = true;
                self.track.cargo_dirty = true;
                self.note = Some(turned?);
                Ok(())
            }
            Act::Sov => {
                let Some(hall) = self.memory.policy.sov.clone() else {
                    return Err(Error::NotSent(crate::sov_bot::MISSING_HALL.to_owned()));
                };
                let content = self.config.content.clone();
                let map = self.memory.policy.travel.clone();
                let society = self.config.society.as_ref().and_then(|s| s.hall.clone());
                let relation = |me: i64, holder: i64| {
                    society
                        .as_ref()
                        .map_or(0.0, |h| h.lock().alliance_relation(me, holder))
                };
                let ctx = crate::sov_bot::Ctx {
                    map: map.as_deref(),
                    content: content.as_deref(),
                    hall: &hall,
                    relation: Some(&relation),
                    society: society.as_deref(),
                };
                let turned = crate::sov_bot::turn(conn, &mut self.memory, &ctx);
                self.track.docked_dirty = true;
                self.note = Some(turned?);
                Ok(())
            }
            Act::Haul => {
                let content = self.config.content.clone();
                let Some(map) = self.memory.policy.travel.clone() else {
                    return Err(Error::NotSent(crate::trader::MISSING_MAP.to_owned()));
                };
                let minutes = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() / 60);
                let trips = crate::travel::Trips {
                    map: &map,
                    floor: self.memory.policy.security_floor,
                };
                let mut shop_next = self.memory.capital.shop_next;
                self.attach_desk();
                let hauled = crate::hauler::run(
                    conn,
                    trips,
                    content.as_deref(),
                    usize::try_from(minutes).unwrap_or(0),
                    &mut shop_next,
                    &mut self.memory.desk,
                );
                self.memory.capital.shop_next = shop_next;
                self.save_desk();
                self.track.docked_dirty = true;
                self.note = Some(hauled?.summary());
                Ok(())
            }
            Act::Trade => {
                let content = self.config.content.clone();
                let Some(map) = self.memory.policy.travel.clone() else {
                    return Err(Error::NotSent(crate::trader::MISSING_MAP.to_owned()));
                };
                let minutes = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() / 60);
                let setup = crate::trader::Setup {
                    trips: crate::travel::Trips {
                        map: &map,
                        floor: self.memory.policy.security_floor,
                    },
                    content: content.as_deref(),
                    types: &[],
                    offset: usize::try_from(minutes).unwrap_or(0),
                    couriers: true,
                    costs: self.memory.policy.trader.costs(),
                    skills: crate::trader::SkillUse::Own,
                };
                // X-CONTRACT: the trader's decision point, before its own
                // probe: a contract here that is worth more than its price.
                let taken = self.take_underpriced(conn);
                self.track.docked_dirty = true;
                let taken = match taken {
                    Ok(t) => t.note,
                    Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                    Err(e) => format!("contracts failed: {e}"),
                };
                self.attach_desk();
                let traded = crate::trader::run_with(conn, &setup, &mut self.memory.desk);
                self.save_desk();
                self.track.docked_dirty = true;
                self.note = Some(format!("{}; {taken}", traded?.summary()));
                Ok(())
            }
            Act::Stance { hurt, range, reach } => self.stance(conn, hurt, range, reach),
            Act::AlignTo(id) => conn.align_to(id).map(|_| ()),
            Act::Stop => conn.stop().map(|_| ()),
            Act::TravelTo { system } => {
                if conn.session().solar_system_id2() == Some(system) {
                    return Ok(());
                }
                if self.begin_travel(system, "boss's order") {
                    Ok(())
                } else {
                    Err(Error::NotSent(format!(
                        "TravelTo: no known station in system {system}"
                    )))
                }
            }
            Act::JumpGate { gate } => self.step_to_gate(conn, gate),
            Act::Wing(ref op) => self.wing_act(conn, op),
            Act::Resync => conn.request_state().map(|_| ()),
            Act::Orbit { id, range } => {
                if self.move_repeats(conn, true, id, range) {
                    return Ok(());
                }
                conn.orbit(id, range)?;
                self.track.last_move = conn.session().ship_id().map(|s| (s, true, id, range));
                full_throttle(conn)?;
                self.await_tether(conn, id)
            }
            Act::Fire { gun, target } => {
                let Some(g) = self.obs.guns.iter().find(|g| g.item_id == gun).copied() else {
                    return Err(Error::NotSent(format!("fire: {gun} is not a fitted gun")));
                };
                match conn.fire(gun, g.type_id, self.config.content.as_deref(), target) {
                    Ok(e) => {
                        self.track.cap.ran();
                        if let Some(g) = self.obs.guns.iter_mut().find(|g| g.item_id == gun) {
                            g.active = true;
                            g.target = e.target.or(Some(target));
                        }
                        // The first shot of a fight: a propulsion module or a
                        // hardener the data lets overload runs hot (K-OVERHEAT).
                        if !g.active
                            && let Err(e @ (Error::Io(_) | Error::Protocol(_))) =
                                crate::overload::engage(conn, self.config.content.as_deref())
                        {
                            return Err(e);
                        }
                        // PROG-PVE2: so do the tank's hardeners and repairer.
                        if !g.active
                            && self.memory.policy.closed
                            && let Some(content) = self.config.content.as_deref()
                            && let Err(e @ (Error::Io(_) | Error::Protocol(_))) =
                                crate::tank::engage(
                                    conn,
                                    content,
                                    crate::drone_use::weapon(&self.memory),
                                )
                        {
                            return Err(e);
                        }
                        Ok(())
                    }
                    Err(Error::Refused(why)) => {
                        // BOT-CAP: a hold, not a failure.
                        if let Some(r) = crate::cap::Refusal::of(&why) {
                            self.track.cap.refused(r, Instant::now());
                            self.note = Some(format!("held: {r:?}"));
                            return Ok(());
                        }
                        // Empty, by the server's word: believe it.
                        if why.contains("NoCharges") {
                            self.set_rounds(g.flag_id, escort::AMMO, 0);
                        }
                        // Not locked: lock again rather than fire again.
                        if why.contains("DeniedActivateTargetNotPresent") {
                            self.obs.locked.retain(|&t| t != target);
                        }
                        // Out of the gun's reach, by the server's word (mine-range):
                        // not a failure but a distance. Close it -- orbit at the
                        // fight range -- and fire again next turn; a refusal counted
                        // as a failure would send the pilot home for a gap.
                        if why.contains("TargetTooFar") {
                            let range = escort::fight_range(&self.obs.guns);
                            conn.orbit(target, range)?;
                            full_throttle(conn)?;
                            return Ok(());
                        }
                        Err(Error::Refused(why))
                    }
                    Err(e) => Err(e),
                }
            }
            Act::CeaseFire(gun) => {
                let Some(g) = self.obs.guns.iter().find(|g| g.item_id == gun).copied() else {
                    return Ok(());
                };
                let content = self.config.content.as_deref();
                let Some((name, effect_id)) = crate::weapon_effect(content, g.type_id) else {
                    return Ok(());
                };
                let said = conn.deactivate(gun, name);
                // The fight is over: the modules run hot for it stop
                // (K-OVERHEAT).
                if let Err(e @ (Error::Io(_) | Error::Protocol(_))) =
                    crate::overload::disengage(conn, content)
                {
                    return Err(e);
                }
                if self.memory.policy.closed
                    && let Some(content) = content
                    && let Err(e @ (Error::Io(_) | Error::Protocol(_))) = crate::tank::disengage(
                        conn,
                        content,
                        crate::drone_use::weapon(&self.memory),
                    )
                {
                    return Err(e);
                }
                // The module's own word on whether it still runs: a stop
                // waits for the cycle's end, and `activeEffects` drops the
                // effect as soon as the stop is pending (`modules.rs`).
                let running = conn.active_effects(gun)?.contains(&effect_id);
                if let Some(g) = self.obs.guns.iter_mut().find(|g| g.item_id == gun) {
                    g.active = running;
                    if !running {
                        g.target = None;
                    }
                }
                said.map(|_| ())
            }
            Act::LoadAmmo {
                ref guns,
                ref charges,
                location,
            } => {
                // The type being loaded, off the first charge's row.
                let round = charges
                    .first()
                    .and_then(|id| {
                        self.obs
                            .cargo
                            .iter()
                            .chain(&self.obs.hangar)
                            .find(|r| r.item_id == *id)
                    })
                    .map_or(escort::AMMO, |r| r.type_id);
                conn.load_ammo(guns, charges, round, location)?;
                for &(_, flag) in guns {
                    let ship = conn.session().ship_id().unwrap_or(0);
                    if let Ok(n) = conn.loaded_rounds(ship, flag, round) {
                        self.set_rounds(flag, escort::AMMO, n);
                    }
                }
                self.track.docked_dirty = true;
                self.track.cargo_dirty = true;
                Ok(())
            }
            Act::Stock { item, quantity } => {
                conn.load_cargo(item, Some(quantity))?;
                self.track.docked_dirty = true;
                Ok(())
            }
            Act::Loot(wreck) => {
                let looted = conn.loot_all(wreck)?;
                self.track.cargo_dirty = true;
                if looted
                    .refusal
                    .as_deref()
                    .is_some_and(|k| k.contains("NotEnoughCargoSpace"))
                {
                    self.obs.hold_full = true;
                }
                Ok(())
            }
            Act::Salvage { module, wreck } => {
                let ran = crate::salvage::run(conn, module, wreck);
                self.track.cargo_dirty = true;
                match ran {
                    Ok(_) => {
                        if let Some(s) = &mut self.obs.salvager {
                            s.active = true;
                            s.target = Some(wreck);
                        }
                        Ok(())
                    }
                    Err(Error::Refused(why)) => {
                        // The server's word on a lock lost in flight: lock
                        // again rather than activate again.
                        if why.contains("DeniedActivateTargetNotPresent") {
                            self.obs.locked.retain(|&t| t != wreck);
                        }
                        // Cargo that cannot take the worst case: the hold is
                        // full for this purpose.
                        if why.contains("SalvagingPointlessOverload") {
                            self.obs.hold_full = true;
                        }
                        Err(Error::Refused(why))
                    }
                    Err(e) => Err(e),
                }
            }
            Act::Logi(ref op) => crate::logi::execute(
                conn,
                op,
                &mut self.obs.logi,
                &mut self.obs.locked,
                std::time::Instant::now(),
            ),
            Act::Say(ref line) => {
                let Some(chat) = &mut self.chat else {
                    return Err(Error::NotSent("chat is off".to_owned()));
                };
                match chat.say(conn, line)? {
                    Ok(_) => Ok(()),
                    Err(why) => {
                        self.chat = None;
                        self.obs.chat = false;
                        Err(Error::NotSent(format!("chat dropped: {why}")))
                    }
                }
            }
            // Handled by the session loop; never reaches here.
            Act::Wait(_) | Act::Missing(_) | Act::Reconnect => Ok(()),
        }
    }

    /// The park shows the ship warping, or the warp was not taken.
    fn await_warp(&mut self, conn: &mut Conn) -> Result<()> {
        let until = Instant::now() + MOVE_EVIDENCE;
        loop {
            let ship = conn.session().ship_id();
            if ship
                .and_then(|s| conn.world_now().get(s).copied())
                .is_some_and(|b| b.warp_commanded())
            {
                return Ok(());
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Refused(
                    "CmdWarpToStuff: no WarpTo for this ship".to_owned(),
                ));
            }
            conn.pump(left.min(Duration::from_millis(250)))?;
            self.absorb_all(conn);
        }
    }

    /// The park shows the ship following `target`, or the order was not
    /// taken. Without this the next turn reads the park before the
    /// `FollowBall` lands and orders the same approach twice.
    /// MOVE-DEDUPE: this follow or orbit is the one already in force.
    fn move_repeats(&self, conn: &mut Conn, orbit: bool, id: i64, range: f64) -> bool {
        let ship = conn.session().ship_id();
        let tether = ship
            .and_then(|s| conn.world_now().get(s).copied())
            .and_then(|b| b.motion.tether());
        repeats_move(self.track.last_move, ship, tether, orbit, id, range)
    }

    fn await_tether(&mut self, conn: &mut Conn, target: i64) -> Result<()> {
        let until = Instant::now() + MOVE_EVIDENCE;
        loop {
            let ship = conn.session().ship_id();
            if ship
                .and_then(|s| conn.world_now().get(s).copied())
                .and_then(|b| b.motion.tether())
                .is_some_and(|(t, _)| t == target)
            {
                return Ok(());
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Refused(format!(
                    "CmdFollowBall({target}): no FollowBall for this ship"
                )));
            }
            conn.pump(left.min(Duration::from_millis(250)))?;
            self.absorb_all(conn);
        }
    }

    /// **GM setup: the only caller of [`crate::gm::slash`]**, and each slash
    /// is its own act-log entry (T9). The role's skill always; a hull, its
    /// module(s) and its rounds when the brain says the hangar lacks them.
    fn setup(&mut self, conn: &mut Conn, ship: bool, module: bool, ammo: bool) -> Result<()> {
        // A wallet only matters with a hull to fund; a failed ask is empty,
        // the pre-BOTMARK behaviour.
        let isk = ship && conn.cash_balance().map_or(true, |b| b <= 0.0);
        let lines = (self.memory.policy.role.setup)(roles::Lacks {
            ship,
            module,
            ammo,
            isk,
        });
        for line in lines {
            let outcome = crate::gm::slash(conn, &line)?;
            let verdict = match outcome.refusal() {
                None => Verdict::Ok,
                Some(key) => Verdict::Refused(key),
            };
            self.record(ActKind::Slash, line, vec![], verdict);
        }
        self.track.docked_dirty = true;
        Ok(())
    }

    /// **The closed economy's setup** (E8, [`crate::acquire`]): the role's
    /// skill, hull, modules and rounds the player's way, and the rookie ship
    /// for a pilot in a capsule. **No GM**: nothing here calls
    /// [`crate::gm::slash`]. Each thing not found is its own `MISSING`
    /// act-log line; the act's line says what was learned and bought.
    fn acquire(&mut self, conn: &mut Conn, ship: bool, module: bool, ammo: bool) -> Result<()> {
        if let Some(c) = &self.config.content {
            crate::acquire::use_content(c);
        }
        // PROG-PVE: a fighter's guns and rounds are the flown class's;
        // PROG-PVE2: and its race's weapon -- of the hull it is about to
        // fly, which may be the one just bought and not yet boarded.
        let flying = self.obs.ship.and_then(|s| s.type_id);
        let target = crate::progression::hull_to_fly(flying, &conn.hangar_items()?);
        let kit = crate::fit_plan::kit_for_hull(
            crate::acquire::kit_of(self.memory.policy.role),
            target,
            self.memory
                .combat
                .t3_weapon
                .unwrap_or_else(|| crate::progression::weapon_for(target, self.memory.seed)),
        );
        let in_capsule = self
            .obs
            .ship
            .is_none_or(|s| s.type_id.is_none_or(|t| t == brain::CAPSULE));
        let map = self.memory.policy.travel.clone();
        let trips = map.as_deref().map(|map| crate::travel::Trips {
            map,
            floor: self.memory.policy.security_floor,
        });
        // Any small round the kit's gun takes (BOT-TUNE), local builders'
        // first; without a bundle, the kit's own round.
        let (any, local) = match (self.config.content.as_deref(), kit.modules) {
            (Some(c), Some((gun, _))) => (
                crate::ammo::rounds_for(c, gun),
                crate::ammo::local_products(c),
            ),
            _ => (Vec::new(), Vec::new()),
        };
        let lacking = crate::acquire::Lacking {
            ship,
            module,
            ammo,
            in_capsule,
        };
        // DOC-5: with a doctrine the order is the role fit's (hull, every
        // module at its count, its rounds, its skills) plus the tools the
        // role's own kit carries; without, the race and weapon defaults.
        let got = match self.memory.combat.doctrine.as_ref() {
            Some(fit) => {
                let order = crate::fit_plan::fight_order(
                    crate::acquire::kit_of(self.memory.policy.role),
                    target,
                    self.memory.combat.t3_weapon.unwrap_or_else(|| {
                        crate::progression::weapon_for(target, self.memory.seed)
                    }),
                    Some(fit),
                );
                let done = crate::acquire::run_order(
                    conn,
                    &order,
                    lacking,
                    trips,
                    &crate::acquire::Rounds::default(),
                );
                // FE-DEMAND: what the asks did not cover becomes bids with
                // size (hull, modules, days of rounds, a spare when rich).
                if done.is_ok()
                    && !in_capsule
                    && let Err(e @ (Error::Io(_) | Error::Protocol(_))) =
                        self.replace_by_bid(conn, &order)
                {
                    return Err(e);
                }
                done
            }
            None => crate::acquire::run_with_rounds(
                conn,
                &kit,
                lacking,
                trips,
                &crate::acquire::Rounds {
                    any: &any,
                    local: &local,
                },
            ),
        };
        self.track.docked_dirty = true;
        let got = got?;
        for what in &got.missing {
            self.record(ActKind::Missing, what.clone(), vec![], Verdict::Ok);
        }
        self.note = Some(got.summary());
        Ok(())
    }

    /// **Bid for what a doctrine order still lacks** (FE-DEMAND,
    /// [`crate::hull_choice::replenish`]), docked.
    fn replace_by_bid(&mut self, conn: &mut Conn, order: &crate::acquire::Order) -> Result<()> {
        let wallet = conn.cash_balance()?;
        let mut prices = match crate::pricing::Prices::read(conn) {
            Ok(p) => p,
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => return Ok(()),
        };
        let placed = crate::hull_choice::replenish(conn, &mut prices, order, wallet)?;
        if !placed.is_empty() {
            self.record(
                ActKind::Observe,
                format!("demand: bids for {placed:?}"),
                vec![],
                Verdict::Ok,
            );
        }
        Ok(())
    }

    /// **The hunter's own acts** (SOC-C5, [`crate::hunt::HuntAct`]): a
    /// directional scan into the observation, a roam to another system
    /// (its new base is its home), the killmails into the society.
    fn hunt_act(&mut self, conn: &mut Conn, op: &crate::hunt::HuntAct) -> Result<()> {
        use crate::hunt::HuntAct;
        match op {
            HuntAct::DScan { belts, ego, system } => {
                let Some(doc) = self.memory.policy.pvp.clone() else {
                    return Err(Error::NotSent("hunt: no doctrine".to_owned()));
                };
                let known: BTreeSet<i64> = conn.world().balls().map(|b| b.ball_id).collect();
                let done = crate::hunt::sweep(conn, &doc, belts, *ego, *system, &known);
                self.memory.hunt.on_scan(done.is_ok());
                let sweep = done?;
                self.note = Some(format!(
                    "{} pilot ships in the system, {} belts see some",
                    sweep.ships,
                    sweep.toward.len()
                ));
                self.obs.hunt.sweep = Some(sweep);
                Ok(())
            }
            HuntAct::Roam { station } => {
                let Some(map) = self.memory.policy.travel.clone() else {
                    return Err(Error::NotSent("hunt: no map to roam over".to_owned()));
                };
                let went = crate::hunt::roam(conn, &map, *station);
                self.track.docked_dirty = true;
                self.track.cargo_dirty = true;
                self.memory
                    .hunt
                    .on_roam(went.is_ok(), went.as_ref().ok().copied().flatten());
                let system = went?;
                // The base moves with the hunter: a reconnect there is not
                // a stranding.
                self.home_station = Some(*station);
                self.note = Some(format!("now in system {system:?}"));
                Ok(())
            }
            HuntAct::Filament { item } => {
                let home = self.work_station();
                let kills = self.memory.hunt.kills;
                self.track.cargo_dirty = true;
                self.track.docked_dirty = true;
                match conn.activate_filament(*item) {
                    Ok(system) => {
                        self.memory.hunt.fil_failed = 0;
                        self.memory.hunt.away = home.map(|home| crate::filament::Away {
                            home,
                            since: Instant::now(),
                            kills,
                        });
                        let h = &mut self.memory.hunt;
                        h.visited.clear();
                        h.empty_sweeps = 0;
                        h.scan_failed = 0;
                        h.quiet_sorties = 0;
                        self.note = Some(format!("now in system {system}"));
                        Ok(())
                    }
                    Err(e) => {
                        self.memory.hunt.fil_failed += 1;
                        Err(e)
                    }
                }
            }
            HuntAct::Ess(op) => self.ess_act(conn, op),
            HuntAct::Safety => {
                let system = conn.session().solar_system_id().unwrap_or(0);
                let answer = conn.moniker_call(
                    "crimewatch",
                    PyValue::Int(system),
                    "SetSafetyLevel",
                    vec![PyValue::Int(0)],
                    vec![],
                )?;
                if let Some(key) = answer.outcome.refusal() {
                    return Err(Error::Refused(format!("SetSafetyLevel: {key}")));
                }
                Ok(())
            }
            HuntAct::Cloak(kit) => {
                conn.activate(
                    kit.item_id,
                    kit.effect,
                    None,
                    crate::verbs::REPEAT_UNTIL_STOPPED,
                )?;
                if let Some(k) = &mut self.obs.hunt.cloak {
                    k.active = true;
                }
                Ok(())
            }
            HuntAct::Uncloak(kit) => {
                let said = conn.deactivate(kit.item_id, kit.effect.0);
                self.memory.hunt.cloak_off_at = Some(Instant::now());
                if let Some(k) = &mut self.obs.hunt.cloak {
                    k.active = false;
                }
                said.map(|_| ())
            }
            HuntAct::Office => {
                let map = self.memory.policy.travel.clone();
                let trips = map.as_deref().map(|map| crate::travel::Trips {
                    map,
                    // An outlaw's trip is the police net by design: no floor.
                    floor: crate::travel::ANY_SECURITY,
                });
                let note = crate::tag_repair::run(conn, self.memory.policy.closed, trips)?;
                // BOT-LAW: the status after the visit steers the roams and
                // the gate travel clear of the police net.
                let status = crate::tag_repair::status(conn)?;
                self.memory.hunt.status = status;
                conn.set_police_status(status);
                self.track.docked_dirty = true;
                self.track.cargo_dirty = true;
                self.note = Some(note);
                Ok(())
            }
            HuntAct::Settle { allies } => {
                let hall = self.config.society.as_ref().and_then(|s| s.hall.clone());
                let first = !self.memory.hunt.settled_once;
                let got = crate::hunt::settle_sensed(
                    conn,
                    hall.as_deref(),
                    allies,
                    &mut self.memory.hunt.counted,
                    first,
                    Some(&mut self.sensor),
                )?;
                if let Some(board) = self.config.society.as_ref().and_then(|s| s.board.as_ref()) {
                    board.post_kills(self.sensor.take_fresh());
                }
                self.memory.hunt.settled_once = true;
                self.memory.hunt.kills += got.kills;
                self.memory.kills += got.kills;
                self.note = Some(got.summary());
                Ok(())
            }
        }
    }

    /// **The hunter's ESS steps** ([`crate::ess_raid`]).
    fn ess_act(&mut self, conn: &mut Conn, op: &crate::ess_raid::EssAct) -> Result<()> {
        use crate::ess_raid::EssAct;
        let now = Instant::now();
        match op {
            EssAct::Read { system } => {
                let view = conn.ess_read(*system)?;
                self.note = Some(match &view {
                    Some(v) => format!("ESS bank {:.0} ISK, linked by {:?}", v.value, v.linked_by),
                    None => "no ESS here".to_owned(),
                });
                self.memory.hunt.ess.on_read(*system, view);
                Ok(())
            }
            EssAct::Link => {
                let done = conn.ess_call("AttemptLinkToMainBank");
                self.memory.hunt.ess.on_link(done.is_ok(), now);
                done
            }
            EssAct::Unlink => {
                let done = conn.ess_call("RequestMainBankUnlink");
                self.memory.hunt.ess.on_unlink(now);
                done
            }
        }
    }

    /// **The wing's poll** (FC-1): the one bot per FC that holds the hall's
    /// lease asks for the fleet composition and publishes it; every wing bot
    /// then reads the FC's row into [`Observation::wing`]. Never fails the
    /// turn: a miss is a note, and the FC is simply not known this turn.
    fn wing_turn(&mut self, conn: &mut Conn) {
        let Some(seat) = self.memory.policy.wing.clone() else {
            return;
        };
        self.obs.wing.fc = None;
        // FC-2: the tags set before this bot joined, read once per fleet
        // (bound `beyonce`, so in space); later changes are pushed.
        if let Some(fleet) = conn.session().fleet_id()
            && self.obs.wing.tags_of != Some(fleet)
            && matches!(self.obs.place, Place::InSpace { bound: true, .. })
        {
            self.obs.wing.tags_of = Some(fleet);
            match conn.fleet_target_tags() {
                Ok(tags) => self.obs.wing.tags = tags,
                Err(e) => self.note = Some(format!("fleet tags: {e}")),
            }
        }
        let Some(fc) = seat.fc.or_else(|| self.obs.fleet.and_then(|f| f.boss)) else {
            return;
        };
        let (Some(me), Some(_)) = (conn.session().char_id(), conn.session().fleet_id()) else {
            return;
        };
        if seat.hall.take_lease(fc, me, Instant::now()) {
            match conn.fleet_composition() {
                Ok(rows) => seat.hall.publish(fc, rows, Instant::now()),
                Err(e) => {
                    seat.hall.release(fc, Instant::now());
                    self.note = Some(format!("fleet composition: {e}"));
                }
            }
        }
        self.obs.wing.fc = seat.hall.row(fc, fc, Instant::now()).and_then(|row| {
            Some(crate::wing::Fc {
                char_id: fc,
                system: row.system?,
                station: row.station,
            })
        });
        // FC-3: say where this bot is, and read who leads and who stands where.
        let now = Instant::now();
        let pos = match self.obs.place {
            Place::Docked { .. } => crate::wing_hall::Pos::Docked,
            Place::InSpace { .. } if self.obs.park.as_ref().is_some_and(|p| p.boss.is_some()) => {
                crate::wing_hall::Pos::Grid
            }
            _ => crate::wing_hall::Pos::Away,
        };
        seat.hall.post(fc, me, pos, now);
        self.obs.wing.lead = seat.hall.lead(fc, now) == Some(me);
        self.obs.wing.tally = seat.hall.tally(fc, now);
    }

    /// **The wing's act** ([`crate::wing::WingAct`]): apply to the FC's
    /// fleet-finder advert, or fly to his station and dock.
    fn wing_act(&mut self, conn: &mut Conn, op: &crate::wing::WingAct) -> Result<()> {
        use crate::wing::WingAct;
        match op {
            WingAct::Join => {
                let Some(fc) = self.memory.policy.wing.as_ref().and_then(|w| w.fc) else {
                    return Err(Error::NotSent("wing: no FC to look for".to_owned()));
                };
                let ads = conn.available_fleet_ads()?;
                let Some((fleet, _)) = ads.into_iter().find(|(_, leader)| *leader == fc) else {
                    self.note = Some(format!("no fleet advert from {fc} yet"));
                    return Ok(());
                };
                conn.apply_to_join_fleet(fleet)?;
                self.memory.wing.applied = Some(fleet);
                self.note = Some(format!("applied to fleet {fleet}"));
                Ok(())
            }
            WingAct::GoDock { station } => {
                let Some(map) = self.memory.policy.travel.clone() else {
                    return Err(Error::NotSent("wing: no map to dock by".to_owned()));
                };
                let trips = crate::travel::Trips {
                    map: &map,
                    floor: crate::wing::WING_FLOOR,
                };
                let went = crate::travel::go(conn, trips, *station);
                self.track.docked_dirty = true;
                self.track.cargo_dirty = true;
                went.map(|_| ())
            }
        }
    }

    /// **The gang's act** (ROAM-GANG, [`crate::roam_gang::GangAct`]): fly
    /// over the gates and stay in space on the arrival grid.
    /// **Launch the camp's unit** (NW-11b): `ship.Drop([(item, 1)], charID,
    /// False)`, the client's Launch for Self (`menusvc.LaunchForSelf`) **[V]**
    /// (`anchor::drop_call`). The answer's new id is remembered with the spot
    /// the camper stood at. A refusal rests the unit's work.
    fn unit_launch(&mut self, conn: &mut Conn, item: i64) -> Result<()> {
        let now = Instant::now();
        let Some(params) = conn.session().location_moniker() else {
            return Ok(());
        };
        let me = conn.session().char_id().map_or(PyValue::None, PyValue::Int);
        let answer = conn.moniker_call(
            "ship",
            params,
            "Drop",
            vec![
                PyValue::List(vec![PyValue::Tuple(vec![
                    PyValue::Int(item),
                    PyValue::Int(1),
                ])]),
                me,
                PyValue::Bool(false),
            ],
            vec![],
        )?;
        let ids = match (answer.refusal(), answer.value.as_ref()) {
            (None, Some(v)) => crate::ore::launched(v, item),
            _ => Vec::new(),
        };
        match (ids.first(), self.obs.park.as_ref()) {
            (Some(id), Some(park)) => {
                self.memory.camp.unit = Some(crate::camp::Unit {
                    id: *id,
                    stage: crate::camp::Stage::Launched,
                    at: now,
                    spot: park.ego.position,
                });
                self.obs.cargo.retain(|r| r.item_id != item);
            }
            _ => {
                self.memory.camp.unit_blocked_until = Some(now + crate::camp::UNIT_BACKOFF);
                self.note = Some("unit launch refused".to_owned());
            }
        }
        Ok(())
    }

    /// **Anchor or lift the camp's unit** (NW-11b): `dogmaIM.Activate(id,
    /// effectID)` with the int effect, two args (`menusvc:2447`) **[V]**.
    fn unit_work(&mut self, conn: &mut Conn, id: i64, stage: crate::camp::Stage) -> Result<()> {
        let now = Instant::now();
        let effect = match stage {
            crate::camp::Stage::Lifting => eve_server::anchor::EFFECT_ANCHOR_LIFT,
            _ => eve_server::anchor::EFFECT_ANCHOR_DROP,
        };
        let Some(params) = conn.session().location_moniker() else {
            return Ok(());
        };
        let answer = conn.moniker_call(
            "dogmaIM",
            params,
            "Activate",
            vec![PyValue::Int(id), PyValue::Int(effect)],
            vec![],
        )?;
        match answer.refusal() {
            None => {
                if let Some(u) = &mut self.memory.camp.unit {
                    u.stage = stage;
                    u.at = now;
                }
            }
            Some(key) => {
                self.memory.camp.unit_blocked_until = Some(now + crate::camp::UNIT_BACKOFF);
                self.note = Some(format!("unit {stage:?} refused: {key}"));
            }
        }
        Ok(())
    }

    /// **Scoop the camp's unit** (NW-11b): `ship.Scoop(id)` (`menusvc.
    /// RealScoop`) **[V]**. Done, or gone, forgets it.
    fn unit_scoop(&mut self, conn: &mut Conn, id: i64) -> Result<()> {
        let now = Instant::now();
        let Some(params) = conn.session().location_moniker() else {
            return Ok(());
        };
        let answer = conn.moniker_call("ship", params, "Scoop", vec![PyValue::Int(id)], vec![])?;
        match answer.refusal() {
            None => self.memory.camp.unit = None,
            Some(key) => {
                self.memory.camp.unit_blocked_until = Some(now + crate::camp::UNIT_BACKOFF);
                self.note = Some(format!("unit scoop refused: {key}"));
            }
        }
        Ok(())
    }

    /// **The kit's acts** (BP-KIT, [`crate::pvp_kit::KitAct`]): one smartbomb
    /// cycle, or the drive. A refusal of the drive (`ModuleIsBlocked`, the
    /// spool) puts it off ([`crate::pvp_kit::Mem::blocked`]) and is not a
    /// failure.
    fn kit_act(&mut self, conn: &mut Conn, op: &crate::pvp_kit::KitAct) -> Result<()> {
        use crate::pvp_kit::KitAct;
        match op {
            KitAct::Bomb(m) => {
                // One cycle: a friend who arrives is safe by the next.
                conn.activate(m.item_id, m.effect, None, 1)?;
                for b in &mut self.obs.kit.bombs {
                    if b.item_id == m.item_id {
                        b.active = true;
                    }
                }
                Ok(())
            }
            KitAct::Launch(l) => match conn.activate(l.item_id, l.effect, None, 1) {
                Ok(_) => {
                    let now = Instant::now();
                    self.memory.camp.launched_at = Some(now);
                    if let Some(k) = &mut self.obs.kit.launcher {
                        k.active = true;
                        k.loaded = k.loaded.map(|n| (n - 1).max(0));
                    }
                    Ok(())
                }
                // The server's word that bubbles are not allowed here, or
                // the pilot lacks the skill: a fact, not a failure.
                Err(Error::Refused(w)) => {
                    self.memory.camp.blocked(Instant::now());
                    self.note = Some(format!("probe launch refused: {w}"));
                    Ok(())
                }
                Err(e) => Err(e),
            },
            KitAct::UnitLaunch(item) => self.unit_launch(conn, *item),
            KitAct::UnitAnchor(id) => self.unit_work(conn, *id, crate::camp::Stage::Anchoring),
            KitAct::UnitLift(id) => self.unit_work(conn, *id, crate::camp::Stage::Lifting),
            KitAct::UnitScoop(id) => self.unit_scoop(conn, *id),
            KitAct::FieldOn(m) => {
                conn.activate(
                    m.item_id,
                    m.effect,
                    None,
                    crate::verbs::REPEAT_UNTIL_STOPPED,
                )?;
                if let Some(f) = &mut self.obs.kit.field {
                    f.active = true;
                }
                Ok(())
            }
            KitAct::FieldOff(m) => {
                let said = conn.deactivate(m.item_id, m.effect.0);
                if let Some(f) = &mut self.obs.kit.field {
                    f.active = false;
                }
                said.map(|_| ())
            }
            KitAct::Jump(m) => match conn.activate(m.item_id, m.effect, None, 1) {
                Ok(_) => {
                    if let Some(d) = &mut self.obs.kit.mjd {
                        d.active = true;
                    }
                    Ok(())
                }
                Err(Error::Refused(w)) if crate::pvp_kit::is_drive_refusal(&w) => {
                    self.memory.kit.blocked(Instant::now());
                    self.memory.kit.spool_until = None;
                    Err(Error::Refused(w))
                }
                Err(e) => Err(e),
            },
        }
    }

    fn gang_act(&mut self, conn: &mut Conn, op: &crate::roam_gang::GangAct) -> Result<()> {
        use crate::roam_gang::GangAct;
        match op {
            GangAct::Fly { system } => {
                let Some(map) = self.memory.policy.travel.clone() else {
                    return Err(Error::NotSent("gang: no map to fly over".to_owned()));
                };
                let went = crate::roam_gang::fly(conn, &map, *system);
                self.track.docked_dirty = true;
                self.memory.gang.fly_failed = if went.is_ok() {
                    0
                } else {
                    self.memory.gang.fly_failed + 1
                };
                let at = went?;
                self.note = Some(format!("now in system {at}"));
                Ok(())
            }
        }
    }

    /// **What a roaming career is told of this bot** (Y-ROAM,
    /// [`crate::explore_roam::Ctx`]): the map, the mind's nerve, the home
    /// station, the state directory and whether its corporation has a
    /// builder.
    fn roam_ctx<'a>(&'a self, map: Option<&'a crate::travel::Map>) -> crate::explore_roam::Ctx<'a> {
        let soc = self.config.society.as_ref();
        let nerve = soc.map(|s| {
            let mind = s.mind.lock().unwrap_or_else(|e| e.into_inner());
            crate::explore_roam::Nerve {
                risk: mind.traits.get(crate::mind::Trait::Risk),
                aggression: mind.traits.get(crate::mind::Trait::Aggression),
            }
        });
        let builder_in_corp = soc
            .and_then(|s| s.hall.as_ref())
            .is_some_and(|h| crate::explore_roam::builder_in_corp(&h.lock(), &self.config.name));
        crate::explore_roam::Ctx {
            trips: map.map(|map| crate::travel::Trips {
                map,
                floor: self.memory.policy.security_floor,
            }),
            nerve,
            home: self.home_station,
            state: soc.and_then(|s| s.state.as_deref()),
            builder_in_corp,
        }
    }

    /// **A tactical destroyer's stance for the fight** (PROG-PVE2,
    /// [`crate::t3::stance_wanted`], [`crate::t3::set_stance`]): the hull's
    /// pick by the server's rule, sent when it differs from the one set. A
    /// hull with no stances, no bundle, and the switch lock
    /// (`ShipAlreadyChangingStance`) are quiet notes, never failures.
    fn stance(&mut self, conn: &mut Conn, hurt: f64, range: Option<f64>, reach: f64) -> Result<()> {
        let (Some(content), Some(ship)) = (self.config.content.clone(), self.obs.ship) else {
            return Ok(());
        };
        let Some(hull) = ship.type_id else {
            return Ok(());
        };
        let Some(want) = crate::t3::stance_wanted(&content, hull, hurt, range, reach) else {
            return Ok(());
        };
        if self.memory.combat.stance == Some(want) {
            return Ok(());
        }
        match crate::t3::set_stance(conn, ship.ship_id, want)? {
            Ok(()) => {
                self.memory.combat.stance = Some(want);
                self.note = Some(format!("stance {want}"));
            }
            Err(key) => self.note = Some(format!("stance {want}: {key}")),
        }
        Ok(())
    }

    /// **One combat-drone step** ([`crate::drone_use::act`], K-DRONE): the
    /// bot's drones docked or in space. GM lines the arm sent are recorded
    /// as `slash` entries, as [`Self::setup`]'s are.
    fn drone(&mut self, conn: &mut Conn, op: crate::drone_use::DroneAct) -> Result<()> {
        let in_capsule = self
            .obs
            .ship
            .is_none_or(|s| s.type_id.is_none_or(|t| t == brain::CAPSULE));
        let map = self.memory.policy.travel.clone();
        let trips = map.as_deref().map(|map| crate::travel::Trips {
            map,
            floor: self.memory.policy.security_floor,
        });
        let content = self.config.content.clone();
        let ctx = crate::drone_use::Ctx {
            closed: self.memory.policy.closed,
            in_capsule,
            trips,
            content: content.as_deref(),
        };
        let done = crate::drone_use::act(conn, &mut self.memory, op, ctx);
        let done = done?;
        for (line, refusal) in done.slashes {
            let verdict = match refusal {
                None => Verdict::Ok,
                Some(key) => Verdict::Refused(key),
            };
            self.record(ActKind::Slash, line, vec![], verdict);
        }
        if done.docked_dirty {
            self.track.docked_dirty = true;
        }
        self.ledger.invest(done.spent);
        // PROG-PVE2: a bot that moved to richer ground lives there now.
        if let Some(station) = done.moved_to {
            self.home_station = Some(station);
            let moved = self
                .config
                .society
                .as_ref()
                .map(|soc| soc.move_home(station))
                .is_some();
            if moved {
                self.save_mind();
            }
        }
        self.note = done.note;
        Ok(())
    }

    /// PULL-LEASE: where the bot works now: the loan's station, else its
    /// home. Everything that sends a bot back to where it works reads this.
    fn work_station(&self) -> Option<i64> {
        self.lease.or(self.home_station)
    }

    /// PULL-LEASE: take a lend or a recall. A lend keeps `home_station` (the
    /// last station docked at, when the bot has none yet, so the base is
    /// not lost); a recall sends the bot to that base.
    fn take_lend(&mut self, cmd: Option<i64>) {
        match cmd {
            Some(station) => {
                if self.home_station.is_none() {
                    self.home_station = self.obs.home;
                }
                self.lease = Some(station);
                self.stranded = Some((station, 0));
                self.record(
                    ActKind::Travel,
                    format!("lent to station {station} by the bubble"),
                    vec![station],
                    Verdict::Ok,
                );
            }
            None => {
                self.lease = None;
                self.stranded = self.home_station.map(|h| (h, 0));
                let home = self.home_station.unwrap_or_default();
                self.record(
                    ActKind::Travel,
                    format!("recalled to station {home} by the bubble"),
                    vec![home],
                    Verdict::Ok,
                );
            }
        }
        if let Ok(mut l) = self.handle.lease_pub.lock() {
            *l = self.lease;
        }
    }

    /// **One step toward a gate and through it** (a fleet's `JumpTo`): jump
    /// inside the client's range, approach inside the minimum warp, else
    /// warp to it. The order is spent by the jump.
    fn step_to_gate(&mut self, conn: &mut Conn, gate: i64) -> Result<()> {
        let here = conn
            .session()
            .solar_system_id2()
            .ok_or_else(|| Error::NotSent("JumpTo: no system".to_owned()))?;
        let partner = self.memory.policy.travel.as_ref().and_then(|map| {
            map.neighbours(here).into_iter().find_map(|n| {
                map.gate(here, n)
                    .filter(|(g, _)| *g == gate)
                    .map(|(_, p)| p)
            })
        });
        let Some(partner) = partner else {
            self.memory.order = None;
            return Err(Error::NotSent(format!(
                "JumpTo: gate {gate} is on no known route from system {here}"
            )));
        };
        match warp_gap(conn, Some(gate)) {
            Some(g) if g < eve_server::jump::MAX_JUMP_DISTANCE => {
                conn.stargate_jump(gate, partner)?;
                // FC-1: only a heard JumpTo is spent by its jump; a wing's own
                // step through a gate leaves a standing Hold alone.
                if matches!(
                    self.memory.order,
                    Some(crate::fleet_orders::Order {
                        heard: crate::fleet_orders::Heard::Jump { .. },
                        ..
                    })
                ) {
                    self.memory.order = None;
                }
                Ok(())
            }
            Some(g) if g < crate::MIN_WARP_DISTANCE => {
                conn.follow(gate, 0.0)?;
                full_throttle(conn)?;
                self.await_tether(conn, gate)
            }
            _ => {
                conn.warp_to_item(gate, 0.0)?;
                self.await_warp(conn)
            }
        }
    }

    /// **The way home** (module header): a stranded bot flies to its home
    /// station over the gates before anything else. `true` when it tried
    /// (the turn starts again from a fresh observation). Without a map, or
    /// after [`RETURN_TRIES`], one `MISSING` line and it plays where it is.
    /// **A GM called this bot** (`/bots come`). A gang bot hands the call to
    /// its gang (its FC leads everyone); any other bot, a miner too, drops
    /// what it was doing and comes ([`Self::summon_turn`]).
    fn take_summon(&mut self, system: i64, char_id: i64) {
        let what = format!("called to {char_id} in system {system} by a GM");
        match self.memory.policy.gang.clone() {
            Some(seat) => crate::roam_gang::summon(&seat, system, char_id),
            None => {
                self.stranded = None;
                self.summoned = Some(Summoned {
                    system,
                    char_id,
                    since: Instant::now(),
                    warped: None,
                    failed: 0,
                });
            }
        }
        self.record(ActKind::Travel, what, vec![char_id], Verdict::Ok);
    }

    /// **Come to the summoner**: undock, fly to his system, warp to his
    /// character; then, for [`SUMMON_HOLD_S`], warp back whenever his ship is
    /// not on this grid, while the role's own turn runs in between. `true`
    /// when this turn was spent on the trip.
    fn summon_turn(&mut self, conn: &mut Conn) -> Result<bool> {
        let Some(mut s) = self.summoned.clone() else {
            return Ok(false);
        };
        let over = s.since.elapsed() >= Duration::from_secs(SUMMON_HOLD_S);
        if over || s.failed >= SUMMON_TRIES {
            self.summoned = None;
            let why = if over {
                "hold over, back to work"
            } else {
                "could not reach him, back to work"
            };
            self.record(
                ActKind::Travel,
                why.to_owned(),
                vec![s.char_id],
                Verdict::Ok,
            );
            return Ok(false);
        }
        if self.obs.place == Place::Transit {
            return Ok(false);
        }
        if conn.session().station_id().is_some() {
            self.act(conn, Act::Undock)?;
            return Ok(true);
        }
        if conn.session().solar_system_id2() != Some(s.system) {
            self.act(
                conn,
                Act::Gang(crate::roam_gang::GangAct::Fly { system: s.system }),
            )?;
            if self.memory.gang.fly_failed > 0 {
                s.failed += 1;
                self.summoned = Some(s);
            }
            return Ok(true);
        }
        // His system. Not bound yet: the role's turn binds the park.
        if conn.ballpark().is_none() || !conn.world().has_state() {
            return Ok(false);
        }
        let ship = conn.world().ship_of(s.char_id).map(|b| b.ball_id);
        let near =
            warp_gap(conn, ship).is_some_and(|g| g <= crate::roam_gang::RALLY_HOSTILE_RANGE * 2.0);
        let warping = conn
            .world_now()
            .ego_ball()
            .is_some_and(|b| b.warp_commanded());
        let due = s.warped.is_none_or(|t| t.elapsed() >= SUMMON_REPEAT);
        if near || warping || !due {
            return Ok(false);
        }
        s.warped = Some(Instant::now());
        self.act(
            conn,
            Act::WarpToMember {
                char_id: s.char_id,
                range: crate::roam_gang::RALLY_ALLIED_RANGE,
            },
        )?;
        s.failed = if self.obs.boss_unreachable_until.is_some() {
            s.failed + 1
        } else {
            0
        };
        self.summoned = Some(s);
        Ok(true)
    }

    fn return_home(&mut self, conn: &mut Conn) -> Result<bool> {
        let Some((home, tries)) = self.stranded else {
            return Ok(false);
        };
        if conn.session().station_id() == Some(home) {
            self.stranded = None;
            return Ok(false);
        }
        if self.obs.place == Place::Transit {
            return Ok(false);
        }
        let map = self.memory.policy.travel.clone();
        let Some(map) = map.filter(|_| tries < RETURN_TRIES) else {
            self.stranded = None;
            self.record(
                ActKind::Missing,
                format!("a way home to station {home} (after {tries} tries)"),
                vec![home],
                Verdict::Ok,
            );
            return Ok(false);
        };
        self.stranded = Some((home, tries + 1));
        let trips = crate::travel::Trips {
            map: &map,
            floor: self.memory.policy.security_floor,
        };
        let went = crate::travel::go(conn, trips, home);
        self.track.docked_dirty = true;
        self.track.cargo_dirty = true;
        let what = format!("return home to station {home}, try {}", tries + 1);
        let verdict = match went {
            Ok(trip) => {
                self.stranded = None;
                self.record(
                    ActKind::Travel,
                    format!("{what}: {} jumps", trip.jumps),
                    vec![home],
                    Verdict::Ok,
                );
                return Ok(true);
            }
            Err(Error::NotSent(w)) => Verdict::NotSent(w),
            Err(Error::Refused(w)) => Verdict::Refused(w),
            Err(e) => {
                self.record(
                    ActKind::Travel,
                    what,
                    vec![home],
                    Verdict::Failed(e.to_string()),
                );
                return Err(e);
            }
        };
        self.record(ActKind::Travel, what, vec![home], verdict);
        Ok(true)
    }

    /// **A bubble release** (`bubble.rs`): head home and log off docked, or
    /// where it is after [`LOGOFF_GRACE`], like a timetable's end. A society
    /// bot parks its career first, so the server settles the time away from
    /// its real hull, modules and skills and nothing else; any other bot has
    /// nothing to settle and earns nothing while away.
    fn release_turn(&mut self, conn: &mut Conn) -> Result<Option<Exit>> {
        let docked = matches!(self.obs.place, Place::Docked { .. });
        let since = *self.logoff_since.get_or_insert_with(Instant::now);
        if docked || since.elapsed() >= LOGOFF_GRACE {
            let at = if docked { "docked" } else { "in space" };
            self.record(
                ActKind::Session,
                format!("log off {at}: released by the bubble"),
                vec![],
                Verdict::Ok,
            );
            if docked && let Some(soc) = self.config.society.clone() {
                let career = soc.career();
                if let Ok(parked) = crate::offline::park(conn, &career) {
                    self.record(ActKind::Session, parked.line(&career), vec![], Verdict::Ok);
                }
            }
            return Ok(Some(Exit::LogOff));
        }
        if self.memory.homing.is_none() {
            self.memory.homing = Some(Homing::LogOff);
        }
        Ok(None)
    }

    /// **Society mode's turn** (module header): the session's end, then a
    /// career decision at a docked decision point. `Some(exit)` ends the
    /// connection.
    fn society_turn(&mut self, conn: &mut Conn) -> Result<Option<Exit>> {
        if self.handle.released() {
            return self.release_turn(conn);
        }
        let Some(soc) = self.config.society.clone() else {
            return Ok(None);
        };
        let now = unix_now();
        if !soc.timetable.online(now) {
            let docked = matches!(self.obs.place, Place::Docked { .. });
            let since = *self.logoff_since.get_or_insert_with(Instant::now);
            if docked || since.elapsed() >= LOGOFF_GRACE {
                let next = soc.timetable.until_online(now);
                let at = if docked { "docked" } else { "in space" };
                self.record(
                    ActKind::Session,
                    format!("log off {at}: the session is over; the next in {next} s"),
                    vec![],
                    Verdict::Ok,
                );
                // E1: docked, say which career the server should settle
                // while this bot is away. A lost connection is the log-off
                // anyway; anything else is a line, never a stop.
                if docked {
                    let career = soc.career();
                    if let Ok(parked) = crate::offline::park(conn, &career) {
                        self.record(ActKind::Session, parked.line(&career), vec![], Verdict::Ok);
                    }
                }
                return Ok(Some(Exit::LogOff));
            }
            if self.memory.homing.is_none() {
                self.memory.homing = Some(Homing::LogOff);
            }
            return Ok(None);
        }
        self.logoff_since = None;
        let at = Instant::now();
        if decision_point(
            self.obs.place,
            self.stranded.is_some(),
            self.last_decision,
            at,
            soc.decide_every,
        ) {
            self.last_decision = Some(at);
            self.decide_career(conn, &soc, now)?;
        }
        // SOC-C4: the social turn, docked, on this bot's own connection.
        if let Some(hall) = &soc.hall
            && matches!(self.obs.place, Place::Docked { .. })
        {
            let lines = crate::corpmind::turn(conn, hall, &soc.mind, soc.table, now)?;
            // R-REL-3: the friendly humans a fleet commander invites.
            {
                let (regard, loyalty) = {
                    let m = soc.mind.lock().unwrap_or_else(|e| e.into_inner());
                    (m.regard.clone(), m.traits.get(crate::mind::Trait::Loyalty))
                };
                let corp = conn.session().get_i64("corpid").unwrap_or(0);
                self.memory.foreman.friends =
                    hall.lock().friendly_humans(corp, &regard, loyalty, now);
            }
            for line in lines {
                if self.config.echo {
                    println!("[soc] {} corp {line}", self.config.name);
                }
                self.record_as(ActKind::Corp, line, vec![], Verdict::Ok, false);
            }
        }
        Ok(None)
    }

    /// **The per-bot gate of the capital careers** ([`crate::capital::lift_blocks`]):
    /// the price list is read only for a bot rich enough to matter.
    fn lift_capital_blocks(
        &self,
        conn: &mut Conn,
        mut signals: Signals,
        wallet: Option<f64>,
    ) -> Result<Signals> {
        let content = self.config.content.as_deref();
        let prices = match (content, wallet) {
            (Some(_), Some(w)) if w >= crate::capital::SHOP_WALLET_FLOOR => {
                match conn.adjusted_prices() {
                    Ok(p) => p,
                    Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                    Err(_) => Vec::new(),
                }
            }
            _ => Vec::new(),
        };
        let price = |groups: &[i64]| {
            content.and_then(|c| crate::capital::cheapest_hull(c, groups, &prices))
        };
        let active_group = self.obs.ship.and_then(|s| {
            self.obs
                .hangar
                .iter()
                .find(|r| r.item_id == s.ship_id)
                .map(|r| r.group_id)
        });
        crate::capital::lift_blocks(
            &mut signals,
            &crate::capital::Means {
                hangar: &self.obs.hangar,
                active_group,
                wallet,
                jumper_price: price(&crate::capital::JUMPER_GROUPS),
                freighter_price: price(&crate::capital::FREIGHTER_GROUPS),
                partner: self.memory.policy.partner.is_some(),
            },
        );
        Ok(signals)
    }

    /// One career decision: the wallet's change since the last one into
    /// the career's ledger, [`Mind::decide`], the `[soc]` line, the role
    /// the career plays, and the state file.
    fn decide_career(&mut self, conn: &mut Conn, soc: &Society, now: u64) -> Result<()> {
        let wallet = match conn.cash_balance() {
            Ok(w) => Some(w),
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => None,
        };
        // SOC-SIG: the market's signals, read here, docked.
        let mut market = crate::signal::read(
            conn,
            &crate::signal::wanted(soc.table),
            &self.memory.policy.wants,
            crate::builder::recipes_of(self.config.content.as_deref()),
            &self.obs.hangar,
            &self
                .config
                .content
                .as_deref()
                .map(crate::trader::trade_goods)
                .unwrap_or_default(),
        )?;
        // REVIEW-FIX2: a friend's want-to-buy counts for more in a builder's need.
        if !market.products.is_empty()
            && let Some((hall, mind)) = self.relations()
        {
            match crate::effects::friend_wants(conn, &hall, &mind, unix_now()) {
                Ok(w) => crate::signal::apply_buyers(&mut market, &w),
                Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                Err(_) => {}
            }
        }
        // FE-DEMAND: what an ore is worth a m3 now -- the minerals at their
        // values, lifted by the bids on them -- picks the miner's rock.
        if let Some(content) = self.config.content.as_deref()
            && !market.minerals.is_empty()
        {
            let value: BTreeMap<i64, f64> = market
                .minerals
                .iter()
                .filter_map(|q| Some((q.type_id, q.p_mkt?)))
                .collect();
            let demand = crate::demand::DemandMap::from_units(
                market
                    .minerals
                    .iter()
                    .map(|q| (q.type_id, q.bid_units, q.ask_units)),
            );
            self.track.ore_values = crate::demand::ore_values_all(content, &value, &demand);
        }
        let signals = crate::signal::signals(&soc.signals, soc.table, &market);
        // SOC-C4: the corp planner's need multiplies the market's.
        let signals = match (&soc.hall, conn.session().char_id()) {
            (Some(h), Some(me)) => h.with_need(me, soc.table, signals),
            _ => signals,
        };
        // CAP-L: nothing from nowhere -- the capital careers open for a bot
        // that owns the hull (or can buy one) and has a partner.
        let signals = self.lift_capital_blocks(conn, signals, wallet)?;
        // PROG-PVE: the tier of the ship the bot flies prices the fighting
        // careers' priors (`earnings::tier_index`).
        let rung = crate::progression::rung(
            self.obs.ship.and_then(|s| s.type_id),
            &self.obs.hangar,
            self.memory.seed,
        );
        let signals = crate::earnings::tier_index(signals, rung.class);
        // PROG-MINE: the foreman career opens for a bot that owns a command
        // hull, and the miner's prior follows the hull it flies.
        let mut signals = signals;
        let flown = self.obs.ship.and_then(|s| s.type_id);
        crate::foreman::lift_block(&mut signals, &self.obs.hangar, flown);
        let signals = crate::mine_ladder::scale_prior(signals, flown);
        let (lines, decision, career) = {
            let mut mind = soc.lock();
            // PROG-PVE: the wallet's change less what the bot laid out on
            // assets (a hull, guns, books), over this connection's time.
            if let Some((earned, hours)) = self.ledger.step(wallet, now) {
                let current = mind.career.clone();
                mind.record(&current, earned, hours);
            }
            // A new class of ship moves the fighting ledgers with it.
            if let Some(from) = self.ledger.moved(rung.class) {
                crate::earnings::retier(&mut mind.income, from, rung.class);
            }
            // PROG-MINE: a new hull moves what the ledger expects of the
            // miner at once; the ledger then learns the real rate.
            let mult = crate::mine_ladder::income_mult(flown);
            crate::mine_ladder::rescale(&mut mind, "miner", self.mining_mult, mult);
            self.mining_mult = mult;
            let eval = mind.decide(soc.table, &signals, now);
            let hull = crate::acquire::kit_of(role_of(soc.table, &mind.career)).hull;
            let holds = hull.is_some_and(|h| {
                self.obs.ship.and_then(|s| s.type_id) == Some(h)
                    || self.obs.hangar.iter().any(|r| r.type_id == h)
            });
            let price = hull.and_then(|h| market.refs.get(&h).copied());
            let goals = mind.rescore_goals(&mind::career_goals(hull, holds, wallet, price));
            (
                [
                    crate::signal::signals_line(&mind.name, &signals, &market),
                    mind::decision_line(&mind, &eval),
                    mind::goal_line(&mind, &goals),
                    // PROG-PVE2: the ladder and the realised income, per bot.
                    crate::earnings::progress_line(
                        &mind.name,
                        &mind.career,
                        &rung,
                        wallet,
                        &mind.income,
                    ),
                ],
                eval.decision,
                mind.career.clone(),
            )
        };
        let role = role_of(soc.table, &career);
        if role.name() != self.memory.policy.role.name() {
            // A new role starts from a clean memory, as a player who
            // changes trade starts its routine afresh; its kills stay.
            let mut policy = self.memory.policy.clone();
            policy.role = role;
            let kills = self.memory.kills;
            self.memory = Memory::new(&self.config.name, policy);
            self.memory.kills = kills;
            // The status was the old hunt's; the Conn must not keep it.
            conn.set_police_status(None);
        }
        if self.config.echo {
            for line in &lines {
                println!("{line}");
            }
        }
        let what = match &decision {
            Decision::Switch { from, to } => format!("switch {from} -> {to}, role {}", role.name()),
            Decision::Stay => format!("stay {career}, role {}", role.name()),
            Decision::Wait { to, wins } => format!("stay {career}: {to} won {wins}"),
            Decision::Tenure { to } => format!("stay {career}: {to} wins, tenure first"),
        };
        self.record_as(ActKind::Career, what, vec![], Verdict::Ok, false);
        self.save_mind();
        Ok(())
    }
}

/// An asteroid belt's own ball: type 15, "Asteroid Belt", for every belt in
/// the extract **[A]** (the bundle's celestials all carry it). Global, so in
/// every park (GRID-B).
pub fn is_belt(b: &crate::Tracked) -> bool {
    b.type_id() == Some(15)
}

/// **An NPC ship in the park**: a ball whose slim has no pilot, an NPC
/// corporation for an owner (`IsNPCCorporation` **[V]**), and an id in the
/// server's NPC range (`npc::MIN_NPC_ID`..`END_NPC_ID`) that is not a wreck
/// -- the last **[A]**, like `park::is_rock`: the client knows an entity by
/// its type's group, from static data this crate does not ship, and a
/// station or a wreck has the first two as well. Not the server's type
/// table: a bundle's rats are derived per world (`npc::resolve`, CP61), and
/// this process has none.
pub fn is_npc(b: &crate::Tracked) -> bool {
    (eve_server::npc::MIN_NPC_ID..eve_server::npc::END_NPC_ID).contains(&b.ball_id)
        && !is_npc_wreck(b)
        && b.slim.is_some_and(|s| {
            s.char_id.is_none() && s.owner_id.is_some_and(|o| NPC_CORPS.contains(&o))
        })
}

/// **A ship of the law** in the park: an NPC owned by CONCORD or by a
/// faction with police (`eve_server::concord::is_law_owner`). CONCORD is
/// known without content; a faction's police needs it, and with none the bot
/// cannot tell those.
pub fn is_law(content: Option<&eve_server::content::Content>, b: &crate::Tracked) -> bool {
    b.slim.is_some_and(|s| {
        s.owner_id.is_some_and(|o| {
            o == eve_server::concord::CONCORD_CORP
                || content.is_some_and(|c| eve_server::concord::is_law_owner(c, o))
        })
    })
}

/// **An NPC wreck**: a wreck type (`item::WRECK_TYPES` **[V-data]**) owned
/// by an NPC corporation -- which `HaveLootRight` lets anyone take
/// (`michelle.py:1972`) **[V]**. A player's wreck is not the escort's to
/// loot. **[A]**
pub fn is_npc_wreck(b: &crate::Tracked) -> bool {
    b.slim.is_some_and(|s| {
        (s.owner_id.is_some_and(|o| NPC_CORPS.contains(&o)) || npc_launched(&s))
            && s.type_id
                .is_some_and(|t| eve_server::item::WRECK_TYPES.iter().any(|w| w.type_id == t))
    })
}

/// GAP-LOOT: a wreck whose dead ship was an NPC (its `launcherID` in the
/// server's NPC range), whoever the wreck now belongs to -- a rat's wreck is
/// its killer's (`combat::npc_wreck_owner`).
pub fn npc_launched(s: &crate::park::SlimInfo) -> bool {
    s.launcher_id
        .is_some_and(|l| (eve_server::npc::MIN_NPC_ID..eve_server::npc::END_NPC_ID).contains(&l))
}

/// **May this pilot take from the wreck without stealing?** The client's own
/// rule, `Park.HaveLootRight` (`michelle.py:1959-1993`) **[V]**, over the
/// slim's `lootRights`: abandoned, an NPC owner, mine, my player corporation's
/// or my fleet's. A wreck with no `lootRights` falls back to its `ownerID`.
pub fn may_loot(
    b: &crate::Tracked,
    me: Option<i64>,
    my_corp: Option<i64>,
    my_fleet: Option<i64>,
) -> bool {
    let Some(s) = b.slim else { return false };
    let (owner, corp, fleet, abandoned) =
        s.loot_rights
            .unwrap_or((s.owner_id.unwrap_or(0), s.corp_id.unwrap_or(0), None, false));
    let npc_corp = |c: i64| NPC_CORPS.contains(&c);
    abandoned
        || me == Some(owner)
        || NPC_CORPS.contains(&owner)
        || (fleet.is_some() && fleet == my_fleet)
        || my_corp.is_some_and(|c| !npc_corp(c) && (c == owner || c == corp))
}

/// A sublocation key, `(shipID, flagID, typeID)`.
fn sublocation(v: &PyValue) -> Option<(i64, i64, i64)> {
    match v {
        PyValue::Tuple(t) => match t.as_slice() {
            [PyValue::Int(a), PyValue::Int(b), PyValue::Int(c)] => Some((*a, *b, *c)),
            _ => None,
        },
        _ => None,
    }
}

/// `OnDamageStateChange`'s state: `(shield, armour, hull)`, the shield a
/// `(fraction, tau, time)` tuple or a bare float (`destiny::DamageState`)
/// **[V]**.
fn health_of(v: &PyValue) -> Option<Health> {
    let PyValue::Tuple(bars) = v else {
        return None;
    };
    let shield = match bars.first()? {
        PyValue::Tuple(s) => crate::real_at(s, 0)?,
        _ => crate::real_at(bars, 0)?,
    };
    Some(Health {
        shield,
        armour: crate::real_at(bars, 1)?,
        hull: crate::real_at(bars, 2)?,
    })
}

/// The refusal keys that are also stop reasons, if `why` names one.
fn known_key(why: &str) -> Option<&'static str> {
    [
        "MiningItemDepleted",
        "MiningDronesDeactivatedCargoHoldNowFull",
        "MiningDronesDeactivatedCargoHoldFull",
        "NotEnoughCargoSpace",
        "TargetTooFar",
    ]
    .into_iter()
    .find(|k| why.contains(k))
}

/// An act as a log line and the ids it names.
fn describe(act: &Act) -> (String, Vec<i64>) {
    match act {
        Act::GmSetup { ship, module, ammo } => {
            (format!("ship={ship} module={module} ammo={ammo}"), vec![])
        }
        Act::Acquire { ship, module, ammo } => (
            format!("skills, ship={ship} module={module} ammo={ammo}"),
            vec![],
        ),
        Act::Board(s) => (format!("Board {s}"), vec![*s]),
        Act::FitAuto(i) => (format!("MultiAdd [{i}] flag=0"), vec![*i]),
        Act::FitAutoN { item, n } => (format!("MultiAdd [{item}; {n}] flag=0"), vec![*item]),
        Act::OnlineMany(ms) => (format!("SetModuleOnline {ms:?}"), ms.clone()),
        Act::Online(m) => (format!("SetModuleOnline {m}"), vec![*m]),
        Act::Undock => ("Undock".to_owned(), vec![]),
        Act::Bind => ("beyonce.MachoBindObject".to_owned(), vec![]),
        Act::WarpToItem { id, range } => (
            format!("CmdWarpToStuff item {id} minRange={range:.0}"),
            vec![*id],
        ),
        Act::WarpToMember { char_id, range } => (
            format!("CmdWarpToStuff char {char_id} minRange={range:.0}"),
            vec![*char_id],
        ),
        Act::Follow { id, range } => (format!("CmdFollowBall {id} {range:.0}"), vec![*id]),
        Act::Lock(t) => (format!("AddTarget {t}"), vec![*t]),
        Act::Unlock(t) => (format!("RemoveTarget {t}"), vec![*t]),
        Act::Mine { module, rock } => (
            format!("Activate {module} miningLaser -> {rock}"),
            vec![*module, *rock],
        ),
        Act::StopMining(m) => (format!("Deactivate {m} miningLaser"), vec![*m]),
        Act::Dock(s) => (format!("CmdDock {s}"), vec![*s]),
        Act::Unload(stacks) => (
            format!("Add {stacks:?} flag=4"),
            stacks.iter().map(|(i, _)| *i).collect(),
        ),
        Act::AcceptInvite { fleet } => (format!("AcceptInvite fleet {fleet}"), vec![*fleet]),
        Act::Courier { agent } => (format!("courier run for agent {agent}"), vec![*agent]),
        Act::Sites => ("site run: Agency, pick, warp".to_owned(), vec![]),
        Act::Ore(op) => op.describe(),
        Act::Drone(op) => op.describe(),
        Act::Huff => ("gas trip: sell, find, harvest, home".to_owned(), vec![]),
        Act::Dive => (
            "gas dive: sell, hole in, harvest, hole out, home".to_owned(),
            vec![],
        ),
        Act::Raid(op) => (op.describe().to_owned(), vec![]),
        Act::Ice => (
            "ice trip: find a field, harvest, home, refine, sell".to_owned(),
            vec![],
        ),
        Act::Mission { agent, level } => (
            format!("mission errand, agent {agent:?}, ship level {level}"),
            agent.iter().copied().collect(),
        ),
        Act::Explore => (
            "explore: outfit, or scan down, warp, save".to_owned(),
            vec![],
        ),
        Act::Plex => (
            "plex: enlist, front, overlay, warp or fly".to_owned(),
            vec![],
        ),
        Act::Pi => ("pi: tend colonies, sell, found one".to_owned(), vec![]),
        Act::Hack => (
            "hack trip: sell, find a site, hack, loot, home".to_owned(),
            vec![],
        ),
        Act::Sell => ("refine and sell the hangar's goods".to_owned(), vec![]),
        Act::SellLoot => ("sell, list or refine the hangar's loot".to_owned(), vec![]),
        Act::Market => (
            "builder cycle: deliver, list, build, buy wants".to_owned(),
            vec![],
        ),
        Act::Trade => ("trader cycle: probe, trip, home".to_owned(), vec![]),
        Act::Capital => (
            "capital pair: stock, fleet, light or jump".to_owned(),
            vec![],
        ),
        Act::Haul => ("freighter cycle: hull, bulk trade, home".to_owned(), vec![]),
        Act::Foreman => (
            "foreman turn: outfit, belt, fleet, boost".to_owned(),
            vec![],
        ),
        Act::Sov => ("sov turn: plan, duty, sortie".to_owned(), vec![]),
        Act::Fleet(order) => order.describe(),
        Act::Hunt(op) => op.describe(),
        Act::Kit(op) => op.describe(),
        Act::Gang(op) => op.describe(),
        Act::Wing(op) => op.describe(),
        Act::Econ => (
            "econ turn: geodes, SKIN, injectors, PLEX".to_owned(),
            vec![],
        ),
        Act::Rare => (
            "rare ore turn: deep belt out, or stock home".to_owned(),
            vec![],
        ),
        Act::Stance { hurt, range, .. } => (
            format!("SetShipStance (hurt {hurt:.2}, target at {range:.0?} m)"),
            vec![],
        ),
        Act::Resync => ("UpdateStateRequest".to_owned(), vec![]),
        Act::Orbit { id, range } => (format!("CmdOrbit {id} {range:.0}"), vec![*id]),
        Act::Fire { gun, target } => (format!("Activate {gun} -> {target}"), vec![*gun, *target]),
        Act::CeaseFire(g) => (format!("Deactivate {g}"), vec![*g]),
        Act::LoadAmmo {
            guns,
            charges,
            location,
        } => (
            format!("LoadAmmo {guns:?} from {charges:?} at {location}"),
            guns.iter().map(|g| g.0).collect(),
        ),
        Act::Stock { item, quantity } => (format!("Add {item} qty={quantity} flag=5"), vec![*item]),
        Act::Loot(w) => (format!("Loot All {w}"), vec![*w]),
        Act::Salvage { module, wreck } => (
            format!("Activate {module} salvaging -> {wreck}"),
            vec![*module, *wreck],
        ),
        Act::Logi(op) => op.describe(),
        Act::Say(line) => (line.clone(), vec![]),
        Act::Wait(d) => (format!("{d:?}"), vec![]),
        Act::Missing(w) => ((*w).to_owned(), vec![]),
        Act::AlignTo(id) => (format!("CmdAlignTo {id}"), vec![*id]),
        Act::Stop => ("CmdStop".to_owned(), vec![]),
        Act::TravelTo { system } => (format!("TravelTo system {system}"), vec![*system]),
        Act::JumpGate { gate } => (format!("JumpTo gate {gate}"), vec![*gate]),
        Act::Reconnect => ("Reconnect".to_owned(), vec![]),
    }
}

/// **The throttle a pilot opens when he moves** (ENGINE-FIX): a fresh ship's
/// speed fraction is 0, so an orbit or a follow of a moving ship (another
/// pilot, a bot) commands no thrust and the ship sat where it was. Against a
/// still target the tether snaps and speed was never seen. Full, because a
/// bot flies as a player does with the slider at the end **[A]**; falsified
/// by a bot that should creep (a miner easing up to a rock) and does not.
/// MOVE-DEDUPE: whether a follow/orbit (`orbit`, `id`, `range`) repeats the
/// last one sent. It does only when the same ship sent it, the verb, target
/// and range (within [`MOVE_RANGE_SLACK`]) match, and the server's ball is
/// still tethered to that target; a stop, warp, dock or session change
/// clears the tether or the ship, so the command goes out again.
fn repeats_move(
    last: Option<(i64, bool, i64, f64)>,
    ship: Option<i64>,
    tether: Option<(i64, f64)>,
    orbit: bool,
    id: i64,
    range: f64,
) -> bool {
    let (Some(ship), Some((s, o, t, r))) = (ship, last) else {
        return false;
    };
    s == ship
        && o == orbit
        && t == id
        && (r - range).abs() <= MOVE_RANGE_SLACK
        && tether.is_some_and(|(b, _)| b == id)
}

/// Metres of range difference still counted as the same command.
const MOVE_RANGE_SLACK: f64 = 50.0;

fn full_throttle(conn: &mut Conn) -> Result<()> {
    conn.set_speed_fraction(1.0).map(|_| ())
}

/// The GM lines that hand a dev-spawned hunter its tackle skill, scrambler
/// and web; none for a policy without a PvP doctrine. Under the `mjd` kit
/// drill (`drill`) the foe gets none (it must stay free to jump) and its
/// allies get only a disruptor, which pins it without a client.
fn tackle_lines(policy: &crate::brain::Policy, drill: Option<&str>, foe: bool) -> Vec<String> {
    if policy.pvp.is_none() || (drill == Some("mjd") && foe) {
        return Vec::new();
    }
    if drill == Some("mjd") {
        return vec![
            format!("/giveskill {} 1", crate::hunt::SKILL_PROPULSION_JAMMING),
            format!("/createitem {}", crate::hunt::DISRUPTOR),
        ];
    }
    vec![
        format!("/giveskill {} 1", crate::hunt::SKILL_PROPULSION_JAMMING),
        format!("/createitem {}", crate::hunt::SCRAMBLER),
        format!("/createitem {}", crate::hunt::STASIS_WEBIFIER),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fix-w8b B2: the tackle grant keys on the doctrine alone (`/bots` bots
    /// run closed); a policy without one gets none.
    #[test]
    fn a_closed_pvp_policy_gets_the_tackle_lines_and_a_plain_one_none() {
        let mut policy = crate::brain::Policy::fast();
        policy.closed = true;
        assert!(tackle_lines(&policy, None, false).is_empty());
        policy.pvp = Some(std::sync::Arc::new(crate::hunt::Ground::new()));
        let lines = tackle_lines(&policy, None, false);
        assert_eq!(lines.len(), 3);
        assert!(lines.contains(&format!("/createitem {}", crate::hunt::SCRAMBLER)));
        assert!(lines.contains(&format!("/createitem {}", crate::hunt::STASIS_WEBIFIER)));
        // The mjd drill skips them; the smartbomb drill does not.
        assert!(tackle_lines(&policy, Some("mjd"), true).is_empty());
        let ally = tackle_lines(&policy, Some("mjd"), false);
        assert_eq!(
            ally,
            vec![
                format!("/giveskill {} 1", crate::hunt::SKILL_PROPULSION_JAMMING),
                format!("/createitem {}", crate::hunt::DISRUPTOR),
            ]
        );
        assert_eq!(tackle_lines(&policy, Some("smartbomb"), true).len(), 3);
    }

    /// A wing follower whose FC warped off grid does not resync; every other
    /// adrift bot still does (lane fight-stall-fix, step 6).
    #[test]
    fn only_a_wing_follower_that_lost_its_fc_skips_the_resync() {
        const FC: i64 = 90_000_007;
        const OTHER: i64 = 90_000_008;
        assert!(skips_resync(true, Some(FC), Some(FC)), "the FC warped");
        assert!(!skips_resync(false, Some(FC), Some(FC)), "no wing: resync");
        assert!(
            !skips_resync(true, Some(FC), Some(OTHER)),
            "lost another pilot"
        );
        assert!(!skips_resync(true, Some(FC), None), "an ownerless ball");
        assert!(!skips_resync(true, None, None), "no FC known");
        assert!(
            !skips_resync(false, None, Some(OTHER)),
            "a miner lost a rock"
        );
    }

    /// MOVE-DEDUPE: the same follow twice is one command; a stop, another
    /// range, verb, target or ship sends again.
    #[test]
    fn a_repeated_move_is_skipped_until_something_changes() {
        let last = Some((1, false, 7, 10_000.0));
        let held = Some((7, 10_000.0));
        assert!(repeats_move(last, Some(1), held, false, 7, 10_000.0));
        assert!(repeats_move(last, Some(1), held, false, 7, 10_020.0));
        // Never sent yet.
        assert!(!repeats_move(None, Some(1), held, false, 7, 10_000.0));
        // Stopped, warped or docked: no tether any more.
        assert!(!repeats_move(last, Some(1), None, false, 7, 10_000.0));
        // Tethered to another ball.
        assert!(!repeats_move(
            last,
            Some(1),
            Some((8, 1.0)),
            false,
            7,
            10_000.0
        ));
        // A different range, target, verb, or ship (session change).
        assert!(!repeats_move(last, Some(1), held, false, 7, 5_000.0));
        assert!(!repeats_move(last, Some(1), held, false, 8, 10_000.0));
        assert!(!repeats_move(last, Some(1), held, true, 7, 10_000.0));
        assert!(!repeats_move(last, Some(2), held, false, 7, 10_000.0));
        assert!(!repeats_move(last, None, held, false, 7, 10_000.0));
    }

    #[test]
    fn a_refusal_names_its_stop_reason() {
        assert_eq!(
            known_key("dogmaIM.MachoBindObject: NotEnoughCargoSpace"),
            Some("NotEnoughCargoSpace")
        );
        assert_eq!(
            known_key("Activate(1, \"miningLaser\"): no OnGodmaShipEffect (TargetTooFar)"),
            Some("TargetTooFar")
        );
        assert_eq!(known_key("no UserError key"), None);
    }

    #[test]
    fn a_miner_i_reaches_as_far_as_the_server_says() {
        let none = |_| None;
        assert!(crate::modules::laser_range(none, None, brain::MINER_I).0 > 0.0);
        assert_eq!(crate::modules::laser_range(none, None, -1).0, 0.0);
    }

    /// Ruling 2 (IP41): a bot logs in with its token, never its name. A
    /// server with one account, whose token is invented: the bot holding
    /// that token gets in, and the same bot without it, which would get in
    /// if the name were what it sent, is refused. The default token is one
    /// the server's `check_names` keeps out of every accounts file.
    #[test]
    fn the_token_is_never_the_name() {
        use eve_server::accounts::{Accounts, TOOL_TOKENS, hex, sha256};
        use eve_server::server::{Config, Server};

        const NAME: &str = "Invented Tokbot";
        const TOKEN: &str = "ip41-invented-token-not-a-name";
        let config = BotConfig::new(NAME, "127.0.0.1:1".parse().unwrap(), Pace::Fast);
        assert_eq!(config.token, DEV_TOKEN);
        assert_ne!(config.token, config.name);
        assert!(TOOL_TOKENS.contains(&DEV_TOKEN));
        assert_eq!(config.clone().with_token(TOKEN).token, TOKEN);

        let line = format!(
            "{}\t7001\tplayer\tinvented\n",
            hex(&sha256(TOKEN.as_bytes()))
        );
        let hs = eve_server::handshake::Config {
            accounts: Some(Arc::new(Accounts::parse(&line).unwrap())),
            ..eve_server::handshake::Config::default()
        };
        let mut server = Config::with_handshake("127.0.0.1:0".parse().unwrap(), hs);
        // Owned by the token's account: with --accounts a session sees only
        // its own characters (IP27-b).
        server.sandbox.characters = vec![eve_server::model::Character {
            character_id: 90_000_001,
            name: NAME.to_owned(),
            owner_user_id: Some(7001),
            ..eve_server::model::Character::default()
        }];
        let server = Server::bind(server).expect("bind");
        let addr = server.local_addr().expect("addr");
        std::thread::spawn(move || {
            let _ = server.run();
        });

        let first = |config: BotConfig| -> Entry {
            let mut config = config;
            config.addr = addr;
            config.chat = false;
            config.policy.gm_setup = false;
            let (h, t) = spawn(config);
            let deadline = Instant::now() + Duration::from_secs(30);
            let found = loop {
                let log = h.log();
                let hit = log
                    .iter()
                    .find(|e| matches!(e.kind, ActKind::Connect | ActKind::Lost))
                    .cloned();
                if hit.is_some() || Instant::now() > deadline {
                    break hit;
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            h.stop();
            let _ = t.join();
            found.expect("the bot connected or was refused")
        };
        let with = first(config.clone().with_token(TOKEN));
        assert_eq!((with.kind, with.verdict), (ActKind::Connect, Verdict::Ok));
        let without = first(config);
        assert_eq!(without.kind, ActKind::Lost, "{without:?}");
    }

    #[test]
    fn fast_pace_has_no_delays_at_all() {
        assert_eq!(Pace::Fast.reaction(), (Duration::ZERO, Duration::ZERO));
        assert_eq!(Pace::Fast.stagger(), (Duration::ZERO, Duration::ZERO));
        assert_eq!(Pace::Fast.policy().dwell, (Duration::ZERO, Duration::ZERO));
    }

    /// C7 §6.3: payloads wanted in bursts, in a trickle and all at once are
    /// written when the pacer says, and no window of [`PACING_WINDOW`] --
    /// starting at any send -- holds more than [`BOT_PAYLOADS_PER_S`] a
    /// second of them. Every payload is written eventually, in order.
    #[test]
    fn the_payload_rate_never_exceeds_bot_payloads_per_s_over_any_pacing_window() {
        let t0 = Instant::now();
        let mut wants: Vec<Duration> = Vec::new();
        wants.extend(std::iter::repeat_n(Duration::ZERO, 200));
        wants.extend((0..100).map(|i| PACING_WINDOW * 30 + PACING_WINDOW / 40 * i));
        wants.extend(std::iter::repeat_n(PACING_WINDOW * 90, 150));
        let mut p = Pacer::standard();
        let mut clock = t0;
        let mut sends = Vec::new();
        for want in wants {
            // The caller sleeps out each wait before it wants the next.
            let now = clock.max(t0 + want);
            let wait = p.admit(now);
            clock = now + wait;
            sends.push(clock);
        }
        assert!(sends.windows(2).all(|w| w[0] <= w[1]));
        let cap = BOT_PAYLOADS_PER_S * PACING_WINDOW.as_secs_f64();
        for (i, &start) in sends.iter().enumerate() {
            let inside = sends[i..]
                .iter()
                .take_while(|&&s| s < start + PACING_WINDOW)
                .count();
            assert!(
                inside as f64 <= cap,
                "{inside} payloads in a window from send {i}"
            );
        }
    }

    /// The bot marker and the fleet boss are not humans; nor is the bot
    /// itself, nor a ball with no pilot (an NPC). Any other pilot is.
    #[test]
    fn the_bot_marker_and_the_fleet_boss_are_not_humans() {
        let bots = BTreeSet::from([90_000_002]);
        let (me, boss) = (Some(90_000_001), Some(90_000_003));
        assert!(is_human(Some(90_000_004), me, boss, &bots));
        assert!(!is_human(Some(90_000_002), me, boss, &bots));
        assert!(!is_human(Some(90_000_003), me, boss, &bots));
        assert!(!is_human(Some(90_000_001), me, boss, &bots));
        assert!(!is_human(None, me, boss, &bots));
        assert!(is_human(Some(90_000_003), me, None, &bots));
    }

    /// An invented chain of systems `1 - 2 - ... - N`, longer than the home
    /// range, so system `k` is `k - 1` jumps from system 1.
    fn chain() -> Graph {
        let n = i64::from(HOME_RANGE_JUMPS) + 4;
        let mut g = Graph::new();
        for s in 1..n {
            g.entry(s).or_default().insert(s + 1);
            g.entry(s + 1).or_default().insert(s);
        }
        g
    }

    /// C7 §6.3: a home is the humans' home or a system within
    /// [`HOME_RANGE_JUMPS`] of it, whoever the bot is; a list naming a
    /// system farther out is refused while the graph is known.
    #[test]
    fn a_home_is_within_home_range_jumps_of_the_humans_home() {
        let g = chain();
        let homes = bot_homes(Some(&g), 1, &[]).unwrap();
        let reach = i64::from(HOME_RANGE_JUMPS);
        assert_eq!(homes, (1..=1 + reach).collect::<Vec<_>>());
        for i in 0..50 {
            let home = pick_home(&format!("Bot{i}"), &homes).unwrap();
            assert!((1..=1 + reach).contains(&home), "Bot{i} -> {home}");
        }
        let too_far = 2 + reach;
        assert!(bot_homes(Some(&g), 1, &[too_far]).is_err());
        assert_eq!(
            bot_homes(Some(&g), 1, &[1 + reach]).unwrap(),
            vec![1 + reach]
        );
        assert_eq!(bot_homes(None, 1, &[]).unwrap(), vec![1]);
        assert_eq!(pick_home("Bot1", &[]), None);
    }

    // ---- society mode (SOC-A2) ----------------------------------------------------

    use crate::career::{Band, INCOME_REF, Support};
    use crate::mind::{HYSTERESIS, MIN_TENURE, SWITCH_CONFIRMATIONS, TRAIT_COUNT};

    /// Invented careers: the rival has more affinity and more income than
    /// the incumbent, whatever the traits, and another role.
    static FIXTURE: &[Career] = &[
        Career {
            name: "t_stay",
            role: Some(&roles::MINE),
            support: Support::Served,
            base: 0.2,
            affinity: [0.0; TRAIT_COUNT],
            band: Band::High,
            prior_isk_h: INCOME_REF,
        },
        Career {
            name: "t_rival",
            role: Some(&roles::ESCORT),
            support: Support::Served,
            base: 1.0,
            affinity: [0.0; TRAIT_COUNT],
            band: Band::High,
            prior_isk_h: 10.0 * INCOME_REF,
        },
    ];

    /// A server on a free port whose roster is `names`, each docked at
    /// the sandbox's station in its capsule. Invented ids.
    fn society_server(names: &[&str]) -> SocketAddr {
        let mut config = eve_server::server::Config::new("127.0.0.1:0".parse().expect("literal"));
        config.sandbox.characters = names
            .iter()
            .enumerate()
            .map(|(i, n)| eve_server::model::Character {
                character_id: 90_000_501 + i as i64,
                name: (*n).to_owned(),
                ship_id: 100_000_501 + i as i64,
                ..eve_server::model::Character::default()
            })
            .collect();
        let server = eve_server::server::Server::bind(config).expect("bind");
        let addr = server.local_addr().expect("addr");
        std::thread::spawn(move || {
            let _ = server.run();
        });
        addr
    }

    fn society_bot(name: &str, addr: SocketAddr, soc: Society) -> BotConfig {
        let mut config = BotConfig::new(name, addr, Pace::Fast).with_society(soc);
        config.chat = false;
        config.policy.gm_setup = false;
        config
    }

    /// Poll `h`'s log until `done` finds what it wants or `within` passes.
    fn wait_for(h: &Handle, within: Duration, done: impl Fn(&[Entry]) -> bool) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if done(&h.log()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        done(&h.log())
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("soc-a2-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// **The harness test** (SOC-A2): a bot whose fixture rival beats its
    /// career by more than [`HYSTERESIS`], past [`MIN_TENURE`], on its last
    /// confirmation, switches at its first docked decision and plays the
    /// rival's role. The state file then holds the new career: a restarted
    /// bot reads it back, plays that role from its first turn, and stays.
    #[test]
    fn a_bot_switches_role_at_dock_and_its_state_survives_a_restart() {
        const NAME: &str = "Invented Socbot";
        const SEED: u64 = 7;
        let addr = society_server(&[NAME]);
        let dir = scratch("switch");
        let signals = Signals::default();
        let now = unix_now();
        let mut mind = Mind::seeded(NAME, SEED, FIXTURE, &signals, now);
        let scores = mind.scores(FIXTURE, &signals);
        assert!(
            scores[1].1 > scores[0].1 * (1.0 + HYSTERESIS),
            "the fixture's rival wins by more than the threshold: {scores:?}"
        );
        mind.career = "t_stay".to_owned();
        mind.since = now - MIN_TENURE - 1;
        mind.pending = Some(("t_rival".to_owned(), SWITCH_CONFIRMATIONS - 1));
        let mut soc = Society::new(mind, FIXTURE, signals.clone());
        soc.state = Some(dir.clone());
        soc.decide_every = Duration::ZERO;
        let config = society_bot(NAME, addr, soc.clone());
        assert_eq!(config.policy.role.name(), roles::MINE.name());

        let (h, t) = spawn(config);
        let switched = wait_for(&h, Duration::from_secs(30), |log| {
            log.iter().any(|e| {
                e.kind == ActKind::Career && e.what.starts_with("switch t_stay -> t_rival")
            })
        });
        h.stop();
        let _ = t.join();
        let log = h.log();
        assert!(switched, "{log:#?}");
        let at = log.iter().position(|e| e.kind == ActKind::Career).unwrap();
        assert!(
            log[at]
                .what
                .ends_with(&format!("role {}", roles::ESCORT.name())),
            "{:?}",
            log[at]
        );
        assert!(
            log[..at]
                .iter()
                .any(|e| e.kind == ActKind::Observe && e.what.starts_with("docked")),
            "decided docked: {log:#?}"
        );
        assert_eq!(soc.career(), "t_rival");

        let (again, source) = Society::open(NAME, SEED, FIXTURE, signals, Some(&dir), unix_now());
        assert_eq!(source, crate::state::Source::Restored);
        assert_eq!(again.career(), "t_rival");
        assert_eq!(again.role().name(), roles::ESCORT.name());
        let mut again = again;
        again.decide_every = Duration::ZERO;
        let config = society_bot(NAME, addr, again);
        assert_eq!(config.policy.role.name(), roles::ESCORT.name());
        let (h, t) = spawn(config);
        let decided = wait_for(&h, Duration::from_secs(30), |log| {
            log.iter().any(|e| e.kind == ActKind::Career)
        });
        h.stop();
        let _ = t.join();
        let log = h.log();
        assert!(decided, "{log:#?}");
        let first = log.iter().find(|e| e.kind == ActKind::Career).unwrap();
        assert!(first.what.starts_with("stay t_rival"), "{first:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Mid-trip, the role stays** (SOC-A2): no decision in space, in
    /// transit, or docked while stranded away from home; docked, one when
    /// due and none before.
    #[test]
    fn a_decision_is_docked_only_never_mid_trip() {
        let now = Instant::now();
        let every = Duration::from_secs(60);
        let docked = Place::Docked { station: 1 };
        let space = Place::InSpace {
            system: 2,
            bound: true,
        };
        assert!(decision_point(docked, false, None, now, every));
        assert!(!decision_point(space, false, None, now, every));
        assert!(!decision_point(Place::Transit, false, None, now, every));
        assert!(!decision_point(docked, true, None, now, every));
        assert!(!decision_point(docked, false, Some(now), now, every));
        assert!(decision_point(docked, false, Some(now), now + every, every));
    }

    /// **The harness test for sessions** (SOC-A2): with a short invented
    /// timetable, the bot logs on, logs off docked when its window ends,
    /// waits, and logs on again at the next window.
    #[test]
    fn a_bot_logs_off_at_its_window_end_and_back_on() {
        const NAME: &str = "Invented Shiftbot";
        let addr = society_server(&[NAME]);
        let signals = Signals::default();
        let mind = Mind::seeded(NAME, 1, FIXTURE, &signals, unix_now());
        let mut soc = Society::new(mind, FIXTURE, signals);
        soc.timetable = Timetable {
            period: 8,
            windows: vec![(0, 4)],
        };
        let online = Arc::clone(&soc.online);
        let (h, t) = spawn(society_bot(NAME, addr, soc));
        let cycled = wait_for(&h, Duration::from_secs(60), |log| {
            let kinds: Vec<(ActKind, bool)> = log
                .iter()
                .map(|e| (e.kind, e.what.starts_with("log off")))
                .collect();
            let first = kinds.iter().position(|k| k.0 == ActKind::Connect);
            let off = first.and_then(|i| {
                kinds[i..]
                    .iter()
                    .position(|k| *k == (ActKind::Session, true))
                    .map(|j| i + j)
            });
            off.is_some_and(|j| kinds[j..].iter().any(|k| k.0 == ActKind::Connect))
        });
        h.stop();
        let _ = t.join();
        assert!(cycled, "{:#?}", h.log());
        assert_eq!(online.load(Ordering::SeqCst), 0, "logged off at the end");
    }

    /// **A bubble release and wake** (`bubble.rs`): a released bot logs off
    /// through the ordinary session end -- with the same `Park` a timetable's
    /// end makes, so the server's offline settlement is the only thing that
    /// can pay for the absence -- stays off, and logs on again when woken.
    #[test]
    fn a_released_bot_logs_off_stays_off_and_logs_on_when_woken() {
        const NAME: &str = "Invented Shedbot";
        let addr = society_server(&[NAME]);
        let signals = Signals::default();
        let mind = Mind::seeded(NAME, 1, FIXTURE, &signals, unix_now());
        let soc = Society::new(mind, FIXTURE, signals);
        let online = Arc::clone(&soc.online);
        let (h, t) = spawn(society_bot(NAME, addr, soc));
        h.watch();
        let connects = |h: &Handle| {
            h.log()
                .iter()
                .filter(|e| e.kind == ActKind::Connect)
                .count()
        };
        assert!(
            wait_for(&h, Duration::from_secs(30), |_| h.is_live()),
            "{:#?}",
            h.log()
        );
        h.release();
        assert!(
            wait_for(&h, Duration::from_secs(60), |log| {
                log.iter()
                    .any(|e| e.what.starts_with("log off") && e.what.contains("bubble"))
            }),
            "{:#?}",
            h.log()
        );
        // The park line is the timetable path's: a career, parked or refused.
        let park = h
            .log()
            .iter()
            .any(|e| e.kind == ActKind::Session && e.what.contains("parked"));
        assert!(park, "{:#?}", h.log());
        std::thread::sleep(Duration::from_secs(2));
        assert!(!h.is_live());
        assert_eq!(online.load(Ordering::SeqCst), 0);
        assert_eq!(connects(&h), 1, "it stays off while released");
        h.wake();
        assert!(
            wait_for(&h, Duration::from_secs(30), |_| connects(&h) == 2
                && h.is_live()),
            "{:#?}",
            h.log()
        );
        h.stop();
        let _ = t.join();
    }

    /// **The way home** (SOC-A2, the travel lane's follow-up): a bot that
    /// logs on one jump from its home station -- as a trip cut off midway
    /// leaves it -- flies home over the gate before its role gets a turn.
    /// Invented systems, gates and station.
    #[test]
    fn a_stranded_bot_flies_home_over_the_gates() {
        const NAME: &str = "Invented Strandbot";
        const B: i64 = 30_000_903;
        const STATION_B: i64 = 60_900_901;
        const GATE_A: i64 = 50_900_901;
        const GATE_B: i64 = 50_900_902;
        const GATE_TYPE: i64 = 990_400_901;
        let home = eve_server::model::Character::default();
        let a = home.solar_system_id.expect("the default pilot's system");
        let station_a = home.station_id.expect("the default pilot is docked");
        let (constellation, region) = (home.constellation_id, home.region_id);
        let station_type = home.station_type_id;
        let catalogue = || {
            eve_server::celestial::Catalogue::parse(&format!(
                "#system\t{a}\t{constellation}\t{region}\n\
                 #system\t{B}\t{constellation}\t{region}\n\
                 station\t{station_a}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{a}\t0\t0\n\
                 gate\t{GATE_A}\t{GATE_TYPE}\t2473.0\t1000000.0\t0.0\t0.0\t{a}\t{GATE_B}\t{B}\n\
                 station\t{STATION_B}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{B}\t0\t0\n\
                 gate\t{GATE_B}\t{GATE_TYPE}\t2473.0\t0.0\t0.0\t-1000000.0\t{B}\t{GATE_A}\t{a}\n"
            ))
        };
        let mut config = eve_server::server::Config::new("127.0.0.1:0".parse().expect("literal"));
        config.sandbox.characters = vec![
            eve_server::model::Character {
                character_id: 90_000_601,
                name: NAME.to_owned(),
                ship_id: 100_000_601,
                station_id: Some(STATION_B),
                solar_system_id: Some(B),
                ..home.clone()
            },
            // Seeded at home, so this world knows the home station.
            eve_server::model::Character {
                character_id: 90_000_602,
                name: "Invented Homebody".to_owned(),
                ship_id: 100_000_602,
                ..home.clone()
            },
        ];
        config.sandbox.celestials = catalogue();
        let server = eve_server::server::Server::bind(config).expect("bind");
        let addr = server.local_addr().expect("addr");
        std::thread::spawn(move || {
            let _ = server.run();
        });
        let map = crate::travel::Map::from_catalogue(&catalogue(), None)
            .with_security(a, 1.0)
            .with_security(B, 1.0);

        let mut config = BotConfig::new(NAME, addr, Pace::Fast);
        config.chat = false;
        config.policy.gm_setup = false;
        config.policy.travel = Some(Arc::new(map));
        config.home_station = Some(station_a);
        let (h, t) = spawn(config);
        let home_again = wait_for(&h, Duration::from_secs(150), |log| {
            log.iter()
                .any(|e| e.kind == ActKind::Travel && e.verdict == Verdict::Ok)
        });
        h.stop();
        let _ = t.join();
        let log = h.log();
        assert!(home_again, "{log:#?}");
        let trip = log.iter().find(|e| e.kind == ActKind::Travel).unwrap();
        assert!(trip.what.ends_with("1 jumps"), "{trip:?}");
        assert_eq!(trip.ids, vec![station_a]);
    }

    /// **PULL-LEASE**: a lent bot works from the lease station and keeps
    /// its home; after the recall its next dock is the base. Invented
    /// systems, gates and stations.
    #[test]
    fn a_lent_bot_docks_at_the_lease_and_after_the_recall_at_its_base() {
        const NAME: &str = "Invented Lentbot";
        const B: i64 = 30_000_903;
        const STATION_B: i64 = 60_900_901;
        const GATE_A: i64 = 50_900_901;
        const GATE_B: i64 = 50_900_902;
        const GATE_TYPE: i64 = 990_400_901;
        let home = eve_server::model::Character::default();
        let a = home.solar_system_id.expect("the default pilot's system");
        let station_a = home.station_id.expect("the default pilot is docked");
        let (constellation, region) = (home.constellation_id, home.region_id);
        let station_type = home.station_type_id;
        let catalogue = || {
            eve_server::celestial::Catalogue::parse(&format!(
                "#system\t{a}\t{constellation}\t{region}\n\
                 #system\t{B}\t{constellation}\t{region}\n\
                 station\t{station_a}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{a}\t0\t0\n\
                 gate\t{GATE_A}\t{GATE_TYPE}\t2473.0\t1000000.0\t0.0\t0.0\t{a}\t{GATE_B}\t{B}\n\
                 station\t{STATION_B}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{B}\t0\t0\n\
                 gate\t{GATE_B}\t{GATE_TYPE}\t2473.0\t0.0\t0.0\t-1000000.0\t{B}\t{GATE_A}\t{a}\n"
            ))
        };
        let mut config = eve_server::server::Config::new("127.0.0.1:0".parse().expect("literal"));
        config.sandbox.characters = vec![
            eve_server::model::Character {
                character_id: 90_000_611,
                name: NAME.to_owned(),
                ship_id: 100_000_611,
                ..home.clone()
            },
            // Seeded at the lease station, so this world knows it.
            eve_server::model::Character {
                character_id: 90_000_612,
                name: "Invented Homebody".to_owned(),
                ship_id: 100_000_612,
                station_id: Some(STATION_B),
                solar_system_id: Some(B),
                ..home.clone()
            },
        ];
        config.sandbox.celestials = catalogue();
        let server = eve_server::server::Server::bind(config).expect("bind");
        let addr = server.local_addr().expect("addr");
        std::thread::spawn(move || {
            let _ = server.run();
        });
        let map = crate::travel::Map::from_catalogue(&catalogue(), None)
            .with_security(a, 1.0)
            .with_security(B, 1.0);

        let mut config = BotConfig::new(NAME, addr, Pace::Fast);
        config.chat = false;
        config.policy.gm_setup = false;
        config.policy.travel = Some(Arc::new(map));
        config.home_station = Some(station_a);
        let (h, t) = spawn(config);
        h.lend(STATION_B);
        let there = wait_for(&h, Duration::from_secs(150), |log| {
            log.iter().any(|e| {
                e.kind == ActKind::Travel
                    && e.what.starts_with("return home to station")
                    && e.ids == vec![STATION_B]
                    && e.verdict == Verdict::Ok
            })
        });
        let leased = h.leased_to();
        h.recall();
        let back = wait_for(&h, Duration::from_secs(150), |log| {
            log.iter().any(|e| {
                e.kind == ActKind::Travel
                    && e.what.starts_with("return home to station")
                    && e.ids == vec![station_a]
                    && e.verdict == Verdict::Ok
            })
        });
        h.stop();
        let _ = t.join();
        let log = h.log();
        assert!(there, "{log:#?}");
        assert_eq!(leased, Some(STATION_B));
        assert!(back, "{log:#?}");
        assert_eq!(h.leased_to(), None);
        // CLONE-HOME: the clone is set at the base, once, and never at the
        // lease station.
        let clone_at = |st: i64| {
            log.iter()
                .filter(|e| e.what.starts_with("clone home at") && e.ids == vec![st])
                .count()
        };
        assert_eq!(clone_at(station_a), 1, "{log:#?}");
        assert_eq!(clone_at(STATION_B), 0, "{log:#?}");
    }

    /// The timetable's arithmetic on an invented cycle: inside a window is
    /// online, the wait to the next window and to the window's end, a
    /// window that wraps the cycle, and `always`.
    #[test]
    fn a_timetable_is_on_inside_its_windows_only() {
        let t = Timetable {
            period: 100,
            windows: vec![(10, 20), (90, 20)],
        };
        assert!(t.online(5), "the wrapping window covers 90..110");
        assert!(t.online(10) && t.online(29) && !t.online(30));
        assert_eq!(t.until_online(30), 60);
        assert_eq!(t.until_online(130), 60);
        assert_eq!(t.until_offline(25), Some(5));
        assert_eq!(t.until_offline(50), Some(0));
        assert!(Timetable::always().online(12_345));
        assert_eq!(Timetable::always().until_offline(1), None);
    }

    /// The weekly timetable: one window a weekday, each as long as the
    /// bot's diligence says within the jitter, the same for the same name
    /// and seed, and a diligent bot online longer than a casual one.
    #[test]
    fn a_weekly_timetable_follows_diligence_and_is_deterministic() {
        let day = DAY_SECS;
        let lazy = Timetable::weekly("Invented Pilot", 3, 0.0, day);
        let keen = Timetable::weekly("Invented Pilot", 3, 1.0, day);
        assert_eq!(lazy, Timetable::weekly("Invented Pilot", 3, 0.0, day));
        assert_eq!(lazy.period, WEEK_DAYS * day);
        assert_eq!(lazy.windows.len(), WEEK_DAYS as usize);
        for (t, h) in [(&lazy, SESSION_HOURS.0), (&keen, SESSION_HOURS.1)] {
            for (k, &(start, len)) in t.windows.iter().enumerate() {
                assert!(start < t.period);
                let hours = len as f64 / 3600.0;
                let wk = if WEEKEND_DAYS.contains(&(k as u64)) {
                    WEEKEND_FACTOR
                } else {
                    1.0
                };
                assert!(
                    hours >= h * wk * (1.0 - SESSION_JITTER) - 0.01,
                    "{hours} vs {h}"
                );
                assert!(
                    hours <= h * wk * (1.0 + SESSION_JITTER) + 0.01,
                    "{hours} vs {h}"
                );
            }
        }
        assert!(keen.hours() > lazy.hours());
        assert_ne!(
            lazy.windows,
            Timetable::weekly("Another Pilot", 3, 0.0, day).windows
        );
        let short = Timetable::weekly("Invented Pilot", 3, 0.5, 60);
        assert_eq!(short.period, WEEK_DAYS * 60);
        assert!(short.windows.iter().all(|&(_, l)| (1..=60).contains(&l)));
    }

    /// PRIME-TIME: a profile with all weight in one hour puts every
    /// window's start within the jitter of it; a flat profile spreads the
    /// starts evenly over the day; the shipped file parses to 24 hours with
    /// the evening above the morning; a weekend window is longer.
    #[test]
    fn start_hours_follow_the_profile() {
        let hour = 5usize;
        let mut one = [0.0; PROFILE_HOURS];
        one[hour] = 1.0;
        for n in 0..50 {
            let t = Timetable::weekly_with(&one, &format!("Invented {n}"), 7, 0.5, DAY_SECS);
            for &(start, _) in &t.windows {
                let within_day = (start % DAY_SECS) as f64 / 3600.0;
                let d = (within_day - hour as f64 - 0.5).abs();
                let d = d.min(24.0 - d);
                assert!(d <= START_JITTER_H + 0.5 + 1e-6, "{within_day}");
            }
        }
        let flat = [1.0; PROFILE_HOURS];
        let mut blocks = [0u32; 4];
        let n = 10_000u32;
        for i in 0..n {
            let t = Timetable::weekly_with(&flat, &format!("Invented {i}"), 9, 0.5, DAY_SECS);
            let h = (t.windows[0].0 % DAY_SECS) / 3600;
            blocks[(h / 6) as usize] += 1;
        }
        for b in blocks {
            let share = b as f64 / n as f64;
            assert!((share - 0.25).abs() < 0.02, "{blocks:?}");
        }
        let shipped = start_profile();
        assert!(shipped.iter().all(|w| *w > 0.0));
        assert!(shipped[19] > shipped[9]);
        let (mut wd, mut we, mut nd, mut ne) = (0.0, 0.0, 0.0, 0.0);
        for i in 0..400 {
            let t = Timetable::weekly_with(&flat, &format!("Invented {i}"), 3, 0.5, DAY_SECS);
            for (k, &(_, l)) in t.windows.iter().enumerate() {
                if WEEKEND_DAYS.contains(&(k as u64)) {
                    we += l as f64;
                    ne += 1.0;
                } else {
                    wd += l as f64;
                    nd += 1.0;
                }
            }
        }
        let ratio = (we / ne) / (wd / nd);
        assert!((ratio / WEEKEND_FACTOR - 1.0).abs() < 0.1, "{ratio}");
        assert!(parse_profile("# none\n").iter().all(|w| *w == 1.0));
    }

    /// The graph comes off the catalogue's gate rows, both ways; a gate
    /// whose partner's system nothing names adds no edge. Invented rows.
    #[test]
    fn the_gate_graph_links_partner_systems_both_ways() {
        let cat = eve_server::celestial::Catalogue::parse(
            "#kind\tid\ttype_id\tradius\tx\ty\tz\tsystem\tdest\tdest_system\n\
             gate\t51\t1\t1.0\t0.0\t0.0\t0.0\t1\t52\t2\n\
             gate\t52\t1\t1.0\t0.0\t0.0\t0.0\t2\t51\t1\n\
             gate\t53\t1\t1.0\t0.0\t0.0\t0.0\t2\t99\t0\n",
        );
        let g = gate_graph(&cat);
        assert_eq!(g.get(&1), Some(&BTreeSet::from([2])));
        assert_eq!(g.get(&2), Some(&BTreeSet::from([1])));
        assert_eq!(g.len(), 2);
    }
}
