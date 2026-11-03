//! **The hacker** (R-HACK packet HK4, SOC-B4; the spec is gitignored): a
//! bot that finds data and relic sites, hacks their cans and sells what it
//! finds, the way an explorer does in a Heron with a Data Analyzer.
//!
//! Two halves:
//!
//! - **The solver** ([`BoardView`], [`next`], pure). The client is only a
//!   view of the game [V]: the server pushes `OnHackingTurnComplete(events)`
//!   and the player sends one of four inputs to `hackingMgr`. The bot does
//!   the same. [`BoardView`] is rebuilt from the pushed events alone (what a
//!   player sees, never the server's board), and [`next`] picks the next
//!   input ([`Move`]). [`play`] runs one game over the wire.
//! - **The career** ([`ROLE`], [`run`]): one [`Act::Hack`] is a trip from
//!   dock to dock:
//!   1. **Ready** (docked): board a hull, fit the analyzers in the hangar
//!      (group [`GROUP_ANALYZER`]) and online them, and outfit a probe
//!      launcher (`probe::outfit`). With no analyzer the trip ends lacking
//!      one, and the next docked turn asks GM setup or the closed economy's
//!      `Acquire` for [`KIT`].
//!   2. **Sell** what earlier trips brought home ([`sale_plan`]): into a
//!      bid when one is there, else listed at the seller's ask from the
//!      server's reference price. Datacores are what the builders'
//!      invention buys from asks (`docs/lanes/soc-b7.md`).
//!   3. **Find a site**: data and relic signatures the explorers booked on
//!      `probe::LEDGER` in this system first, then any other, scanned down
//!      with `probe::find_with` and warped to. Sites this process's hackers
//!      emptied are skipped ([`DONE`]).
//!   4. **On the grid**: every can whose slim carries
//!      `hackingSecurityState` 0 is approached, locked and hacked: the
//!      analyzer that suits the site's kind first, the other on a refusal
//!      (the can's skill decides, not the site's [I]). The game opens when
//!      the analyzer's cycle ends [A, the server's `START_ON_CYCLE_END`]; it
//!      is played by [`play`]; a won can is looted (`loot_all`: HK3 puts the
//!      loot in; until then it opens empty). A lost can is tried once more
//!      ([`RETRIES`]).
//!   5. **Home** when the hold is full, after [`MAX_SITES`] sites, or at
//!      [`TRIP_CEILING`]; unload the loot and sell it.
//!
//! ## The solver's policy (R-HACK §4 HK4) **[A]**
//!
//! In priority order; each step offers moves, and the first one the server
//! has not just refused ([`BoardView::rejected`]) is played:
//!
//! 1. **Bookkeeping.** The candidate core cells are the hidden cells
//!    consistent with every distance indicator seen (hex BFS over every
//!    board cell, as the server measures it). The largest indicator seen
//!    may be the server's cap, so it is read as "at least" [I].
//! 2. **Win if you can.** If the core is revealed and its kill costs less
//!    coherence than the virus has, attack it: Kernel Rot first when the
//!    core's coherence is above twice the strength, a Secondary Vector when
//!    held, the shield when the core hits hard ([`SHIELD_SHARE`]).
//! 3. **Survive.** Below [`LOW_COHERENCE`] with Self Repair held, use it.
//! 4. **Pick up utilities and open caches** when a slot is free.
//! 5. **Reveal** the free frontier cell with the least expected distance to
//!    the candidates, the fewest hidden neighbours on a tie; **unblock**
//!    (attack the blocking defenses, Restoration Nodes first, then
//!    Suppressors, then the cheapest) only when no free frontier is left,
//!    or when a blocked cell is [`UNBLOCK_GAIN`] closer and cheap
//!    ([`UNBLOCK_SHARE`]).
//! 6. **Otherwise** the cheapest attack that does not kill the virus.
//! 7. **Quit** ([`Move::Quit`]) only when every move left kills the virus
//!    or none exists: the server counts a quit as a failure, like a loss
//!    [V-derived], so quitting a lost game only saves its turns.
//!
//! Every rule and number here is **[A] policy** except the client's calls,
//! ids and event shapes, cited [V].

use crate::brain::{self, Act, Memory, Observation};
use crate::courier::{Here, Move as Travel, next_move};
use crate::explore_roam as roam;
use crate::pricing::{self, Quote};
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::verbs::{Book, SellLine};
use crate::{Conn, DOCK_RANGE, Error, ItemRow, LOOT_RANGE, LockState, Result};
use eve_marshal::PyValue;
use eve_server::hacking::{self as engine, Coord, Outcome};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ---- ids and numbers -----------------------------------------------------------

/// The Heron **[V-data]**: Caldari's exploration frigate, five mid slots.
pub const HULL: i64 = 605;
/// Data Analyzer I **[V-data]** (group 538, requires Hacking).
pub const DATA_ANALYZER_I: i64 = 22_175;
/// Relic Analyzer I **[V-data]** (group 538, requires Archaeology).
pub const RELIC_ANALYZER_I: i64 = 22_177;
/// The data analyzers this bot knows by id **[V-data]**: tried first on a
/// data site. Any other analyzer is tried first on a relic site.
pub const DATA_ANALYZERS: &[i64] = &[DATA_ANALYZER_I, 22_329];
/// `groupDataMiners` **[V-data]**: every analyzer's group, so any analyzer
/// the server knows is fitted.
pub const GROUP_ANALYZER: i64 = 538;
/// Hacking **[V-data]**: the data can's `requiredSkill1`.
pub const SKILL_HACKING: i64 = 21_718;
/// Archaeology **[V-data]**: the relic can's.
pub const SKILL_ARCHAEOLOGY: i64 = engine::SKILL_ARCHAEOLOGY;
/// The analyzer's effect as `dogmaIM.Activate` names it **[V-data]**
/// (`dogmaeffects[1738].effectName`).
pub const DO_HACKING: (&str, i64) = ("doHacking", engine::EFFECT_DO_HACKING);
/// `Activate`'s `repeat` for an analyzer: one cycle **[I]**
/// (`disallowRepeatingActivation` 1; the server runs `repeat <= 0` once).
pub const ONE_CYCLE: i64 = 0;
/// `maxRange` **[V]**.
pub const ATTR_MAX_RANGE: i64 = engine::ATTR_MAX_RANGE;
/// `duration` **[V]**: the analyzer's cycle, after which the game opens.
pub const ATTR_DURATION: i64 = 73;
/// The refusal of an analyzer that does not suit the can **[V key, the
/// server's choice]**: the other analyzer is tried.
pub const WRONG_ANALYZER: &str = "DeniedActivateTargetModuleDisallowed";

/// What the hacker sells that the market prices **[V-data]**: the common
/// datacores (group 333; the faction-tech ones left out), the class a data
/// can's loot is mostly made of (R-HACK §3), and what inventors consume.
/// The career's [`crate::signal`] basket.
pub const LOOT_TYPES: &[i64] = &[
    20_171, 20_172, 20_410, 20_411, 20_412, 20_413, 20_414, 20_415, 20_416, 20_417, 20_418, 20_419,
    20_420, 20_421, 20_423, 20_424, 25_887,
];

/// The client's `LOW_COHERENCE_WARN_LEVEL` **[V]**, as the policy's repair
/// threshold **[A]**: falsified by solvers that die with a repair held.
pub const LOW_COHERENCE: i64 = 50;
/// Raise the shield before attacking a node whose strength is at least
/// this share of the virus's coherence **[A]** (R-HACK: a quarter).
pub const SHIELD_SHARE: f64 = 0.25;
/// Unblock early only when the blocked cell is this many steps closer to
/// the core candidates than the best free one **[A]**.
pub const UNBLOCK_GAIN: f64 = 2.0;
/// ... and the blockers cost under this share of the coherence **[A]**.
pub const UNBLOCK_SHARE: f64 = 0.5;
/// The most turns one game is played before it is quit **[A]**: a bound
/// on a solver loop, never reached by a sound policy on a 10 by 9 board.
pub const MAX_TURNS: u32 = 300;
/// A lost can is hacked again this many times **[A]**: the server's
/// second failure destroys it (HK1's `failures_to_destroy`), so a player
/// tries once more. Falsified by hackers that lose most retries.
pub const RETRIES: u32 = 1;
/// Sites landed on in one trip before it goes home **[A]**.
pub const MAX_SITES: usize = 3;
/// The longest one trip lasts **[A]**: a player's sitting.
pub const TRIP_CEILING: Duration = Duration::from_secs(45 * 60);
/// How long a site's grid must be quiet before its cans are counted
/// **[A]**, the site runner's.
pub const SITE_SETTLE: Duration = crate::site_runner::SITE_SETTLE;
/// The range kept from a can while hacking it **[A]**: inside the loot
/// range, so the won can is looted where it was hacked.
pub const APPROACH_RANGE: f64 = 1_000.0;
/// How long after the analyzer's cycle the game's start may take **[A]**.
pub const START_SLACK: Duration = Duration::from_secs(10);
/// How long one turn's push may take **[A]**: it rides with the answer.
pub const TURN_WAIT: Duration = Duration::from_secs(5);
/// The `MISSING` line of a trip that found no site.
pub const MISSING_SITES: &str =
    "a scanned-down data or relic site (none here, or no probe launcher and probes)";

const POLL: Duration = Duration::from_millis(250);
const CEILING: Duration = crate::courier::TRAVEL_CEILING;

// The wire's tile types and subtypes [V `hackingConstants`], as the engine
// names them (pinned by `wire_constants_are_the_engines`).
const SEGMENT: i64 = engine::TYPE_SEGMENT;
const VIRUS: i64 = engine::TYPE_VIRUS;
const CORE: i64 = engine::TYPE_CORE;
const DEFENSE: i64 = engine::TYPE_DEFENSESOFTWARE;
const UTILITY_TILE: i64 = engine::TYPE_UTILITYELEMENTTILE;
const CACHE: i64 = engine::TYPE_DATACACHE;
/// `SELFREPAIR` 0, `KERNALROT` 1, `SECONDARYVECTOR` 2, `POLYMORPHICSHIELD`
/// 3 [V].
const SELF_REPAIR: i64 = 0;
const KERNEL_ROT: i64 = 1;
const SECONDARY_VECTOR: i64 = 2;
const SHIELD: i64 = 3;
/// `HONEYPOT_STRENGTH` 102, `HONEYPOT_HEALING` 103 [V].
const SUPPRESSOR: i64 = 102;
const RESTORATION: i64 = 103;

// ---- the board as the player sees it ---------------------------------------------

/// One tile as the pushes left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ViewTile {
    pub kind: i64,
    pub subtype: i64,
    pub hidden: bool,
    pub blocked: bool,
    pub strength: i64,
    pub coherence: i64,
}

/// One utility slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewSlot {
    pub id: Option<i64>,
    /// −1 when empty [V].
    pub subtype: i64,
    pub info: Option<i64>,
    /// `UE_INUSE` said so, and no `UE_REMOVED` since.
    pub active: bool,
    /// The last `durationRemaining` read for it.
    pub remaining: Option<i64>,
}

impl ViewSlot {
    fn empty(&self) -> bool {
        self.subtype < 0
    }
}

/// One input to `hackingMgr` [V].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Move {
    /// `ClickedOnTile(coord)`: reveal, attack, open or pick up.
    Click(Coord),
    /// `UsedUtilityElement(index, coord)`: Kernel Rot or Secondary Vector.
    UseOnTile(usize, Coord),
    /// `UsedUtilityElementOnVirus(index)`: Self Repair or the shield.
    UseOnVirus(usize),
    /// `QuitHackingAttempt()`.
    Quit,
}

/// **The game as the pushed events describe it**, and nothing else.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BoardView {
    pub tiles: BTreeMap<Coord, ViewTile>,
    pub virus_coherence: i64,
    pub virus_strength: i64,
    /// The coherence at `VIRUS_CREATED`.
    pub start_coherence: i64,
    pub slots: Vec<ViewSlot>,
    /// `DISTANCEINDICATOR`s seen, by cell.
    pub indicators: BTreeMap<Coord, i64>,
    /// `GAME_START` seen.
    pub started: bool,
    /// `None` while running.
    pub ended: Option<Outcome>,
    /// Inputs that were answered with a lone `ACK` since the last turn
    /// that changed anything: never sent again until something changes.
    pub rejected: BTreeSet<Move>,
    /// Valid turns played.
    pub turns: u32,
    /// The input last sent ([`BoardView::sent`]).
    last: Option<Move>,
}

fn get<'a>(v: &'a PyValue, key: &str) -> Option<&'a PyValue> {
    match v {
        PyValue::Dict(pairs) => pairs.iter().find_map(|(value, k)| match k {
            PyValue::Bytes(b) if b == key.as_bytes() => Some(value),
            PyValue::Unicode(s) if s == key => Some(value),
            _ => None,
        }),
        _ => None,
    }
}

fn int(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        PyValue::Bool(b) => Some(i64::from(*b)),
        PyValue::Real(r) => Some(r.round() as i64),
        _ => None,
    }
}

fn flag(v: Option<&PyValue>) -> Option<bool> {
    match v? {
        PyValue::Bool(b) => Some(*b),
        PyValue::Int(n) => Some(*n != 0),
        _ => None,
    }
}

fn coord(v: Option<&PyValue>) -> Option<Coord> {
    match v? {
        PyValue::Tuple(xy) | PyValue::List(xy) if xy.len() == 2 => Some((
            i32::try_from(int(xy.first())?).ok()?,
            i32::try_from(int(xy.get(1))?).ok()?,
        )),
        _ => None,
    }
}

fn coord_value(c: Coord) -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Int(i64::from(c.0)),
        PyValue::Int(i64::from(c.1)),
    ])
}

/// Hex steps from `from` to every cell of `cells` [V odd-r neighbours,
/// `engine::neighbours`].
fn bfs(cells: &BTreeSet<Coord>, from: Coord) -> BTreeMap<Coord, u32> {
    let mut dist = BTreeMap::new();
    if !cells.contains(&from) {
        return dist;
    }
    dist.insert(from, 0);
    let mut q = VecDeque::from([from]);
    while let Some(c) = q.pop_front() {
        let d = dist[&c];
        for n in engine::neighbours(c) {
            if cells.contains(&n) && !dist.contains_key(&n) {
                dist.insert(n, d + 1);
                q.push_back(n);
            }
        }
    }
    dist
}

impl BoardView {
    pub fn new() -> BoardView {
        BoardView::default()
    }

    /// The input about to be sent, so a lone `ACK` can be pinned on it.
    pub fn sent(&mut self, mv: Move) {
        self.last = Some(mv);
    }

    /// Fold one turn's events in (the list `OnHackingTurnComplete` carries).
    pub fn apply_turn(&mut self, events: &[PyValue]) {
        let lone_ack = events.len() == 1 && event_id(&events[0]) == Some(engine::EVENT_ACK);
        if lone_ack {
            if let Some(mv) = self.last {
                self.rejected.insert(mv);
            }
        } else if !events.is_empty() {
            self.rejected.clear();
            if self.started && self.last.is_some() {
                self.turns += 1;
            }
        }
        for e in events {
            self.apply(e);
        }
    }

    /// Fold one event in: `{'eventID': int, 'eventData': dict}` [V].
    pub fn apply(&mut self, event: &PyValue) {
        let Some(id) = event_id(event) else { return };
        let empty = PyValue::Dict(Vec::new());
        let d = get(event, "eventData").unwrap_or(&empty);
        match id {
            engine::EVENT_GAME_LOST => self.ended = Some(Outcome::Lost),
            engine::EVENT_GAME_WON => self.ended = Some(Outcome::Won),
            engine::EVENT_GAME_START => {
                self.started = true;
                self.ended = None;
            }
            engine::EVENT_VIRUS_CREATED => {
                // A new game (the server ends any old one first).
                let keep_tiles = std::mem::take(&mut self.tiles);
                *self = BoardView {
                    tiles: keep_tiles,
                    ..BoardView::default()
                };
                self.virus(d);
                self.start_coherence = self.virus_coherence;
                if let Some(PyValue::List(inv)) = get(d, "inventory") {
                    self.inventory(inv);
                }
            }
            engine::EVENT_TILE_CREATED => {
                if self.started || self.ended.is_some() {
                    // The first tile of a new game's start batch.
                    *self = BoardView::default();
                }
                if let Some(c) = coord(get(d, "coord")) {
                    self.tiles.insert(c, ViewTile::default());
                    self.record(d);
                }
            }
            engine::EVENT_OBJECT_KILLED
            | engine::EVENT_TILE_FLIPPED
            | engine::EVENT_DATACACHE_OPEN
            | engine::EVENT_KERNALROT => self.record(d),
            engine::EVENT_TILE_BLOCKED => {
                if let (Some(c), Some(b)) = (coord(get(d, "coord")), flag(get(d, "blocked")))
                    && let Some(t) = self.tiles.get_mut(&c)
                {
                    t.blocked = b;
                }
            }
            engine::EVENT_UE_PICKEDUP => {
                if let Some(r) = get(d, "replacementObject") {
                    self.record(r);
                }
                if let Some(PyValue::List(inv)) = get(d, "inventory") {
                    self.inventory(inv);
                }
            }
            engine::EVENT_UE_REMOVED => {
                if let Some(PyValue::List(inv)) =
                    get(d, "inventory").and_then(|i| get(i, "inventory"))
                {
                    self.inventory(inv);
                }
            }
            engine::EVENT_UE_INUSE => {
                if let (Some(i), Some(v)) = (int(get(d, "index")), flag(get(d, "value")))
                    && let Some(s) = self.slots.get_mut(i as usize)
                {
                    s.active = v;
                }
            }
            engine::EVENT_ATTACK => {
                for side in ["attackerResult", "defenderResult"] {
                    if let Some(r) = get(d, side) {
                        self.record(r);
                    }
                }
            }
            engine::EVENT_SELFREPAIR => {
                if let Some(t) = get(d, "target") {
                    self.virus(t);
                }
                self.remaining(d);
            }
            engine::EVENT_SECONDARYVECTOR => {
                self.record(d);
                self.remaining(d);
            }
            engine::EVENT_POLYMORPHICSHIELDHIT => self.remaining(d),
            engine::EVENT_HONEYPOT_STRENGTH => {
                if let Some(v) = get(d, "virus") {
                    self.virus(v);
                }
            }
            engine::EVENT_HONEYPOT_HEALING => {
                if let Some(h) = get(d, "healedObject") {
                    self.record(h);
                }
            }
            engine::EVENT_DISTANCEINDICATOR => {
                if let (Some(c), Some(n)) =
                    (coord(get(d, "coord")), int(get(d, "distanceIndicator")))
                {
                    self.indicators.insert(c, n);
                }
            }
            _ => {}
        }
    }

    /// A virus record, or a tile record (by `type`, as the client routes
    /// `ATTACK`'s two sides [V]).
    fn record(&mut self, r: &PyValue) {
        if int(get(r, "type")) == Some(VIRUS) && get(r, "coord").is_none() {
            self.virus(r);
            return;
        }
        let Some(c) = coord(get(r, "coord")) else {
            return;
        };
        let Some(t) = self.tiles.get_mut(&c) else {
            return;
        };
        if let Some(v) = int(get(r, "type")) {
            t.kind = v;
        }
        if let Some(v) = int(get(r, "subtype")) {
            t.subtype = v;
        }
        if let Some(v) = flag(get(r, "hidden")) {
            t.hidden = v;
        }
        if let Some(v) = flag(get(r, "blocked")) {
            t.blocked = v;
        }
        if let Some(v) = int(get(r, "strength")) {
            t.strength = v;
        }
        if let Some(v) = int(get(r, "coherence")) {
            t.coherence = v;
        }
    }

    fn virus(&mut self, v: &PyValue) {
        if let Some(c) = int(get(v, "coherence")) {
            self.virus_coherence = c;
        }
        if let Some(s) = int(get(v, "strength")) {
            self.virus_strength = s;
        }
    }

    fn remaining(&mut self, d: &PyValue) {
        if let (Some(i), Some(r)) = (int(get(d, "index")), int(get(d, "durationRemaining")))
            && let Some(s) = self.slots.get_mut(i as usize)
        {
            s.remaining = Some(r);
        }
    }

    /// A slot list [V keys `id`, `subtype`, `info`]: a slot that still
    /// holds the same utility keeps its "in use".
    fn inventory(&mut self, inv: &[PyValue]) {
        let old = std::mem::take(&mut self.slots);
        self.slots = inv
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let id = int(get(s, "id"));
                let subtype = int(get(s, "subtype")).unwrap_or(-1);
                let kept = old
                    .get(i)
                    .filter(|o| o.id == id && o.subtype == subtype && subtype >= 0);
                ViewSlot {
                    id,
                    subtype,
                    info: int(get(s, "info")),
                    active: kept.is_some_and(|o| o.active),
                    remaining: kept.and_then(|o| o.remaining),
                }
            })
            .collect();
    }

    /// Still being played.
    pub fn running(&self) -> bool {
        self.started && self.ended.is_none()
    }

    fn cells(&self) -> BTreeSet<Coord> {
        self.tiles.keys().copied().collect()
    }

    fn tile(&self, c: Coord) -> Option<&ViewTile> {
        self.tiles.get(&c)
    }

    fn passable(&self, c: Coord) -> bool {
        self.tile(c).is_some_and(|t| !t.hidden && t.kind == SEGMENT)
    }

    fn live_defense(&self, c: Coord) -> bool {
        self.tile(c)
            .is_some_and(|t| !t.hidden && t.kind == DEFENSE && t.coherence > 0)
    }

    fn next_to_passable(&self, c: Coord) -> bool {
        engine::neighbours(c).iter().any(|&n| self.passable(n))
    }

    /// The revealed, live core, if any.
    pub fn core(&self) -> Option<Coord> {
        self.tiles
            .iter()
            .find(|(_, t)| !t.hidden && t.kind == CORE && t.coherence > 0)
            .map(|(c, _)| *c)
    }

    /// **The cells the core may be in** (policy step 1).
    pub fn candidates(&self) -> Vec<Coord> {
        if let Some(c) = self.core() {
            return vec![c];
        }
        let hidden: Vec<Coord> = self
            .tiles
            .iter()
            .filter(|(_, t)| t.hidden)
            .map(|(c, _)| *c)
            .collect();
        if self.indicators.is_empty() {
            return hidden;
        }
        let cells = self.cells();
        let top = self.indicators.values().copied().max().unwrap_or(0);
        let fields: Vec<(BTreeMap<Coord, u32>, i64)> = self
            .indicators
            .iter()
            .map(|(&c, &n)| (bfs(&cells, c), n))
            .collect();
        let fit: Vec<Coord> = hidden
            .iter()
            .copied()
            .filter(|h| {
                fields.iter().all(|(dist, n)| {
                    dist.get(h).is_some_and(|&d| {
                        let d = i64::from(d);
                        d == *n || (*n == top && d >= *n)
                    })
                })
            })
            .collect();
        if fit.is_empty() { hidden } else { fit }
    }

    fn slot_of(&self, subtype: i64) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.subtype == subtype && !s.active)
    }

    fn free_slot(&self) -> bool {
        self.slots.iter().any(ViewSlot::empty)
    }

    /// Counter-hits a raised shield still absorbs.
    fn shield_left(&self) -> i64 {
        self.slots
            .iter()
            .filter(|s| s.subtype == SHIELD && s.active)
            .map(|s| s.remaining.unwrap_or(1).max(0))
            .sum()
    }

    /// **The coherence a kill of the node at `c` costs** with the strength
    /// the virus has now: one counter-hit per hit it survives, less what a
    /// raised shield absorbs. `coh` overrides the node's coherence.
    fn cost(&self, c: Coord, coh: Option<i64>) -> i64 {
        let Some(t) = self.tile(c) else {
            return i64::MAX;
        };
        let s = self.virus_strength.max(1);
        let coh = coh.unwrap_or(t.coherence).max(0);
        let hits = (coh + s - 1) / s;
        let counters = (hits - 1).max(0);
        (counters - self.shield_left()).max(0) * t.strength
    }
}

/// An event's `eventID`.
fn event_id(e: &PyValue) -> Option<i64> {
    int(get(e, "eventID"))
}

// ---- the policy --------------------------------------------------------------------

/// **The next input** for the game `view` shows (module header's policy);
/// [`Move::Quit`] when nothing but a fatal move is left.
pub fn next(view: &BoardView) -> Move {
    plan(view)
        .into_iter()
        .find(|m| !view.rejected.contains(m))
        .unwrap_or(Move::Quit)
}

/// The moves worth playing, best first (`next` takes the first the server
/// has not refused).
pub fn plan(view: &BoardView) -> Vec<Move> {
    let mut moves = Vec::new();
    if !view.running() {
        return moves;
    }
    let coh = view.virus_coherence;

    // 2. Win if you can.
    let core = view.core();
    if let Some(core) = core {
        let t = view.tiles[&core];
        let rotted = if view.slot_of(KERNEL_ROT).is_some() {
            t.coherence - t.coherence / 2
        } else {
            t.coherence
        };
        if view.cost(core, Some(rotted)) < coh {
            moves.extend(attack(view, core, true));
        }
    }

    // 3. Survive.
    if coh < LOW_COHERENCE
        && let Some(i) = view.slot_of(SELF_REPAIR)
    {
        moves.push(Move::UseOnVirus(i));
    }

    // 4. Utilities and caches, when a slot is free.
    if view.free_slot() {
        for (c, t) in &view.tiles {
            if !t.hidden && t.kind == UTILITY_TILE {
                moves.push(Move::Click(*c));
            }
        }
        for (c, t) in &view.tiles {
            if !t.hidden && t.kind == CACHE {
                moves.push(Move::Click(*c));
            }
        }
    }

    // 5. Reveal, or unblock.
    let cands = view.candidates();
    let cells = view.cells();
    let expected = |c: Coord| -> f64 {
        if cands.is_empty() {
            return 0.0;
        }
        let dist = bfs(&cells, c);
        cands
            .iter()
            .map(|k| dist.get(k).map_or(f64::from(u16::MAX), |&d| f64::from(d)))
            .sum::<f64>()
            / cands.len() as f64
    };
    let hidden_around = |c: Coord| {
        engine::neighbours(c)
            .iter()
            .filter(|n| view.tile(**n).is_some_and(|t| t.hidden))
            .count()
    };
    let mut free: Vec<(f64, usize, Coord)> = view
        .tiles
        .iter()
        .filter(|(c, t)| t.hidden && !t.blocked && view.next_to_passable(**c))
        .map(|(c, _)| (expected(*c), hidden_around(*c), *c))
        .collect();
    free.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    // Blocked frontier cells: (cost of every blocker, expected distance, cell).
    let mut blocked: Vec<(i64, f64, Coord)> = view
        .tiles
        .iter()
        .filter(|(c, t)| t.hidden && t.blocked && view.next_to_passable(**c))
        .map(|(c, _)| {
            let cost = blockers(view, *c)
                .iter()
                .map(|b| view.cost(*b, None))
                .fold(0_i64, i64::saturating_add);
            (cost, expected(*c), *c)
        })
        .collect();
    blocked.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
    let early = free.first().and_then(|best| {
        blocked
            .iter()
            .filter(|(cost, d, _)| {
                (*cost as f64) < coh as f64 * UNBLOCK_SHARE && *d + UNBLOCK_GAIN <= best.0
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .copied()
    });
    if let Some((_, _, cell)) = early {
        moves.extend(unblock(view, cell));
    }
    moves.extend(free.iter().map(|(_, _, c)| Move::Click(*c)));
    for (cost, _, cell) in &blocked {
        if *cost < coh {
            moves.extend(unblock(view, *cell));
        }
    }

    // A core out of reach: every tool on it, then the attack anyway if the
    // virus outlives the next counter-hit.
    if let Some(core) = core {
        if let Some(i) = view.slot_of(KERNEL_ROT) {
            moves.push(Move::UseOnTile(i, core));
        }
        if let Some(i) = view.slot_of(SECONDARY_VECTOR) {
            moves.push(Move::UseOnTile(i, core));
        }
        if let Some(i) = view.slot_of(SELF_REPAIR) {
            moves.push(Move::UseOnVirus(i));
        }
        if view.shield_left() == 0
            && let Some(i) = view.slot_of(SHIELD)
        {
            moves.push(Move::UseOnVirus(i));
        }
        if !fatal(view, core) {
            moves.push(Move::Click(core));
        }
    }

    // 6. Otherwise the cheapest attack that does not kill the virus, then
    //    any cache (it may open into a segment).
    let mut targets: Vec<(i64, Coord)> = view
        .tiles
        .iter()
        .filter(|(c, _)| view.live_defense(**c))
        .map(|(c, _)| (view.cost(*c, None), *c))
        .filter(|(cost, _)| *cost < coh)
        .collect();
    targets.sort();
    for (_, c) in targets {
        moves.extend(attack(view, c, false));
    }
    for (c, t) in &view.tiles {
        if !t.hidden && t.kind == CACHE {
            moves.push(Move::Click(*c));
        }
    }
    moves
}

/// The live defenses next to a blocked cell, in the order to kill them:
/// Restoration Nodes, then Suppressors, then the cheapest.
fn blockers(view: &BoardView, cell: Coord) -> Vec<Coord> {
    let mut b: Vec<(u8, i64, Coord)> = engine::neighbours(cell)
        .iter()
        .filter(|n| view.live_defense(**n))
        .map(|&n| {
            let rank = match view.tiles[&n].subtype {
                RESTORATION => 0,
                SUPPRESSOR => 1,
                _ => 2,
            };
            (rank, view.cost(n, None), n)
        })
        .collect();
    b.sort();
    b.into_iter().map(|(_, _, c)| c).collect()
}

fn unblock(view: &BoardView, cell: Coord) -> Vec<Move> {
    blockers(view, cell)
        .first()
        .map(|&b| attack(view, b, false))
        .unwrap_or_default()
}

/// The next hit on `c` leaves the virus dead (the node survives it and
/// its counter-hit is the virus's coherence or more, unshielded).
fn fatal(view: &BoardView, c: Coord) -> bool {
    let Some(t) = view.tile(c) else { return true };
    t.coherence > view.virus_strength.max(1)
        && view.shield_left() == 0
        && t.strength >= view.virus_coherence
}

/// **The moves of one attack on `c`**, best first: on the core, Kernel Rot
/// on a fat one and a Secondary Vector; the shield before a hard-hitting
/// node that survives the hit; then the click. On a defense the two target
/// utilities are spent only when the kill is out of reach without them.
fn attack(view: &BoardView, c: Coord, is_core: bool) -> Vec<Move> {
    let Some(t) = view.tile(c).copied() else {
        return Vec::new();
    };
    let s = view.virus_strength.max(1);
    let coh = view.virus_coherence;
    let mut moves = Vec::new();
    let tools = is_core || view.cost(c, None) >= coh;
    if tools {
        if t.coherence > 2 * s
            && let Some(i) = view.slot_of(KERNEL_ROT)
        {
            moves.push(Move::UseOnTile(i, c));
        }
        if t.coherence > s
            && let Some(i) = view.slot_of(SECONDARY_VECTOR)
        {
            moves.push(Move::UseOnTile(i, c));
        }
    }
    if t.coherence > s
        && view.shield_left() == 0
        && t.strength as f64 >= coh as f64 * SHIELD_SHARE
        && let Some(i) = view.slot_of(SHIELD)
    {
        moves.push(Move::UseOnVirus(i));
    }
    if !fatal(view, c) {
        moves.push(Move::Click(c));
    }
    moves
}

// ---- the wire ------------------------------------------------------------------------

/// The `hackingMgr` call of `mv` [V `hackingUISvc`]: method and arguments.
pub fn call_of(mv: Move) -> (&'static str, Vec<PyValue>) {
    match mv {
        Move::Click(c) => ("ClickedOnTile", vec![coord_value(c)]),
        Move::UseOnTile(i, c) => (
            "UsedUtilityElement",
            vec![PyValue::Int(i as i64), coord_value(c)],
        ),
        Move::UseOnVirus(i) => ("UsedUtilityElementOnVirus", vec![PyValue::Int(i as i64)]),
        Move::Quit => ("QuitHackingAttempt", vec![]),
    }
}

/// The events of an `OnHackingTurnComplete` push, if `(name, args)` is one.
pub fn turn_events(name: &str, args: &[PyValue]) -> Option<Vec<PyValue>> {
    if name != "OnHackingTurnComplete" {
        return None;
    }
    match args.first() {
        Some(PyValue::List(events)) => Some(events.clone()),
        _ => None,
    }
}

/// **Send one input** and fold its turn into `view`: the push rides with
/// the answer [HK2]; one not yet read is waited for ([`TURN_WAIT`]).
pub fn send(c: &mut Conn, view: &mut BoardView, mv: Move) -> Result<()> {
    let (method, args) = call_of(mv);
    view.sent(mv);
    let mut outcome = c.call("hackingMgr", method, args)?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!("hackingMgr.{method}: {key}")));
    }
    let mut turns: Vec<Vec<PyValue>> = outcome
        .notifications
        .iter()
        .filter_map(crate::notification)
        .filter_map(|(n, a)| turn_events(&n, &a))
        .collect();
    if turns.is_empty() {
        let got = c.await_push(TURN_WAIT, &mut outcome.notifications, |n, a| {
            turn_events(n, a).is_some()
        })?;
        turns.extend(got.and_then(|(n, a)| turn_events(&n, &a)));
    }
    if turns.is_empty() {
        return Err(Error::Refused(format!(
            "hackingMgr.{method}: no OnHackingTurnComplete"
        )));
    }
    for t in turns {
        view.apply_turn(&t);
    }
    Ok(())
}

/// Wait up to `within` for the start batch (`GAME_START`) and fold it in.
pub fn await_start(c: &mut Conn, view: &mut BoardView, within: Duration) -> Result<bool> {
    let deadline = Instant::now() + within;
    let mut sink = Vec::new();
    while Instant::now() < deadline && !view.started {
        let left = deadline.saturating_duration_since(Instant::now());
        let Some((n, a)) = c.await_push(left, &mut sink, |n, a| turn_events(n, a).is_some())?
        else {
            break;
        };
        if let Some(events) = turn_events(&n, &a) {
            view.apply_turn(&events);
        }
    }
    Ok(view.started)
}

/// **Play the started game to its end** by [`next`]: quit after
/// [`MAX_TURNS`] inputs. Returns the outcome.
pub fn play(c: &mut Conn, view: &mut BoardView) -> Result<Outcome> {
    let mut inputs = 0;
    while view.running() {
        let mv = if inputs >= MAX_TURNS {
            Move::Quit
        } else {
            next(view)
        };
        inputs += 1;
        send(c, view, mv)?;
        if mv == Move::Quit && view.ended.is_none() {
            view.ended = Some(Outcome::Lost);
        }
    }
    Ok(view.ended.unwrap_or(Outcome::Lost))
}

// ---- the role --------------------------------------------------------------------------

/// Finds data and relic sites and hacks their cans. A civilian.
pub static ROLE: RoleSpec = RoleSpec {
    name: "hack",
    greeting: "o7 -- hacking cans, not fighting",
    civilian: true,
    docked,
    in_space,
    banner,
    setup,
};

/// What the closed economy buys for the hacker (`acquire::kit_of`, by the
/// coordinator's row) **[A]**: a Heron, a Data Analyzer I and core probes,
/// and beside them a Relic Analyzer I and a core probe launcher; Hacking
/// and Archaeology. Falsified by hackers that idle with a hull and no way
/// to scan or open a relic can.
pub const KIT: crate::acquire::Kit = crate::acquire::Kit {
    hull: Some(HULL),
    modules: Some((DATA_ANALYZER_I, 1)),
    ammo: Some((crate::probe::CORE_PROBE, crate::probe::SETUP_PROBES)),
    skills: &[(SKILL_HACKING, 1), (SKILL_ARCHAEOLOGY, 1)],
    more: &[(RELIC_ANALYZER_I, 1), (crate::probe::CORE_LAUNCHER, 1)],
};

fn setup(lacks: Lacks) -> Vec<String> {
    let mut lines = vec![
        format!("/giveskill {SKILL_HACKING} 1"),
        format!("/giveskill {SKILL_ARCHAEOLOGY} 1"),
    ];
    if lacks.ship {
        lines.push(format!("/createitem {HULL}"));
    }
    if lacks.module {
        lines.push(format!("/createitem {DATA_ANALYZER_I}"));
        lines.push(format!("/createitem {RELIC_ANALYZER_I}"));
    }
    if lacks.module || lacks.ammo {
        lines.push(format!("/createitem {}", crate::probe::CORE_LAUNCHER));
        lines.push(format!(
            "/createitem {} {}",
            crate::probe::CORE_PROBE,
            crate::probe::SETUP_PROBES
        ));
    }
    lines
}

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, sold {:.2} ISK, reconnects {}",
        s.name, s.phase, s.isk_sold, s.reconnects
    )
}

/// What the last [`run`] of a bot left for its next docked turn, by bot
/// name (per process, like the huffer's).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Note {
    /// The last run had no analyzer to fit.
    pub lacks_analyzer: bool,
    /// The last run found no site: dwell the longer dwell.
    pub found_nothing: bool,
    /// The last run had no probe launcher or probes.
    pub lacks_probing: bool,
    /// Types looted so far: what the sale offers.
    pub looted: BTreeSet<i64>,
}

static NOTES: Mutex<BTreeMap<String, Note>> = Mutex::new(BTreeMap::new());

/// The note of bot `name`.
pub fn note_of(name: &str) -> Note {
    NOTES
        .lock()
        .map(|n| n.get(name).cloned().unwrap_or_default())
        .unwrap_or_default()
}

fn set_note(name: &str, note: Note) {
    if let Ok(mut n) = NOTES.lock() {
        n.insert(name.to_owned(), note);
    }
}

/// `(server, system, target id)` of the sites this process's hackers
/// emptied: the next hacker skips them until they respawn under a new id.
pub static DONE: Mutex<BTreeSet<(SocketAddr, i64, String)>> = Mutex::new(BTreeSet::new());

fn done(server: SocketAddr, system: i64, target: &str) -> bool {
    DONE.lock()
        .map(|d| d.contains(&(server, system, target.to_owned())))
        .unwrap_or(false)
}

fn mark_done(server: SocketAddr, system: i64, target: &str) {
    if let Ok(mut d) = DONE.lock() {
        d.insert((server, system, target.to_owned()));
    }
}

/// Is `row` an analyzer?
pub fn is_analyzer(row: &ItemRow) -> bool {
    row.group_id == GROUP_ANALYZER
}

/// Docked: unload the hold's loot, a hull, the analyzer or probes the last
/// run lacked, then one [`Act::Hack`] and a dwell.
fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if o.failures.values().any(|&n| n >= brain::GIVE_UP_AFTER) {
        return vec![Act::Reconnect];
    }
    m.claim = None;
    m.homing = None;
    let active = o.ship.map(|s| s.ship_id);
    let flying = o.ship.and_then(|s| s.type_id);
    let in_capsule = flying.is_none_or(|t| t == brain::CAPSULE);
    let note = note_of(&m.name);
    let analyzer = o.hangar.iter().any(is_analyzer);
    if in_capsule {
        if let Some(hull) = o
            .hangar
            .iter()
            .find(|r| {
                r.type_id == HULL
                    && Some(r.item_id) != active
                    && !m.combat.untrained.contains(&r.type_id)
            })
            .or_else(|| {
                o.hangar.iter().find(|r| {
                    r.category_id == crate::acquire::CATEGORY_SHIP
                        && r.type_id != brain::CAPSULE
                        && Some(r.item_id) != active
                        && !m.combat.untrained.contains(&r.type_id)
                })
            })
        {
            return vec![Act::Board(hull.item_id)];
        }
        return brain::gm_or_missing(m, true, !analyzer, false, "a hull in the hangar");
    }
    if note.lacks_analyzer && !analyzer {
        return brain::gm_or_missing(m, false, true, false, "a data analyzer in the hangar");
    }
    if note.lacks_probing {
        set_note(
            &m.name,
            Note {
                lacks_probing: false,
                ..note.clone()
            },
        );
        return brain::gm_or_missing(
            m,
            false,
            false,
            true,
            "a probe launcher and probes in the hangar",
        );
    }
    let rest = m.jitter(if note.found_nothing {
        m.policy.idle_dwell
    } else {
        m.policy.dwell
    });
    vec![Act::Hack, Act::Wait(rest)]
}

/// In space outside a run (a dropped connection mid-trip): carry on the
/// trip the state file holds ([`roam::resumable`]) -- until it has failed
/// [`brain::GIVE_UP_AFTER`] times, when it is abandoned -- else home.
fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if roam::resumable(&m.name) {
        if o.failed(crate::brain::ActKind::Hack) < brain::GIVE_UP_AFTER {
            return vec![Act::Hack];
        }
        roam::abandon(&m.name);
    }
    match &o.park {
        Some(park) => brain::go_home(m, o, park),
        None => vec![Act::Wait(brain::SHORT_WAIT)],
    }
}

// ---- the sale, pure --------------------------------------------------------------

/// What to do with one loot stack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sale {
    /// Into the best bid, whoever's, at its price.
    Bid(SellLine),
    /// Listed for [`crate::escort::LIST_DAYS`] at the seller's ask.
    List(SellLine),
    /// A datacore or decryptor, listed at the seller's ask for the builder
    /// in the hacker's own corporation to buy, not dumped into a bid
    /// ([`sale_plan_for`], Y-ROAM).
    Offer(SellLine),
    /// Nobody bids and the server quotes no price: kept.
    Keep { type_id: i64, quantity: i64 },
}

/// Is `row` loot this hacker sells: a stack (never a fitted or packaged
/// module of the kit) of a type it looted or of [`LOOT_TYPES`].
pub fn is_loot(row: &ItemRow, looted: &BTreeSet<i64>) -> bool {
    !row.singleton
        && row.quantity > 0
        && !is_analyzer(row)
        && row.group_id != crate::probe::GROUP_LAUNCHER
        && row.group_id != crate::probe::GROUP_PROBE
        && row.category_id != crate::acquire::CATEGORY_SHIP
        && (looted.contains(&row.type_id) || LOOT_TYPES.contains(&row.type_id))
}

/// **The hacker's sale** of `stacks` at `station` **[A]**: a bid of at
/// least one [`pricing::TICK`] takes the stack; else a `P_ref` lists it at
/// [`pricing::ask`] (the builders buy datacores from asks); else it is
/// kept.
pub fn sale_plan(
    stacks: &[ItemRow],
    looted: &BTreeSet<i64>,
    books: &BTreeMap<i64, Book>,
    p_refs: &BTreeMap<i64, f64>,
    station: i64,
) -> Vec<Sale> {
    sale_plan_for(stacks, looted, books, p_refs, station, false)
}

/// [`sale_plan`], and with `builder` -- a builder in the hacker's own
/// corporation ([`roam::builder_in_corp`]) -- datacores and decryptors
/// ([`roam::is_builder_input`]) are never sold into a bid: they are
/// **offered** at the ask, where the builder's invention buys them
/// (`builder::buy_datacores` reads asks), or kept when the server quotes no
/// price **[A]**: there is no hand-to-hand trade between characters, so
/// the market is the corporation's counter. Falsified by a builder that
/// buys the stack from somebody else's ask, or a hacker whose offers sit
/// unsold while its corp's builder idles for want of datacores.
pub fn sale_plan_for(
    stacks: &[ItemRow],
    looted: &BTreeSet<i64>,
    books: &BTreeMap<i64, Book>,
    p_refs: &BTreeMap<i64, f64>,
    station: i64,
    builder: bool,
) -> Vec<Sale> {
    stacks
        .iter()
        .filter(|r| is_loot(r, looted))
        .map(|r| {
            let line = |price: f64| SellLine {
                station_id: station,
                type_id: r.type_id,
                item_id: r.item_id,
                price,
                quantity: r.quantity,
            };
            let ask = p_refs
                .get(&r.type_id)
                .and_then(|&p| pricing::ask(&Quote::at_ref(p)));
            if builder && roam::is_builder_input(r) {
                return match ask {
                    Some(a) => Sale::Offer(line(a)),
                    None => Sale::Keep {
                        type_id: r.type_id,
                        quantity: r.quantity,
                    },
                };
            }
            let bid = books
                .get(&r.type_id)
                .and_then(|b| b.best_bid())
                .map(|b| b.price)
                .filter(|&p| p >= pricing::TICK);
            if let Some(p) = bid {
                return Sale::Bid(line(p));
            }
            match ask {
                Some(ask) => Sale::List(line(ask)),
                None => Sale::Keep {
                    type_id: r.type_id,
                    quantity: r.quantity,
                },
            }
        })
        .collect()
}

// ---- the run ---------------------------------------------------------------------

/// Why a run could not go out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lack {
    Hull,
    Analyzer,
}

/// One can's game.
#[derive(Debug, Clone, PartialEq)]
pub struct Played {
    pub can: i64,
    /// The can's type, from its slim.
    pub type_id: Option<i64>,
    pub won: bool,
    pub turns: u32,
    /// The virus's coherence at the end.
    pub coherence: i64,
    /// The core's subtype, once revealed (`CORE_LOW` .. `CORE_HIGH`: the
    /// can's tier as the board shows it).
    pub core: Option<i64>,
    /// Units looted from the won can.
    pub looted: i64,
}

impl Played {
    /// The `[hack]` line.
    pub fn line(&self) -> String {
        format!(
            "[hack] can {} (type {:?}, core {:?}): {} in {} turns, coherence {} left, {} units looted",
            self.can,
            self.type_id,
            self.core,
            if self.won { "won" } else { "lost" },
            self.turns,
            self.coherence,
            self.looted
        )
    }
}

/// What one [`run`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trip {
    pub lacking: Option<Lack>,
    /// What a lacking run waits on (the skills a fit or an online asks for).
    pub waits: Option<String>,
    pub sales: Vec<Sale>,
    /// Target ids of the sites landed on.
    pub sites: Vec<String>,
    pub games: Vec<Played>,
    /// Loot units brought home.
    pub units: i64,
    /// The trip ended on a full hold.
    pub full: bool,
    pub before: f64,
    pub after: f64,
    /// The systems the trip visited besides its home, in order (Y-ROAM).
    pub route: Vec<i64>,
    /// Systems it reached and left because of what it saw there.
    pub avoided: Vec<(i64, roam::Threat)>,
    /// Why the trip turned for home before its loop was done.
    pub turned: Option<roam::Turn>,
    /// The station the hold was sold at when it was not home.
    pub outlet: Option<i64>,
    /// The trip carried on one a dropped connection had cut off.
    pub resumed: bool,
    /// The bot was found in space with no trip, and flown home.
    pub homed: bool,
}

impl Trip {
    /// The act line's note: the sale, the sites, one `[hack]` line a game.
    pub fn summary(&self) -> String {
        if let Some(l) = self.lacking {
            return match &self.waits {
                Some(w) => format!("did not go out: no {l:?} ({w})"),
                None => format!("did not go out: no {l:?}"),
            };
        }
        if self.homed {
            return "found in space with no trip: flown home".to_owned();
        }
        let (bids, lists, kept) = self.sales.iter().fold((0, 0, 0), |(b, l, k), s| match s {
            Sale::Bid(_) => (b + 1, l, k),
            Sale::List(_) | Sale::Offer(_) => (b, l + 1, k),
            Sale::Keep { .. } => (b, l, k + 1),
        });
        let won = self.games.iter().filter(|g| g.won).count();
        let mut s = format!(
            "{}sold {bids} into bids, listed {lists}, kept {kept}; sites {:?}, cans won {won} of {}, {} loot units home{}; wallet {:.2} -> {:.2} ISK",
            if self.resumed { "[resumed] " } else { "" },
            self.sites,
            self.games.len(),
            self.units,
            if self.full { " (hold full)" } else { "" },
            self.before,
            self.after
        );
        if !self.route.is_empty() {
            s.push_str(&format!("; [roam] route {:?}", self.route));
        }
        for (system, threat) in &self.avoided {
            s.push_str(&format!("; [roam] left {system}: {threat:?}"));
        }
        if let Some(t) = self.turned {
            s.push_str(&format!("; [roam] turned for home: {t:?}"));
        }
        if let Some(o) = self.outlet {
            s.push_str(&format!("; [roam] sold at station {o}"));
        }
        if self.sites.is_empty() {
            s.push_str(&format!("; MISSING {MISSING_SITES}"));
        }
        for g in &self.games {
            s.push_str("; ");
            s.push_str(&g.line());
        }
        s
    }
}

/// **One hacking trip, dock to dock** (module header), at home: the HK4
/// trip, no roaming. `name` is the bot's (its [`Note`]) and `seed` its
/// spread. Docked to start; docked at the same station at the end, unless
/// an error cut it short.
pub fn run(c: &mut Conn, name: &str, seed: u64) -> Result<Trip> {
    run_with(c, name, seed, &roam::Ctx::default())
}

/// **One hacking trip, roaming when `ctx` has a map** (Y-ROAM, `docs/lanes/
/// y-roam.md`): the home system first, then a loop over nearby systems
/// chosen by the bot's nerve ([`roam::plan_tour`]) -- scan, hack what can
/// be hacked, book what cannot -- home, or to the outlet that pays most
/// ([`roam::rank_outlets`]), to sell. The trip's state is kept as it goes
/// ([`roam::put`]); a bot found with one in space or in a foreign station
/// carries on from it, and a bot found in space with none is flown home.
pub fn run_with(c: &mut Conn, name: &str, seed: u64, ctx: &roam::Ctx<'_>) -> Result<Trip> {
    let attached = roam::attached(name);
    let dir = ctx.state.or(attached.as_deref());
    if let Some(st) = roam::get(dir, name, roam::unix_now()) {
        return carry_on(c, name, seed, ctx, st);
    }
    let Some(home) = c.session().station_id() else {
        return home_from_space(c, name, ctx);
    };
    let home_system = c
        .session()
        .solar_system_id2()
        .ok_or_else(|| Error::NotSent("hack: docked with no system".to_owned()))?;
    let mut note = note_of(name);
    let mut trip = Trip::default();
    let mut waits = None;
    let analyzers = match ready(c, &mut waits)? {
        Ok(a) => a,
        Err(lack) => {
            trip.lacking = Some(lack);
            trip.waits = waits;
            note.lacks_analyzer = lack == Lack::Analyzer;
            set_note(name, note);
            return Ok(trip);
        }
    };
    let lacks_probing = match crate::probe::outfit(c) {
        Ok(_) => false,
        Err(Error::NotSent(_) | Error::Refused(_)) => true,
        Err(e) => return Err(e),
    };
    trip.before = c.cash_balance()?;
    trip.sales = sell(c, home, &note.looted, ctx.builder_in_corp)?;
    trip.after = c.cash_balance()?;

    leave(c)?;
    let server = c.socket()?.peer_addr()?;
    let mut st = roam::TripState::new(name, home, home_system, roam::unix_now(), trip.before);
    st.looted = note.looted.clone();
    let mut ride = Ride {
        name,
        seed,
        ctx,
        dir,
        server,
        started: Instant::now(),
        analyzers,
        trip,
        note,
        st,
        prices: None,
    };
    ride.note.lacks_probing = lacks_probing;
    ride.fresh(c)?;
    ride.drive(c)?;
    Ok(ride.trip)
}

/// A way out of J-space: from the arrival point, `Ok(false)` if none.
pub type ExitFn<'a> = dyn Fn(&mut Conn, Option<[f64; 3]>) -> Result<bool> + 'a;

/// What a trip into J-space does that a home trip does not (T3-SALVAGE,
/// `docs/lanes/t3-salvage.md`).
pub struct Dive<'a> {
    /// Dive in; the arrival point when the bot is in J-space, `None` when
    /// no hole took it (the trip then flies home).
    pub enter: &'a dyn Fn(&mut Conn) -> Result<Option<[f64; 3]>>,
    /// Come out again, by the hole nearest the arrival; `Ok(false)`: none
    /// found.
    pub exit: &'a ExitFn<'a>,
    /// May the site `(system, target)` be worked?
    pub allowed: &'a dyn Fn(i64, &str) -> bool,
}

/// **One hacking trip into J-space, dock to dock**: the HK4 trip's ready,
/// sale and finish, with the sites [`Dive::allowed`] only, worked after
/// [`Dive::enter`] and before [`Dive::exit`]. No trip state is kept (a
/// crash in J-space leaves the bot where it is).
pub fn run_dive(c: &mut Conn, name: &str, seed: u64, dive: &Dive<'_>) -> Result<Trip> {
    let ctx = roam::Ctx::default();
    let Some(home) = c.session().station_id() else {
        return Err(Error::NotSent("hack dive: not docked".to_owned()));
    };
    let home_system = c
        .session()
        .solar_system_id2()
        .ok_or_else(|| Error::NotSent("hack dive: docked with no system".to_owned()))?;
    let mut note = note_of(name);
    let mut trip = Trip::default();
    let mut waits = None;
    let analyzers = match ready(c, &mut waits)? {
        Ok(a) => a,
        Err(lack) => {
            trip.lacking = Some(lack);
            trip.waits = waits;
            note.lacks_analyzer = lack == Lack::Analyzer;
            set_note(name, note);
            return Ok(trip);
        }
    };
    let lacks_probing = match crate::probe::outfit(c) {
        Ok(_) => false,
        Err(Error::NotSent(_) | Error::Refused(_)) => true,
        Err(e) => return Err(e),
    };
    trip.before = c.cash_balance()?;
    trip.sales = sell(c, home, &note.looted, false)?;
    trip.after = c.cash_balance()?;
    leave(c)?;
    let server = c.socket()?.peer_addr()?;
    let st = roam::TripState::new(name, home, home_system, roam::unix_now(), trip.before);
    let mut ride = Ride {
        name,
        seed,
        ctx: &ctx,
        dir: None,
        server,
        started: Instant::now(),
        analyzers,
        trip,
        note,
        st,
        prices: None,
    };
    ride.note.lacks_probing = lacks_probing;
    let arrival = (dive.enter)(c)?;
    if arrival.is_some() {
        let system = c.session().solar_system_id().unwrap_or_default();
        while !ride.trip.full
            && ride.started.elapsed() < TRIP_CEILING
            && ride.trip.sites.len() < MAX_SITES
        {
            let tried = ride.trip.sites.clone();
            let Some(sig) = find_site_if(c, server, &tried, dive.allowed)? else {
                break;
            };
            ride.trip.sites.push(sig.target_id.clone());
            settle_for(c, SITE_SETTLE)?;
            let out = hack_site(c, &ride.analyzers, sig.kind, seed, ride.started)?;
            ride.trip.full = out.full;
            ride.note.looted.extend(out.types);
            ride.trip.games.extend(out.games);
            if !out.full {
                mark_done(server, system, &sig.target_id);
            }
        }
        if !(dive.exit)(c, arrival)? {
            return Err(Error::Refused(
                "hack dive: no hole out of J-space was found".to_owned(),
            ));
        }
    }
    ride.st.phase = roam::Phase::Back;
    ride.finish(c)?;
    Ok(ride.trip)
}

/// **A bot in space with no trip** (a crash before the first save, an
/// abandoned trip): home, over the gates when there is a map.
fn home_from_space(c: &mut Conn, name: &str, ctx: &roam::Ctx<'_>) -> Result<Trip> {
    let home = ctx
        .home
        .ok_or_else(|| Error::NotSent("hack: not docked".to_owned()))?;
    let system = c.session().solar_system_id2();
    let home_system = ctx.trips.and_then(|t| t.map.system_of(home)).or(system);
    if system == home_system {
        bind(c)?;
        fly_home(c, home)?;
    } else {
        let trips = ctx
            .trips
            .ok_or_else(|| Error::NotSent("hack: not docked, and no map home".to_owned()))?;
        go_station(c, trips, home)?;
    }
    let _ = name;
    Ok(Trip {
        homed: true,
        ..Trip::default()
    })
}

/// `travel::go` to `station`: at the trip's floor, else at any security
/// (the way home is the way out).
fn go_station(c: &mut Conn, trips: crate::travel::Trips<'_>, station: i64) -> Result<()> {
    match crate::travel::go(c, trips, station) {
        Ok(_) => Ok(()),
        Err(Error::NotSent(_)) => {
            let any = crate::travel::Trips {
                map: trips.map,
                floor: crate::travel::ANY_SECURITY,
            };
            crate::travel::go(c, any, station).map(|_| ())
        }
        Err(e) => Err(e),
    }
}

/// In space with the ballpark bound and a state.
fn bind(c: &mut Conn) -> Result<()> {
    if c.session().station_id().is_some() {
        return Ok(());
    }
    let system = c
        .session()
        .solar_system_id()
        .ok_or_else(|| Error::NotSent("hack: in space with no system".to_owned()))?;
    if c.ballpark().is_none() {
        c.bind_ballpark(system)?;
    }
    settle(c, |c| c.world().has_state())
}

/// The analyzers fitted, as `(module id, type id)` (docked or not).
fn fitted_analyzers(c: &mut Conn) -> Result<Vec<(i64, i64)>> {
    Ok(c.fitted()?
        .into_iter()
        .filter(is_analyzer)
        .map(|r| (r.item_id, r.type_id))
        .collect())
}

/// **Carry on a trip** a dropped connection cut off (the stored `st`).
fn carry_on(
    c: &mut Conn,
    name: &str,
    seed: u64,
    ctx: &roam::Ctx<'_>,
    st: roam::TripState,
) -> Result<Trip> {
    let server = c.socket()?.peer_addr()?;
    for (system, target) in &st.sites {
        mark_done(server, *system, target);
    }
    let mut note = note_of(name);
    note.looted.extend(st.looted.iter().copied());
    let analyzers = fitted_analyzers(c)?;
    let elapsed = Duration::from_secs(roam::unix_now().saturating_sub(st.started));
    let attached = roam::attached(name);
    let dir = ctx.state.or(attached.as_deref());
    let trip = Trip {
        sites: st.sites.iter().map(|(_, t)| t.clone()).collect(),
        units: st.units,
        before: st.before,
        resumed: true,
        ..Trip::default()
    };
    let mut ride = Ride {
        name,
        seed,
        ctx,
        dir,
        server,
        started: Instant::now()
            .checked_sub(elapsed)
            .unwrap_or_else(Instant::now),
        analyzers,
        trip,
        note,
        st,
        prices: None,
    };
    // Docked: at home the trip is over bar the unloading and the sale;
    // anywhere else it goes on from the station.
    if let Some(station) = c.session().station_id() {
        if station != ride.st.home_station
            && ride.st.phase == roam::Phase::Out
            && !ride.analyzers.is_empty()
        {
            leave(c)?;
        } else {
            ride.st.phase = roam::Phase::Back;
        }
    } else {
        bind(c)?;
    }
    if ride.analyzers.is_empty() {
        ride.st.phase = roam::Phase::Back;
    }
    ride.drive(c)?;
    Ok(ride.trip)
}

/// One trip under way: everything [`run_with`] and [`carry_on`] share.
struct Ride<'a, 'b> {
    name: &'a str,
    seed: u64,
    ctx: &'a roam::Ctx<'b>,
    dir: Option<&'a Path>,
    server: SocketAddr,
    started: Instant,
    analyzers: Vec<(i64, i64)>,
    trip: Trip,
    note: Note,
    st: roam::TripState,
    /// The server's reference prices, read once a trip when needed.
    prices: Option<pricing::Prices>,
}

impl Ride<'_, '_> {
    fn save(&mut self) {
        self.st.looted = self.note.looted.clone();
        self.st.units = self.trip.units;
        roam::put(self.dir, &self.st);
    }

    fn system(c: &Conn) -> Option<i64> {
        c.session().solar_system_id2()
    }

    /// The home system first, then the loop planned; the state is saved
    /// before the bot leaves the station's grid.
    fn fresh(&mut self, c: &mut Conn) -> Result<()> {
        self.save();
        let home_system = self.st.home_system;
        let sites = if self.ctx.roams() {
            roam::SITES_PER_SYSTEM
        } else {
            MAX_SITES
        };
        self.work(c, home_system, sites)?;
        if self.ctx.roams() && !self.trip.full {
            self.plan(c);
        }
        self.save();
        Ok(())
    }

    /// Plan the loop from here by [`roam::plan_tour`].
    fn plan(&mut self, c: &mut Conn) {
        let (Some(trips), Some(here)) = (self.ctx.trips, Self::system(c)) else {
            return;
        };
        let now = Instant::now();
        let server = self.server;
        self.st.stops = roam::plan_tour(
            trips.map,
            self.ctx.nerve(),
            self.st.home_system,
            here,
            |s| {
                crate::probe::intel_of(&crate::probe::LEDGER, server, s, now, |t| {
                    done(server, s, t)
                })
            },
        );
    }

    /// Work the system the bot is in: scan, hack what can be hacked
    /// ([`roam::job_for`]), book what cannot, until `sites` are hacked, the
    /// system is dry, the hold is full or the trip is late.
    fn work(&mut self, c: &mut Conn, system: i64, sites: usize) -> Result<()> {
        if !self.st.visited.contains(&system) {
            self.st.visited.push(system);
        }
        let mut hacked = 0;
        while hacked < sites
            && !self.trip.full
            && self.started.elapsed() < roam_ceiling(self.ctx)
            && self.trip.sites.len() < site_cap(self.ctx)
        {
            let tried = self.trip.sites.clone();
            let Some(sig) = find_site(c, self.server, &tried)? else {
                break;
            };
            hacked += 1;
            crate::probe::LEDGER.record(
                (self.server, sig.solar_system_id),
                &sig,
                c.session().char_id().unwrap_or_default(),
                None,
                Instant::now(),
            );
            self.trip.sites.push(sig.target_id.clone());
            settle_for(c, SITE_SETTLE)?;
            let out = hack_site(c, &self.analyzers, sig.kind, self.seed, self.started)?;
            self.trip.full = out.full;
            self.note.looted.extend(out.types);
            self.trip.games.extend(out.games);
            if !out.full {
                mark_done(self.server, sig.solar_system_id, &sig.target_id);
            }
            self.st
                .sites
                .push((sig.solar_system_id, sig.target_id.clone()));
            self.save();
        }
        Ok(())
    }

    /// The loot in the hold, at the server's reference prices.
    fn hold_value(&mut self, c: &mut Conn) -> Result<f64> {
        if self.prices.is_none() {
            self.prices = Some(pricing::Prices::read(c)?);
        }
        let refs = self.prices.as_ref().map(pricing::Prices::refs);
        let cargo = c.cargo()?;
        Ok(cargo
            .iter()
            .filter(|r| is_loot(r, &self.note.looted))
            .map(|r| {
                r.quantity as f64 * refs.and_then(|m| m.get(&r.type_id)).copied().unwrap_or(0.0)
            })
            .sum())
    }

    /// Arrived in `system` (a stop, or where a resumed trip found the bot):
    /// look, leave if the nerve says so, else work it.
    fn arrive(&mut self, c: &mut Conn, system: i64) -> Result<()> {
        let map = self.ctx.trips.map(|t| t.map);
        let threat = match roam::look(c, map) {
            Ok(t) => t,
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => roam::Threat::Clear,
        };
        if roam::avoid(threat, self.ctx.nerve()) {
            self.trip.avoided.push((system, threat));
            self.st.visited.push(system);
            self.save();
            return Ok(());
        }
        self.work(c, system, roam::SITES_PER_SYSTEM)?;
        self.save();
        Ok(())
    }

    /// Visit the stops; then the way home and the sale.
    fn drive(&mut self, c: &mut Conn) -> Result<()> {
        if self.st.phase == roam::Phase::Out
            && self.ctx.roams()
            && c.session().station_id().is_none()
            && let Some(here) = Self::system(c)
            && !self.st.visited.contains(&here)
        {
            self.arrive(c, here)?;
        }
        while self.st.phase == roam::Phase::Out {
            let value = if self.ctx.roams() {
                self.hold_value(c).unwrap_or(0.0)
            } else {
                0.0
            };
            if let Some(turn) = roam::should_return(
                self.trip.full,
                value,
                self.trip.sites.len(),
                self.started.elapsed(),
            ) {
                self.trip.turned = Some(turn);
                break;
            }
            let (Some(trips), Some(&next)) = (self.ctx.trips, self.st.stops.first()) else {
                break;
            };
            self.st.stops.remove(0);
            self.save();
            let floor = roam::floor_for(self.ctx.nerve());
            if Self::system(c) != Some(next) {
                crate::probe::jump_route(c, trips.map, floor, next)?;
            }
            self.trip.route.push(next);
            self.arrive(c, next)?;
        }
        self.st.phase = roam::Phase::Back;
        self.save();
        self.finish(c)
    }

    /// The hold sold where it fetches most, home, the loot unloaded and
    /// sold, the state cleared and the note kept.
    fn finish(&mut self, c: &mut Conn) -> Result<()> {
        if let Some(outlet) = self.best_outlet(c)? {
            self.go_to(c, outlet)?;
            self.sell_here(c, outlet)?;
            self.trip.outlet = Some(outlet);
        }
        let home = self.st.home_station;
        self.go_to(c, home)?;
        for row in c.cargo()? {
            if is_loot(&row, &self.note.looted) {
                self.trip.units += row.quantity;
                c.unload(row.item_id, None)?;
            }
        }
        let after_sale = sell(c, home, &self.note.looted, self.ctx.builder_in_corp)?;
        self.trip.sales.extend(after_sale);
        self.trip.after = c.cash_balance()?;
        self.note.lacks_analyzer = false;
        self.note.found_nothing = self.trip.sites.is_empty();
        set_note(self.name, self.note.clone());
        roam::clear(self.dir, self.name);
        Ok(())
    }

    /// Fly to `station` and dock: this system's way, or the gates'.
    fn go_to(&mut self, c: &mut Conn, station: i64) -> Result<()> {
        if c.session().station_id() == Some(station) {
            return Ok(());
        }
        let here = Self::system(c);
        let there = self
            .ctx
            .trips
            .and_then(|t| t.map.system_of(station))
            .or((station == self.st.home_station).then_some(self.st.home_system));
        match (self.ctx.trips, there) {
            (Some(trips), Some(there)) if here != Some(there) => {
                let floor = roam::floor_for(self.ctx.nerve());
                go_station(
                    c,
                    crate::travel::Trips {
                        map: trips.map,
                        floor,
                    },
                    station,
                )
            }
            _ => {
                bind(c)?;
                fly_home(c, station)
            }
        }
    }

    /// **The station to sell the hold at** when it is not home: the best
    /// of [`roam::rank_outlets`] over the books of what the hold carries.
    fn best_outlet(&mut self, c: &mut Conn) -> Result<Option<i64>> {
        let (Some(trips), Some(here)) = (self.ctx.trips, Self::system(c)) else {
            return Ok(None);
        };
        let stacks: Vec<(i64, i64)> = c
            .cargo()?
            .iter()
            .filter(|r| is_loot(r, &self.note.looted))
            .map(|r| (r.type_id, r.quantity))
            .collect();
        if stacks.is_empty() {
            return Ok(None);
        }
        if self.prices.is_none() {
            self.prices = Some(pricing::Prices::read(c)?);
        }
        let refs = self
            .prices
            .as_ref()
            .map(|p| p.refs().clone())
            .unwrap_or_default();
        let mut books = BTreeMap::new();
        for &(t, _) in &stacks {
            if let std::collections::btree_map::Entry::Vacant(e) = books.entry(t) {
                e.insert(c.market_orders(t)?);
            }
        }
        let ranked = roam::rank_outlets(
            &stacks,
            &books,
            &refs,
            trips.map,
            roam::floor_for(self.ctx.nerve()),
            here,
            self.st.home_station,
        );
        Ok(ranked
            .first()
            .map(|o| o.station)
            .filter(|&s| s != self.st.home_station))
    }

    /// Docked at an outlet that is not home: unload and sell into its bids
    /// what they take; the rest stays in the hold for home.
    fn sell_here(&mut self, c: &mut Conn, station: i64) -> Result<()> {
        let mut books = BTreeMap::new();
        let stacks: Vec<ItemRow> = c
            .cargo()?
            .into_iter()
            .filter(|r| is_loot(r, &self.note.looted))
            .collect();
        let mut lines = Vec::new();
        for r in &stacks {
            let book = c.market_orders(r.type_id)?;
            if let Some(f) = roam::bid_fill(&book, station, r.quantity) {
                c.unload(r.item_id, Some(f.units))?;
                self.trip.units += f.units;
                // The hangar stack the units landed in.
                lines.push((r.type_id, f));
            }
            books.insert(r.type_id, book);
        }
        let hangar = c.hangar_items()?;
        let sell_lines: Vec<SellLine> = lines
            .iter()
            .filter_map(|(t, f)| {
                let row = hangar
                    .iter()
                    .filter(|h| h.type_id == *t && h.quantity >= f.units)
                    .max_by_key(|h| h.quantity)?;
                Some(SellLine {
                    station_id: station,
                    type_id: *t,
                    item_id: row.item_id,
                    price: f.floor,
                    quantity: f.units,
                })
            })
            .collect();
        if !sell_lines.is_empty() {
            c.sell(&sell_lines, 0, crate::escort::LIST_BROKER_FEE)?;
            self.trip
                .sales
                .extend(sell_lines.iter().map(|l| Sale::Bid(*l)));
        }
        Ok(())
    }
}

/// How long a trip lasts: the roaming ceiling with a map, HK4's without.
fn roam_ceiling(ctx: &roam::Ctx<'_>) -> Duration {
    if ctx.roams() {
        roam::TRIP_CEILING
    } else {
        TRIP_CEILING
    }
}

/// The most sites one trip hacks: the loop's with a map, HK4's without.
fn site_cap(ctx: &roam::Ctx<'_>) -> usize {
    if ctx.roams() {
        roam::SITES_PER_TRIP
    } else {
        MAX_SITES
    }
}

/// Docked: in a hull, every analyzer in the hangar fitted and online.
/// `Ok(Err(lack))` for what is missing; else the fitted analyzers as
/// `(module id, type id)`.
fn ready(
    c: &mut Conn,
    waits: &mut Option<String>,
) -> Result<std::result::Result<Vec<(i64, i64)>, Lack>> {
    let hangar = c.hangar_items()?;
    let active = c.session().ship_id();
    let flying = hangar
        .iter()
        .find(|r| Some(r.item_id) == active)
        .map(|r| r.type_id);
    if flying.is_none_or(|t| t == brain::CAPSULE) {
        // SKILLGATE: only a hull the pilot's skills board.
        let levels = crate::acquire::gate_levels(c)?;
        let fly = |r: &&crate::ItemRow| crate::acquire::flyable_in(&levels, r.type_id);
        let hull = hangar
            .iter()
            .filter(fly)
            .find(|r| r.type_id == HULL)
            .or_else(|| {
                hangar.iter().filter(fly).find(|r| {
                    r.category_id == crate::acquire::CATEGORY_SHIP && r.type_id != brain::CAPSULE
                })
            });
        match hull {
            Some(h) => {
                c.board(h.item_id)?;
            }
            None => {
                if let (Some(r), Some(l)) =
                    (hangar.iter().find(|r| r.type_id == HULL), levels.as_ref())
                {
                    // The act's note says "lacks a hull"; a book that could
                    // not be bought is in the skill plan's lines.
                    let _ = crate::acquire::learn_urgent_soft(
                        c,
                        &crate::acquire::lacking_of(l, r.type_id),
                    )?;
                }
                return Ok(Err(Lack::Hull));
            }
        }
    }
    let spare: Vec<i64> = c
        .hangar_items()?
        .iter()
        .filter(|r| is_analyzer(r))
        .map(|r| r.item_id)
        .collect();
    if !spare.is_empty() {
        // A full rack refuses the rest; what fitted is enough.
        // SKILLGATE: an analyzer the skills do not fit yet queues them (ahead
        // of the rest) and the trip says what it waits on.
        match crate::acquire::fit_auto_gated(c, &spare) {
            Ok(Ok(_)) | Err(Error::Refused(_) | Error::NotSent(_)) => {}
            Ok(Err(note)) => *waits = Some(note),
            Err(e) => return Err(e),
        }
    }
    let fitted: Vec<ItemRow> = c.fitted()?.into_iter().filter(is_analyzer).collect();
    if fitted.is_empty() {
        return Ok(Err(Lack::Analyzer));
    }
    for a in &fitted {
        if !c.is_online(a.item_id)? {
            // SKILLGATE: an online the skills refuse (a world saved before
            // the gate) queues them and waits, as the fit does.
            if let Err(note) = crate::acquire::set_online_gated(c, a.item_id)? {
                *waits = Some(note);
                return Ok(Err(Lack::Analyzer));
            }
        }
    }
    Ok(Ok(fitted.iter().map(|r| (r.item_id, r.type_id)).collect()))
}

/// The analyzers in the order to try them on a site of `kind`: the data
/// ones first on a data site, the others first on a relic site.
pub fn analyzer_order(analyzers: &[(i64, i64)], kind: crate::probe::Kind) -> Vec<(i64, i64)> {
    let data = |t: i64| DATA_ANALYZERS.contains(&t);
    let mut v = analyzers.to_vec();
    v.sort_by_key(|&(id, t)| {
        let first = match kind {
            crate::probe::Kind::Relic => !data(t),
            _ => data(t),
        };
        (!first, id)
    });
    v
}

/// **A data or relic site to work**, landed on: the explorers' ledger's
/// signatures of this system first, then any other data or relic one, none
/// in `tried` or already emptied ([`DONE`]). `None`: nothing left, or no
/// probes to scan with.
fn find_site(
    c: &mut Conn,
    server: SocketAddr,
    tried: &[String],
) -> Result<Option<crate::probe::Signature>> {
    find_site_if(c, server, tried, &|_, _| true)
}

/// [`find_site`], and only a signature `allowed(system, target)` takes
/// (T3-SALVAGE: in J-space, only the sites whose guards a raider cleared).
fn find_site_if(
    c: &mut Conn,
    server: SocketAddr,
    tried: &[String],
    allowed: &dyn Fn(i64, &str) -> bool,
) -> Result<Option<crate::probe::Signature>> {
    use crate::probe::{self, Kind};
    let system = c.session().solar_system_id().unwrap_or_default();
    let mut scanner = match probe::Scanner::open(c) {
        Ok(s) => s,
        Err(Error::NotSent(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
    let booked: Vec<String> = [Kind::Data, Kind::Relic]
        .iter()
        .flat_map(|k| probe::LEDGER.of_kind(server, *k))
        .filter(|(s, _)| *s == system)
        .map(|(_, t)| t)
        .collect();
    let fresh =
        |t: &str| !tried.iter().any(|x| x == t) && !done(server, system, t) && allowed(system, t);
    let hackable = |k: Option<Kind>| roam::job_for(k, true) == roam::Job::Work;
    let me = c.session().char_id().unwrap_or_default();
    let mut left: Vec<probe::Seen> = Vec::new();
    let mut found = None;
    if !booked.is_empty() {
        found = probe::find_seen(
            c,
            &mut scanner,
            |t, k| booked.iter().any(|b| b == t) && fresh(t) && hackable(k),
            |s| left.push(s),
        )?;
    }
    if found.is_none() {
        found = probe::find_seen(
            c,
            &mut scanner,
            |t, k| fresh(t) && hackable(k),
            |s| left.push(s),
        )?;
    }
    // What the scan met and left -- gas, combat, ore -- is booked for the
    // careers that work it.
    for s in &left {
        probe::LEDGER.note((server, system), s, me, Instant::now());
    }
    let Some(sig) = found else {
        return Ok(None);
    };
    probe::warp_to(c, &sig)?;
    Ok(Some(sig))
}

/// What one site's work did.
#[derive(Debug, Default)]
struct Worked {
    games: Vec<Played>,
    types: Vec<i64>,
    full: bool,
}

/// The grid's hackable cans as `(ball id, surface gap, type)`: every ball
/// whose slim says `hackingSecurityState` 0 [V key; HK2 sends it].
pub fn cans_in(c: &mut Conn) -> Vec<(i64, f64, Option<i64>)> {
    let world = c.world_now();
    let Some(ego) = world.ego else {
        return Vec::new();
    };
    let mut v: Vec<(i64, f64, Option<i64>)> = world
        .balls()
        .filter(|b| b.slim.and_then(|s| s.hacking_state) == Some(0))
        .filter_map(|b| {
            Some((
                b.ball_id,
                world.surface_distance(ego, b.ball_id)?,
                b.type_id(),
            ))
        })
        .collect();
    v.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    v
}

/// **Hack every can on the grid**, nearest first.
fn hack_site(
    c: &mut Conn,
    analyzers: &[(i64, i64)],
    kind: crate::probe::Kind,
    seed: u64,
    started: Instant,
) -> Result<Worked> {
    let mut out = Worked::default();
    let mut tried: BTreeMap<i64, u32> = BTreeMap::new();
    let order = analyzer_order(analyzers, kind);
    while started.elapsed() < TRIP_CEILING && !out.full {
        let cans: Vec<(i64, f64, Option<i64>)> = cans_in(c)
            .into_iter()
            .filter(|(id, _, _)| tried.get(id).copied().unwrap_or(0) <= RETRIES)
            .take(brain::SPREAD)
            .collect();
        if cans.is_empty() {
            break;
        }
        // Among the nearest few, the bot's own pick: two hackers on one
        // site start on different cans.
        let (can, _, type_id) = cans[(seed % cans.len() as u64) as usize];
        *tried.entry(can).or_default() += 1;
        let Some(mut played) = hack_can(c, can, &order)? else {
            // Refused by every analyzer, or never locked: not ours to open.
            tried.insert(can, RETRIES + 1);
            continue;
        };
        played.type_id = type_id;
        if played.won {
            tried.insert(can, RETRIES + 1);
            match c.loot_all(can) {
                Ok(l) => {
                    played.looted = l.units;
                    out.types.extend(l.types);
                    if l.refusal.is_some_and(|k| k.contains("NotEnoughCargoSpace")) {
                        out.full = true;
                    }
                }
                // Before HK3 a hacked can is no inventory: nothing to take.
                Err(Error::Refused(_) | Error::NotSent(_)) => {}
                Err(e) => return Err(e),
            }
        }
        let _ = c.unlock_target(can);
        out.games.push(played);
    }
    Ok(out)
}

/// **One can**: approach, lock, start an analyzer (the next on a refusal
/// that names the wrong analyzer), wait for the game and play it. `None`
/// when no analyzer could start on it.
fn hack_can(c: &mut Conn, can: i64, order: &[(i64, i64)]) -> Result<Option<Played>> {
    let reach = order
        .iter()
        .filter_map(|&(m, _)| c.module_row(m).ok()?.attribute(ATTR_MAX_RANGE))
        .fold(f64::MAX, f64::min)
        .min(LOOT_RANGE)
        * brain::RANGE_MARGIN;
    c.follow(can, APPROACH_RANGE.min(reach))?;
    settle(c, |c| gap(c, can).is_none_or(|g| g <= reach))?;
    if gap(c, can).is_none() {
        return Ok(None);
    }
    let locked = match c.lock_target(can) {
        Ok(LockState::Locked) => true,
        Ok(LockState::Pending) => await_lock(c, can)?,
        Err(Error::Refused(_) | Error::NotSent(_)) => false,
        Err(e) => return Err(e),
    };
    if !locked {
        return Ok(None);
    }
    for &(module, _) in order {
        let cycle = c
            .module_row(module)
            .ok()
            .and_then(|r| r.attribute(ATTR_DURATION))
            .map_or(Duration::from_secs(10), |ms| {
                Duration::from_millis(ms.max(0.0) as u64)
            });
        match c.activate(module, DO_HACKING, Some(can), ONE_CYCLE) {
            Ok(_) => {}
            Err(Error::Refused(why)) if why.contains(WRONG_ANALYZER) => continue,
            Err(Error::Refused(_) | Error::NotSent(_)) => return Ok(None),
            Err(e) => return Err(e),
        }
        let mut view = BoardView::new();
        if !await_start(c, &mut view, cycle + START_SLACK)? {
            return Ok(None);
        }
        // A turn that never came back (the server ended the game under the
        // bot: a warp, the range) is a lost game, not a lost trip.
        let outcome = match play(c, &mut view) {
            Ok(o) => o,
            Err(Error::Refused(_)) => Outcome::Lost,
            Err(e) => return Err(e),
        };
        let core = view
            .tiles
            .values()
            .find(|t| !t.hidden && t.kind == CORE)
            .map(|t| t.subtype);
        return Ok(Some(Played {
            can,
            type_id: None,
            won: outcome == Outcome::Won,
            turns: view.turns,
            coherence: view.virus_coherence,
            core,
            looted: 0,
        }));
    }
    Ok(None)
}

/// Sell the hangar's loot by [`sale_plan`]: the bids in one call, each
/// listing in its own.
fn sell(c: &mut Conn, station: i64, looted: &BTreeSet<i64>, builder: bool) -> Result<Vec<Sale>> {
    // A data-site copy is no stack: the market refuses a blueprint, so it
    // goes to the builders by contract (BP-COPIES). A refusal is a note.
    match crate::contracts::offer_copies(c) {
        Ok(_) | Err(Error::Refused(_) | Error::NotSent(_)) => {}
        Err(e) => return Err(e),
    }
    let stacks: Vec<ItemRow> = c
        .hangar_items()?
        .into_iter()
        .filter(|r| is_loot(r, looted))
        .collect();
    if stacks.is_empty() {
        return Ok(Vec::new());
    }
    let mut prices = pricing::Prices::read(c)?;
    let mut books = BTreeMap::new();
    for r in &stacks {
        if let std::collections::btree_map::Entry::Vacant(e) = books.entry(r.type_id) {
            e.insert(c.market_orders(r.type_id)?);
        }
    }
    let listing = {
        let types: Vec<i64> = stacks.iter().map(|r| r.type_id).collect();
        prices.values_of(c, &types)?
    };
    let plan = sale_plan_for(&stacks, looted, &books, &listing, station, builder);
    let bids: Vec<SellLine> = plan
        .iter()
        .filter_map(|s| match s {
            Sale::Bid(l) => Some(*l),
            _ => None,
        })
        .collect();
    if !bids.is_empty() {
        c.sell(&bids, 0, crate::escort::LIST_BROKER_FEE)?;
    }
    for s in &plan {
        if let Sale::List(l) | Sale::Offer(l) = s {
            c.sell(
                &[*l],
                crate::escort::LIST_DAYS,
                crate::escort::LIST_BROKER_FEE,
            )?;
        }
    }
    Ok(plan)
}

/// Undock and bind the system's park.
fn leave(c: &mut Conn) -> Result<()> {
    if c.session().station_id().is_some() {
        c.undock_repaired()?;
    }
    let system = c
        .session()
        .solar_system_id()
        .ok_or_else(|| Error::NotSent("hack: in space with no system".to_owned()))?;
    if c.ballpark().is_none() {
        c.bind_ballpark(system)?;
    }
    settle(c, |c| c.world().has_state())
}

/// Read pushes until an `OnTarget('add', id)` (`true`), a loss or the
/// ceiling (`false`).
fn await_lock(c: &mut Conn, id: i64) -> Result<bool> {
    let deadline = Instant::now() + CEILING;
    while Instant::now() < deadline {
        for e in c.pump(POLL)? {
            let crate::Event::Push(name, args) = e else {
                continue;
            };
            if name != "OnTarget" || args.get(1) != Some(&PyValue::Int(id)) {
                continue;
            }
            match args.first().and_then(crate::text).as_deref() {
                Some("add") => return Ok(true),
                Some("lost") => return Ok(false),
                _ => {}
            }
        }
    }
    Ok(false)
}

/// Fly back to `home` in this system and dock, by the courier's
/// [`next_move`].
fn fly_home(c: &mut Conn, home: i64) -> Result<()> {
    if c.session().station_id().is_none() {
        let _ = crate::probe::recover_all(c);
    }
    for _ in 0..crate::courier::MAX_MOVES {
        let here = Here {
            docked_at: c.session().station_id(),
            bound: c.ballpark().is_some() && c.world().has_state(),
            gap: gap(c, home),
            warping: warping(c),
        };
        match next_move(&here, home) {
            Travel::Arrived => return Ok(()),
            Travel::Undock => {
                c.undock_repaired()?;
            }
            Travel::Bind => leave(c)?,
            Travel::Wait => settle(c, |c| !warping(c))?,
            Travel::Warp => {
                c.warp_to_item(home, 0.0)?;
                settle(c, warping)?;
                settle(c, |c| !warping(c))?;
            }
            Travel::Approach => {
                c.follow(home, 0.0)?;
                settle(c, |c| gap(c, home).is_some_and(|g| g < DOCK_RANGE))?;
            }
            Travel::Dock => {
                c.dock(home)?;
            }
            Travel::NotHere => {
                return Err(Error::NotSent(format!(
                    "hack: home station {home} is not in this system's park"
                )));
            }
        }
    }
    Err(Error::Refused(format!("hack: not docked at {home}")))
}

fn gap(c: &mut Conn, target: i64) -> Option<f64> {
    let world = c.world_now();
    world.surface_distance(world.ego?, target)
}

fn warping(c: &mut Conn) -> bool {
    c.world_now().ego_ball().is_some_and(|b| b.warp_commanded())
}

fn settle_for(c: &mut Conn, d: Duration) -> Result<()> {
    let until = Instant::now() + d;
    while Instant::now() < until {
        c.pump(POLL)?;
    }
    Ok(())
}

fn settle(c: &mut Conn, mut done: impl FnMut(&mut Conn) -> bool) -> Result<()> {
    let deadline = Instant::now() + CEILING;
    while !done(c) {
        if Instant::now() >= deadline {
            return Err(Error::Refused(
                "hack: still moving after the travel ceiling".into(),
            ));
        }
        c.pump(POLL)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Place, Policy, ShipInfo};
    use eve_server::hacking::{
        Board, CoreLevel, Defense, Event as Ev, Game, Node, NodeStats, Rules, Style, Utility,
        VirusStats,
    };

    /// Invented rules: every number made up, none the shipped ones.
    fn rules() -> Rules {
        Rules {
            dims: [(6, 5), (7, 6), (8, 7), (9, 8)],
            hole_percent: 15,
            defenses: [2, 3, 4, 6],
            caches: [2, 2, 3, 3],
            utilities: [1, 1, 2, 2],
            defense_weights: [3, 3, 1, 1, 1],
            basic_defense_max_tier: 1,
            defense_stats: [
                NodeStats {
                    coh_base: 55,
                    coh_per_tier: 7,
                    str_base: 9,
                    str_per_tier: 0,
                },
                NodeStats {
                    coh_base: 23,
                    coh_per_tier: 9,
                    str_base: 19,
                    str_per_tier: 4,
                },
                NodeStats::flat(37, 9),
                NodeStats::flat(41, 11),
                NodeStats::flat(47, 8),
            ],
            core_level: [
                CoreLevel::Low,
                CoreLevel::Medium,
                CoreLevel::Medium,
                CoreLevel::High,
            ],
            core_stats: [(43, 11), (67, 19), (97, 27)],
            cache_weights: [4, 4, 2],
            utility_weights: [1, 1, 1, 1],
            min_start_distance: 2,
            restoration_heal: 9,
            suppressor_amount: 7,
            secondary_vector: (17, 3),
            self_repair: (11, 3),
            shield_hits: 2,
            distance_cap: 4,
            failures_to_destroy: 2,
        }
    }

    /// The test's own strong virus (invented).
    const STRONG: VirusStats = VirusStats {
        coherence: 130,
        strength: 35,
        slots: 3,
    };
    /// The share of seeded tier-1 boards the solver must win with
    /// [`STRONG`] under [`rules`]: a property of the solver (it finishes,
    /// plays no invalid move and wins boards it can), not a balance claim.
    const WIN_FLOOR: f64 = 0.9;
    /// R-HACK's falsifier pilot, for the report only: a Data Analyzer I
    /// (`1909` 40, `1910` 20, `1911` 3 [V-data]) at Hacking III (+10
    /// coherence a level [V]).
    const BALANCE_VIRUS: (f64, f64, f64) = (70.0, 20.0, 3.0);

    fn wire(events: &[Ev]) -> Vec<PyValue> {
        events.iter().map(Ev::to_pyvalue).collect()
    }

    /// What one engine-driven game did.
    #[derive(Debug, Default)]
    struct Run {
        moves: Vec<Move>,
        acks: u32,
        outcome: Option<Outcome>,
    }

    /// **Play `game` with the solver**, the view fed only by the wire form
    /// of the engine's events; `check` sees the engine and the view before
    /// each move.
    fn drive(game: &mut Game, mut check: impl FnMut(&Game, &BoardView, Move)) -> (BoardView, Run) {
        let mut view = BoardView::new();
        view.apply_turn(&wire(&game.start()));
        let mut run = Run::default();
        while view.running() && run.moves.len() < MAX_TURNS as usize {
            let mv = next(&view);
            check(game, &view, mv);
            view.sent(mv);
            let ev = match mv {
                Move::Click(c) => game.click(c),
                Move::UseOnTile(i, c) => game.use_on_tile(i, c),
                Move::UseOnVirus(i) => game.use_on_virus(i),
                Move::Quit => game.quit(),
            };
            if ev == [Ev::Ack] {
                run.acks += 1;
            }
            run.moves.push(mv);
            view.apply_turn(&wire(&ev));
        }
        run.outcome = view.ended;
        (view, run)
    }

    fn game(board: Board, virus: VirusStats) -> Game {
        Game::new(board, &rules(), virus, Style::Hacking, None)
    }

    fn assert_view_is_public_board(view: &BoardView, game: &Game) {
        assert_eq!(view.tiles.len(), game.board.tiles.len());
        for t in &game.board.tiles {
            let r = t.record();
            let v = view.tiles[&t.coord];
            assert_eq!(
                (
                    v.kind,
                    v.subtype,
                    v.hidden,
                    v.blocked,
                    v.strength,
                    v.coherence
                ),
                (
                    r.kind,
                    r.subtype,
                    r.hidden,
                    r.blocked,
                    r.strength,
                    r.coherence
                ),
                "tile {:?}",
                t.coord
            );
        }
        let virus = game.virus();
        assert_eq!(
            (view.virus_coherence, view.virus_strength),
            (virus.coherence, virus.strength)
        );
        let slots: Vec<(Option<i64>, i64)> = view.slots.iter().map(|s| (s.id, s.subtype)).collect();
        let want: Vec<(Option<i64>, i64)> =
            game.inventory().iter().map(|s| (s.id, s.subtype)).collect();
        assert_eq!(slots, want);
    }

    #[test]
    fn wire_constants_are_the_engines() {
        assert_eq!(SELF_REPAIR, Utility::SelfRepair.subtype());
        assert_eq!(KERNEL_ROT, Utility::KernelRot.subtype());
        assert_eq!(SECONDARY_VECTOR, Utility::SecondaryVector.subtype());
        assert_eq!(SHIELD, Utility::PolymorphicShield.subtype());
        assert_eq!(SUPPRESSOR, Defense::Suppressor.subtype());
        assert_eq!(RESTORATION, Defense::Restoration.subtype());
        assert_eq!(SKILL_ARCHAEOLOGY, engine::SKILL_ARCHAEOLOGY);
    }

    /// The input shapes the client sends [V `hackingUISvc`]: a tuple coord,
    /// an index then a coord, an index, nothing.
    #[test]
    fn each_move_is_the_clients_call() {
        let c = (3, 4);
        let xy = PyValue::Tuple(vec![PyValue::Int(3), PyValue::Int(4)]);
        assert_eq!(call_of(Move::Click(c)), ("ClickedOnTile", vec![xy.clone()]));
        assert_eq!(
            call_of(Move::UseOnTile(1, c)),
            ("UsedUtilityElement", vec![PyValue::Int(1), xy])
        );
        assert_eq!(
            call_of(Move::UseOnVirus(2)),
            ("UsedUtilityElementOnVirus", vec![PyValue::Int(2)])
        );
        assert_eq!(call_of(Move::Quit), ("QuitHackingAttempt", vec![]));
        let push = vec![PyValue::List(wire(&[Ev::Ack]))];
        assert_eq!(
            turn_events("OnHackingTurnComplete", &push).map(|e| e.len()),
            Some(1)
        );
        assert_eq!(turn_events("OnTarget", &push), None);
    }

    /// Spec test 1: after every turn the view rebuilt from the pushed events
    /// is the engine's public board (records, virus, slots), over seeded
    /// boards of every tier.
    #[test]
    fn the_view_rebuilt_from_the_events_is_the_engines_public_board() {
        let r = rules();
        for tier in 1..=4u8 {
            for seed in 0..12u64 {
                let mut g = game(Board::generate(&r, tier, seed * 31 + 7), STRONG);
                let (view, _) = drive(&mut g, |g, v, _| assert_view_is_public_board(v, g));
                assert_view_is_public_board(&view, &g);
                assert!(view.ended.is_some(), "tier {tier} seed {seed} finished");
            }
        }
    }

    /// Spec test 2: the core beside the start falls in one reveal and
    /// `ceil(coh / str)` attacks.
    #[test]
    fn a_core_beside_the_start_falls_in_ceil_coh_over_str_clicks() {
        let core = Node::Core {
            level: CoreLevel::Low,
            coherence: 101,
            strength: 3,
        };
        let board = Board::from_layout(
            3,
            2,
            (0, 0),
            vec![
                ((0, 0), Node::Segment),
                ((1, 0), core),
                ((2, 0), Node::Segment),
            ],
        )
        .expect("layout");
        let mut g = game(board, STRONG);
        let (_, run) = drive(&mut g, |_, _, _| {});
        assert_eq!(run.outcome, Some(Outcome::Won));
        let hits = (101 + STRONG.strength - 1) / STRONG.strength;
        assert_eq!(run.moves.len() as i64, 1 + hits, "{:?}", run.moves);
        assert!(run.moves.iter().all(|m| *m == Move::Click((1, 0))));
    }

    /// Spec tests 3 and 6: over 200 seeded tier-1 boards with its own strong
    /// virus the solver finishes every game, never sends an input the
    /// server refuses, never clicks a blocked or unreachable hidden tile,
    /// and wins at least [`WIN_FLOOR`].
    #[test]
    fn the_solver_wins_seeded_boards_and_never_plays_an_invalid_move() {
        let r = rules();
        let mut won = 0;
        let games = 200;
        for seed in 0..games {
            let mut g = game(Board::generate(&r, 1, 1_000 + seed), STRONG);
            let (_, run) = drive(&mut g, |g, _, mv| {
                if let Move::Click(c) = mv {
                    let t = g.board.tile(c).expect("a board cell");
                    if t.hidden {
                        assert!(!t.blocked, "seed {seed}: clicked blocked {c:?}");
                        let reachable = engine::neighbours(c).iter().any(|n| {
                            g.board
                                .tile(*n)
                                .is_some_and(|x| !x.hidden && x.node == Node::Segment)
                        });
                        assert!(reachable, "seed {seed}: clicked unreachable {c:?}");
                    }
                }
            });
            assert_eq!(run.acks, 0, "seed {seed}: {:?}", run.moves);
            assert!(run.outcome.is_some(), "seed {seed} unfinished");
            assert!(run.moves.len() < MAX_TURNS as usize, "seed {seed}");
            won += u32::from(run.outcome == Some(Outcome::Won));
        }
        let rate = f64::from(won) / games as f64;
        assert!(rate >= WIN_FLOOR, "won {won} of {games}");
    }

    /// Spec test 4: on a fat core the solver spends Kernel Rot before it
    /// clicks.
    #[test]
    fn the_solver_rots_a_fat_core() {
        let s = STRONG.strength;
        let board = Board::from_layout(
            3,
            1,
            (0, 0),
            vec![
                ((0, 0), Node::Segment),
                ((1, 0), Node::Utility(Utility::KernelRot)),
                (
                    (2, 0),
                    Node::Core {
                        level: CoreLevel::High,
                        coherence: 5 * s + 1,
                        strength: 2,
                    },
                ),
            ],
        )
        .expect("layout");
        let mut g = game(board, STRONG);
        let (_, run) = drive(&mut g, |_, _, _| {});
        assert_eq!(run.outcome, Some(Outcome::Won), "{:?}", run.moves);
        let rot = run
            .moves
            .iter()
            .position(|m| *m == Move::UseOnTile(0, (2, 0)))
            .expect("Kernel Rot on the core");
        let clicks: Vec<usize> = run
            .moves
            .iter()
            .enumerate()
            .filter(|(_, m)| **m == Move::Click((2, 0)))
            .map(|(i, _)| i)
            .collect();
        // Reveal the utility, pick it up, reveal the core, rot, then hit.
        assert!(
            clicks.len() >= 2 && clicks[0] < rot && clicks[1] > rot,
            "{:?}",
            run.moves
        );
    }

    /// Spec test 5: below the low-coherence line with Self Repair held,
    /// the solver repairs.
    #[test]
    fn the_solver_repairs_below_the_threshold() {
        let weak = VirusStats {
            coherence: LOW_COHERENCE - 1,
            strength: 1,
            slots: 2,
        };
        let board = Board::from_layout(
            4,
            1,
            (0, 0),
            vec![
                ((0, 0), Node::Segment),
                ((1, 0), Node::Utility(Utility::SelfRepair)),
                ((2, 0), Node::Segment),
                (
                    (3, 0),
                    Node::Core {
                        level: CoreLevel::Low,
                        coherence: 3,
                        strength: 1,
                    },
                ),
            ],
        )
        .expect("layout");
        let mut g = game(board, weak);
        let (_, run) = drive(&mut g, |_, _, _| {});
        assert!(run.moves.contains(&Move::UseOnVirus(0)), "{:?}", run.moves);
        assert_eq!(run.outcome, Some(Outcome::Won));
    }

    /// A move the server answered with a lone `ACK` is not sent again
    /// until a turn changes something; with nothing else left, quit.
    #[test]
    fn a_refused_move_is_not_repeated() {
        let board = Board::from_layout(
            2,
            1,
            (0, 0),
            vec![
                ((0, 0), Node::Segment),
                (
                    (1, 0),
                    Node::Core {
                        level: CoreLevel::Low,
                        coherence: 1,
                        strength: 1,
                    },
                ),
            ],
        )
        .expect("layout");
        let g = game(board, STRONG);
        let mut view = BoardView::new();
        view.apply_turn(&wire(&g.start()));
        let first = next(&view);
        assert_eq!(first, Move::Click((1, 0)));
        view.sent(first);
        view.apply_turn(&wire(&[Ev::Ack]));
        assert_eq!(next(&view), Move::Quit);
    }

    /// The candidates honour the distance indicators, the largest read as
    /// a lower bound.
    #[test]
    fn indicators_narrow_the_core_candidates() {
        let mut view = BoardView {
            started: true,
            ..BoardView::default()
        };
        for x in 0..6 {
            view.tiles.insert(
                (x, 0),
                ViewTile {
                    hidden: x > 0,
                    ..ViewTile::default()
                },
            );
        }
        view.indicators.insert((0, 0), 2);
        assert_eq!(view.candidates(), vec![(2, 0), (3, 0), (4, 0), (5, 0)]);
        view.indicators.insert((1, 0), 1);
        // 1 is now exact; 2 (the largest) is "at least".
        assert_eq!(view.candidates(), vec![(2, 0)]);
    }

    fn row(item_id: i64, type_id: i64, group_id: i64, quantity: i64) -> ItemRow {
        ItemRow {
            item_id,
            type_id,
            owner_id: 90_000_001,
            location_id: 61_000_001,
            flag_id: crate::FLAG_HANGAR,
            quantity,
            group_id,
            category_id: 17,
            singleton: false,
        }
    }

    /// The sale: a bid takes a stack, a priced type is listed, an unpriced
    /// one is kept, and the kit (analyzers, probes) is never sold.
    #[test]
    fn the_sale_bids_lists_keeps_and_spares_the_kit() {
        const STATION: i64 = 61_000_001;
        const BID_TYPE: i64 = 990_777_001;
        const REF_TYPE: i64 = 990_777_002;
        const BARE_TYPE: i64 = 990_777_003;
        let looted = BTreeSet::from([BID_TYPE, REF_TYPE, BARE_TYPE]);
        let stacks = vec![
            row(1, BID_TYPE, 990_001, 3),
            row(2, REF_TYPE, 990_001, 2),
            row(3, BARE_TYPE, 990_001, 1),
            row(4, DATA_ANALYZER_I, GROUP_ANALYZER, 1),
            row(5, crate::probe::CORE_PROBE, crate::probe::GROUP_PROBE, 8),
        ];
        let books = BTreeMap::from([(
            BID_TYPE,
            Book {
                asks: vec![],
                bids: vec![crate::verbs::BookOrder {
                    order_id: 1,
                    type_id: BID_TYPE,
                    station_id: STATION,
                    price: 7.0,
                    vol_remaining: 10,
                    bid: true,
                }],
            },
        )]);
        let refs = BTreeMap::from([(REF_TYPE, 100.0)]);
        let plan = sale_plan(&stacks, &looted, &books, &refs, STATION);
        assert_eq!(plan.len(), 3, "{plan:?}");
        assert!(matches!(plan[0], Sale::Bid(l) if l.item_id == 1 && l.price == 7.0));
        assert!(matches!(plan[1], Sale::List(l) if l.item_id == 2));
        assert!(matches!(
            plan[2],
            Sale::Keep {
                type_id: BARE_TYPE,
                ..
            }
        ));
    }

    /// CAP-LOOT: a capital commodity looted from a can is not a builder
    /// input of the datacore kind: with a builder in the corporation it is
    /// still listed at the ask (no bid) where a capital builder's market
    /// buys, and with a bid it goes into it. Nothing is hard-coded: it is
    /// sold because it was looted.
    #[test]
    fn a_looted_capital_commodity_is_listed_for_the_builders() {
        const STATION: i64 = 61_000_001;
        const COMMODITY: i64 = 990_777_011;
        let looted = BTreeSet::from([COMMODITY]);
        let stacks = vec![row(1, COMMODITY, 990_526, 1)];
        let refs = BTreeMap::from([(COMMODITY, 1.0e9)]);
        for builder in [false, true] {
            let plan = sale_plan_for(&stacks, &looted, &BTreeMap::new(), &refs, STATION, builder);
            assert!(
                matches!(plan[0], Sale::List(l) if l.item_id == 1 && l.quantity == 1),
                "builder {builder}: {plan:?}"
            );
        }
    }

    #[test]
    fn the_data_analyzer_goes_first_on_a_data_site_and_last_on_a_relic_site() {
        use crate::probe::Kind;
        const OTHER: i64 = 990_538_002;
        let fitted = [(11, OTHER), (12, DATA_ANALYZER_I)];
        assert_eq!(analyzer_order(&fitted, Kind::Data)[0].1, DATA_ANALYZER_I);
        assert_eq!(analyzer_order(&fitted, Kind::Relic)[0].1, OTHER);
    }

    /// **The balance report** (R-HACK §2 falsifier; not a gate): win rates
    /// per tier under the shipped rules for a Data Analyzer I pilot at
    /// Hacking III. Asserts nothing about them; run with `--ignored
    /// --nocapture`.
    #[test]
    #[ignore = "a report, not a gate"]
    fn balance_report_under_the_shipped_rules() {
        let virus = VirusStats::from_attributes(BALANCE_VIRUS.0, BALANCE_VIRUS.1, BALANCE_VIRUS.2);
        for tier in 1..=4u8 {
            let mut won = 0;
            for seed in 0..200u64 {
                let board = Board::generate(&Rules::SHIPPED, tier, seed);
                let mut g = Game::new(board, &Rules::SHIPPED, virus, Style::Hacking, None);
                let (_, run) = drive(&mut g, |_, _, _| {});
                won += u32::from(run.outcome == Some(Outcome::Won));
            }
            println!("[hack] balance: tier {tier}: won {won} of 200 ({virus:?})");
        }
    }

    fn docked_obs(ship_type: Option<i64>) -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.place = Place::Docked {
            station: 61_000_001,
        };
        o.ship = Some(ShipInfo {
            ship_id: 100_000_201,
            type_id: ship_type,
        });
        o
    }

    /// Docked in a hull with nothing lacking: one trip and a dwell; in a
    /// capsule with a hull in the hangar: board it.
    #[test]
    fn docked_it_boards_a_hull_then_goes_hacking() {
        let name = "Invented Hacker";
        let mut m = Memory::new(
            name,
            Policy {
                role: &ROLE,
                ..Policy::fast()
            },
        );
        let mut o = docked_obs(Some(brain::CAPSULE));
        let mut hull = row(100_000_300, HULL, 25, 1);
        hull.category_id = crate::acquire::CATEGORY_SHIP;
        hull.singleton = true;
        o.hangar.push(hull);
        assert_eq!(docked(&mut m, &o), vec![Act::Board(100_000_300)]);
        let o = docked_obs(Some(HULL));
        let acts = docked(&mut m, &o);
        assert_eq!(acts.first(), Some(&Act::Hack), "{acts:?}");
        assert!(ROLE.civilian);
        assert_eq!(ROLE.name(), "hack");
    }

    /// With a builder in the corporation, datacores and decryptors are
    /// offered at the ask -- never dumped into a bid -- and the rest of
    /// the loot is sold as ever; without one they are sold as ever.
    #[test]
    fn a_corp_builders_inputs_are_offered_not_dumped_into_a_bid() {
        const STATION: i64 = 61_000_001;
        const CORE: i64 = 990_778_001;
        const DECRYPTOR: i64 = 990_778_002;
        const SALVAGE: i64 = 990_778_003;
        const NO_PRICE: i64 = 990_778_004;
        let looted = BTreeSet::from([CORE, DECRYPTOR, SALVAGE, NO_PRICE]);
        let mut decryptor = row(2, DECRYPTOR, 990_002, 1);
        decryptor.category_id = roam::CATEGORY_DECRYPTOR;
        let stacks = vec![
            row(1, CORE, roam::GROUP_DATACORE, 4),
            decryptor,
            row(3, SALVAGE, 990_003, 6),
            row(4, NO_PRICE, roam::GROUP_DATACORE, 2),
        ];
        let bid = |t: i64| {
            (
                t,
                Book {
                    asks: vec![],
                    bids: vec![crate::verbs::BookOrder {
                        order_id: t,
                        type_id: t,
                        station_id: STATION,
                        price: 9.0,
                        vol_remaining: 100,
                        bid: true,
                    }],
                },
            )
        };
        let books = BTreeMap::from([bid(CORE), bid(DECRYPTOR), bid(SALVAGE)]);
        let refs = BTreeMap::from([(CORE, 100.0), (DECRYPTOR, 50.0), (SALVAGE, 5.0)]);
        let with = sale_plan_for(&stacks, &looted, &books, &refs, STATION, true);
        assert!(
            matches!(with[0], Sale::Offer(l) if l.item_id == 1 && l.price > 9.0),
            "{with:?}"
        );
        assert!(
            matches!(with[1], Sale::Offer(l) if l.item_id == 2),
            "{with:?}"
        );
        assert!(
            matches!(with[2], Sale::Bid(l) if l.item_id == 3),
            "{with:?}"
        );
        assert!(
            matches!(
                with[3],
                Sale::Keep {
                    type_id: NO_PRICE,
                    ..
                }
            ),
            "no price, no offer: kept {with:?}"
        );
        let without = sale_plan_for(&stacks, &looted, &books, &refs, STATION, false);
        assert!(matches!(without[0], Sale::Bid(_)), "{without:?}");
        assert!(matches!(without[1], Sale::Bid(_)), "{without:?}");
        assert_eq!(
            without,
            sale_plan(&stacks, &looted, &books, &refs, STATION),
            "the plain plan is the no-builder plan"
        );
    }

    fn space_obs() -> Observation {
        Observation::empty(Instant::now())
    }

    /// In space with a trip waiting the bot resumes it -- until the trip
    /// has failed three times, when it is given up and the bot goes home
    /// the plain way.
    #[test]
    fn in_space_a_waiting_trip_is_resumed_until_it_keeps_failing() {
        let name = "Invented Resumer";
        let mut m = Memory::new(
            name,
            Policy {
                role: &ROLE,
                ..Policy::fast()
            },
        );
        assert_eq!(
            in_space(&mut m, &space_obs()),
            vec![Act::Wait(brain::SHORT_WAIT)]
        );
        let st = roam::TripState::new(name, 1, 2, roam::unix_now(), 0.0);
        roam::put(None, &st);
        assert_eq!(in_space(&mut m, &space_obs()), vec![Act::Hack]);
        let mut o = space_obs();
        o.failures
            .insert(crate::brain::ActKind::Hack, brain::GIVE_UP_AFTER);
        assert_ne!(in_space(&mut m, &o), vec![Act::Hack]);
        assert!(!roam::resumable(name), "the trip was abandoned");
    }

    /// A trip's summary says where it roamed, who it avoided, why it
    /// turned and where it sold.
    #[test]
    fn the_summary_tells_the_roam() {
        let trip = Trip {
            route: vec![5, 6],
            avoided: vec![(6, roam::Threat::Hostile(2))],
            turned: Some(roam::Turn::Full),
            outlet: Some(77),
            resumed: true,
            ..Trip::default()
        };
        let said = trip.summary();
        for want in [
            "[resumed]",
            "[roam] route [5, 6]",
            "left 6: Hostile(2)",
            "turned for home: Full",
            "sold at station 77",
        ] {
            assert!(said.contains(want), "{want} in {said}");
        }
        assert!(
            Trip {
                homed: true,
                ..Trip::default()
            }
            .summary()
            .contains("flown home")
        );
    }
}
