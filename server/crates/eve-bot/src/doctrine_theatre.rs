//! **The response machine, per theatre** (DOC-4, R-DOCTRINE 7).
//!
//! A *theatre* is a system (or a sov structure's system) where a leader's
//! group is engaged. The machine is pure: [`step`] takes the theatre, a
//! [`Ctx`] (the picture as foes, the group's doctrine, the members as
//! [`Asset`]s, the knobs) and the time, and answers the [`Action`]s the fleet
//! commander takes on its own connection (`fleet::orders_for`). The hall only
//! computes; nothing here conjures a ship or a member that is offline.
//!
//! ```text
//! Watch -> Scout -> Commit(k) -> Surge -> Commit
//!                      |            \-> Disengage -> Rebuild -> Watch
//!                      \-> (no fight, a counter is makeable) Hold -> Scout | Watch
//! ```
//!
//! - **Available strength** ([`available`]) is not total strength: a member
//!   counts when it is online (or wakes within [`WAKE_H`]), can reach the
//!   theatre within [`TRAVEL_H`], holds a doctrine ship or can reship within
//!   [`RESHIP_MARKET_H`], and obeys this step (`P_OBEY`). Each carries an
//!   *arrival time*: wake + reship + travel + the fleet's spread.
//! - **Staged commitment** ([`commit_cap`], [`needed`]): a step commits
//!   `COMMIT_STEP` of the shortfall, never more than `(1 - RESERVE)` of the
//!   pool, and never inside [`COMMIT_LAG_S`] of the last step. The `ER`
//!   judged is that of the ships *present* (arrived, not lost), so a surge
//!   that needs a reship waits for it.
//! - **Reinforcement** ([`Call`], [`settle`]): an ally answers a `NeedBackup`
//!   only when `R[ally][asker] >= HELP_AT`, its aggression is at least
//!   [`HELP_AGGRESSION`] and the exchange is not lost; if it came the hall
//!   records `Event::Reinforced` (both views), if it could and did not within
//!   [`HELP_WAIT_S`], `Event::Abandoned` (the asker's view).
//!
//! **What DOC-5/6 must publish for this to bite**: `corpmind::Readiness` per
//! bot (online, next window, system, the ship it holds). Without a report a
//! member counts as online at the staging system holding a doctrine ship in
//! the corp hangar ([`Ship::Hangar`]).

use crate::doctrine::Doctrine;
use crate::doctrine_exchange::{Features, Side, exchange};
use crate::doctrine_lead::LOSS_RATIO_CAP;
use crate::doctrine_sense::{EnemyMemory, Lens, Memory, Picture, Seen, SenseKnobs, infer};
use crate::society::{Event, Group};
use std::collections::BTreeSet;

// ---- named constants (all [A]; R-DOCTRINE 11) -----------------------------------------------

/// The least reliability to act on a picture **[A]**.
pub const CONF_MIN: f64 = 0.3;
/// `ER` margin over `ENGAGE_ER` the commitment aims at **[A]**.
pub const ENGAGE_MARGIN: f64 = 0.1;
/// `ER` under which a thin fight is a surge, not a keep **[A]**.
pub const HOLD_ER: f64 = 0.0;
/// Loss ratio (ISK lost / ISK killed over the window) a fight may run at
/// before it asks for more, and the one at which it runs **[A]**. Falsified
/// by a fleet that runs while winning or fights to the last ship.
pub const LOSS_OK: f64 = 1.2;
pub const LOSS_BAIL: f64 = 2.5;
/// The loss window, seconds (20 minutes) **[A]**.
pub const LOSS_WINDOW_S: u64 = 20 * 60;
/// Minimum seconds between commit steps, so a fleet is never "all at once"
/// **[A]**.
pub const COMMIT_LAG_S: u64 = 5 * 60;
/// Scout reports (non-blind steps) before the choice **[A]**.
pub const REPORT_N: u32 = 2;
/// Surge steps before the machine gives up **[A]**.
pub const SURGE_MAX: u32 = 3;
/// A member that wakes within this many game hours counts **[A]**.
pub const WAKE_H: f64 = 1.0;
/// The longest trip a member may be from the theatre, game hours **[A]**.
pub const TRAVEL_H: f64 = 0.5;
/// Seconds per jump and for the last warp, game time **[A]**.
pub const JUMP_S: f64 = 60.0;
pub const WARP_S: f64 = 30.0;
/// Reship: a hull in the corp hangar (game minutes), one to buy (game hours)
/// **[A]**.
pub const RESHIP_HANGAR_MIN: f64 = 5.0;
pub const RESHIP_MARKET_H: f64 = 1.0;
/// Rebuild after a disengage, game hours **[A]**.
pub const REBUILD_H: f64 = 4.0;
/// A hold lasts this many game hours unless the enemy moves **[A]**.
pub const HOLD_H: f64 = 2.0;
/// The enemy count moving by this share ends a hold **[A]**.
pub const HOLD_BREAK: f64 = 0.25;
/// A caller's ally must be at least this aggressive to answer **[A]**.
pub const HELP_AGGRESSION: f64 = 0.4;
/// Seconds an ally that could help is given **[A]**.
pub const HELP_WAIT_S: u64 = 600;
/// `HELP_AT` is `effects::HELP_AT` (R-REL-3); re-exported for the tests.
pub const HELP_AT: f64 = crate::effects::HELP_AT;

// ---- difficulty (R-DOCTRINE 10: commitment and coordination) ----------------------------------

/// **The theatre knobs**: the commitment, coordination and resources groups
/// of the difficulty table. [`Self::NORMAL`] is the spec's normal column.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TheatreKnobs {
    /// Share of the shortfall committed per step.
    pub commit_step: f64,
    /// Share of the pool held back.
    pub reserve: f64,
    /// Exchange score to start.
    pub engage_er: f64,
    /// Exchange score to run.
    pub disengage_er: f64,
    pub max_theatres: usize,
    /// Share of the fleet that follows orders in a step.
    pub p_obey: f64,
    /// Gap between first and last arrival, seconds.
    pub arrive_spread_s: f64,
    /// Seconds between primary calls.
    pub fc_call_s: u64,
    /// Seconds from a changed picture to a new order.
    pub tactical_delay_s: u64,
    /// Loss ratio before a size step.
    pub escalate_er: f64,
}

impl TheatreKnobs {
    pub const EASY: TheatreKnobs = TheatreKnobs {
        commit_step: 0.25,
        reserve: 0.40,
        engage_er: 0.60,
        disengage_er: -0.2,
        max_theatres: 1,
        p_obey: 0.60,
        arrive_spread_s: 120.0,
        fc_call_s: 20,
        tactical_delay_s: 600,
        escalate_er: 3.0,
    };
    pub const NORMAL: TheatreKnobs = TheatreKnobs {
        commit_step: 0.35,
        reserve: 0.30,
        engage_er: 0.35,
        disengage_er: -0.5,
        max_theatres: 1,
        p_obey: 0.80,
        arrive_spread_s: 60.0,
        fc_call_s: 10,
        tactical_delay_s: 240,
        escalate_er: 2.0,
    };
    pub const HARD: TheatreKnobs = TheatreKnobs {
        commit_step: 0.50,
        reserve: 0.20,
        engage_er: 0.20,
        disengage_er: -0.7,
        max_theatres: 2,
        p_obey: 0.95,
        arrive_spread_s: 20.0,
        fc_call_s: 5,
        tactical_delay_s: 90,
        escalate_er: 1.5,
    };
    pub const BRUTAL: TheatreKnobs = TheatreKnobs {
        commit_step: 0.70,
        reserve: 0.10,
        engage_er: 0.10,
        disengage_er: -0.9,
        max_theatres: 3,
        p_obey: 1.00,
        arrive_spread_s: 5.0,
        fc_call_s: 3,
        tactical_delay_s: 30,
        escalate_er: 1.2,
    };

    /// Level 1 easy .. 4 brutal; 2 is normal; 0 is normal with a reserve of
    /// 0.6 (the peaceful world never commits more than 40 %).
    pub fn at(level: u8) -> TheatreKnobs {
        match level {
            0 => TheatreKnobs {
                reserve: 0.6,
                ..Self::NORMAL
            },
            1 => Self::EASY,
            3 => Self::HARD,
            4.. => Self::BRUTAL,
            _ => Self::NORMAL,
        }
    }
}

impl Default for TheatreKnobs {
    fn default() -> Self {
        Self::NORMAL
    }
}

/// Game-time conversion: a game day is `day_s` real seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clock {
    pub day_s: u64,
}

impl Clock {
    pub fn hours(&self, h: f64) -> f64 {
        h * self.day_s as f64 / 24.0
    }
    pub fn minutes(&self, m: f64) -> f64 {
        self.hours(m / 60.0)
    }
    /// Game seconds as real ones.
    pub fn secs(&self, s: f64) -> f64 {
        s * self.day_s as f64 / 86_400.0
    }
}

// ---- who can come ----------------------------------------------------------------------------

/// What a member holds to fly the doctrine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ship {
    /// A doctrine ship in hand (docked or in space).
    Staged,
    /// A doctrine hull and fit in the corp hangar: [`RESHIP_HANGAR_MIN`].
    Hangar,
    /// Must be bought and fitted: [`RESHIP_MARKET_H`].
    Market,
    /// Nobody makes it: the member cannot be counted.
    Nobody,
}

/// One member as the machine sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Asset {
    pub char_id: i64,
    pub online: bool,
    /// Seconds until its next window, when offline.
    pub wake_in_s: Option<u64>,
    /// Jumps to the theatre; `None`: unreachable.
    pub jumps: Option<u32>,
    pub ship: Ship,
}

/// A member that can come and when it is on grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Avail {
    pub char_id: i64,
    /// Seconds from now to arrival.
    pub eta_s: f64,
}

fn die(parts: &[u64]) -> f64 {
    let mut h = 0x9e37_79b9_7f4a_7c15_u64;
    for p in parts {
        h ^= p.wrapping_add(0x9e37_79b9_7f4a_7c15);
        h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        h ^= h >> 31;
    }
    let mut s = h | 1;
    crate::brain::unit(crate::brain::xorshift(&mut s))
}

/// **Available strength** (R-DOCTRINE 7.1): who can come, soonest first.
/// `tick` fixes the obedience dice within a step.
pub fn available(
    assets: &[Asset],
    clock: &Clock,
    k: &TheatreKnobs,
    seed: u64,
    tick: u64,
) -> Vec<Avail> {
    let mut out = Vec::new();
    for a in assets {
        let wake = if a.online {
            0.0
        } else {
            match a.wake_in_s {
                Some(w) if (w as f64) <= clock.hours(WAKE_H) => w as f64,
                _ => continue,
            }
        };
        let Some(j) = a.jumps else { continue };
        let travel = clock.secs(f64::from(j) * JUMP_S + WARP_S);
        if travel > clock.hours(TRAVEL_H) {
            continue;
        }
        let reship = match a.ship {
            Ship::Staged => 0.0,
            Ship::Hangar => clock.minutes(RESHIP_HANGAR_MIN),
            Ship::Market => clock.hours(RESHIP_MARKET_H),
            Ship::Nobody => continue,
        };
        if die(&[a.char_id as u64, tick, seed]) >= k.p_obey {
            continue;
        }
        let spread = die(&[a.char_id as u64, seed, 5]) * k.arrive_spread_s;
        out.push(Avail {
            char_id: a.char_id,
            eta_s: wake + reship + travel + spread,
        });
    }
    out.sort_by(|a, b| a.eta_s.total_cmp(&b.eta_s).then(a.char_id.cmp(&b.char_id)));
    out
}

/// **The most a pool may commit** (R-DOCTRINE 7.3): `(1 - RESERVE)` of it,
/// rounded down, and at least one ship stays home from a pool of two or
/// more.
pub fn commit_cap(pool: usize, reserve: f64) -> usize {
    if pool == 0 {
        return 0;
    }
    let cap = ((1.0 - reserve.clamp(0.0, 1.0)) * pool as f64).floor() as usize;
    if pool >= 2 { cap.min(pool - 1) } else { cap }
}

// ---- the exchange against what the leader believes -------------------------------------------

/// An enemy component: a doctrine and how many ships of it.
#[derive(Debug, Clone, Copy)]
pub struct Foe<'a> {
    pub doc: &'a Doctrine,
    pub ships: f64,
}

/// **`ER` of `n` of my ships against the enemy** (R-DOCTRINE 7.2): my force
/// is shared out over the enemy components by their ships, each pair is the
/// exchange at the band the dictator takes, and the result is the
/// ship-weighted mean. Positive: I am ahead. No ship of mine: `-NONE_ER`.
pub fn er_with(mine: &Doctrine, n: f64, foes: &[Foe<'_>], feat: Features) -> f64 {
    let total: f64 = foes.iter().map(|f| f.ships).sum();
    if n <= 0.0 {
        return -NONE_ER;
    }
    if total <= 0.0 {
        return NONE_ER;
    }
    foes.iter()
        .map(|f| {
            let w = f.ships / total;
            let x = exchange(
                &Side { d: mine, n: n * w },
                &Side {
                    d: f.doc,
                    n: f.ships,
                },
                feat,
            );
            w * x.er
        })
        .sum()
}

/// The score of no ships against some, and of some against none **[A]**.
pub const NONE_ER: f64 = 10.0;

/// **The smallest force that reaches `engage_er + ENGAGE_MARGIN`**, by
/// binary search (the exchange rises with the number of ships). `None` when
/// even `max` does not.
pub fn needed(
    mine: &Doctrine,
    foes: &[Foe<'_>],
    engage_er: f64,
    max: usize,
    feat: Features,
) -> Option<usize> {
    let goal = engage_er + ENGAGE_MARGIN;
    if max == 0 || er_with(mine, max as f64, foes, feat) < goal {
        return None;
    }
    let (mut lo, mut hi) = (1usize, max);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if er_with(mine, mid as f64, foes, feat) >= goal {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Some(lo)
}

// ---- the picture as foes ---------------------------------------------------------------------

/// **The enemy as doctrines** (R-DOCTRINE 5/6): per hostile group in the
/// picture, its hulls laid over what the memory knows of its weapons and
/// tank, inferred against the catalog. Returns `(catalog index, ships)`
/// components and the ship-weighted confidence.
pub fn foes_of(
    picture: &Picture,
    memory: &Memory,
    lens: &Lens,
    knobs: &SenseKnobs,
    catalog: &[Doctrine],
    is_foe: &dyn Fn(Group) -> bool,
) -> (Vec<(usize, f64)>, f64) {
    let mut comps: Vec<(usize, f64)> = Vec::new();
    let (mut conf_sum, mut ships_sum) = (0.0, 0.0);
    for e in picture.theirs.iter().filter(|e| is_foe(e.group)) {
        let mut mem = EnemyMemory::new(e.group);
        if let Some(old) = memory.groups.get(&e.group) {
            mem.seen = old.seen.clone();
        }
        for &(hull, n) in &e.hulls {
            let s: &mut Seen = mem.seen.entry(hull).or_default();
            s.n_max = s.n_max.max(n);
            s.last_s = picture.at_s;
        }
        let (top, conf) = infer(&mem, picture.at_s, knobs, lens, catalog);
        let ships = f64::from(e.ships());
        let conf = conf.min(e.conf).clamp(0.0, 1.0);
        conf_sum += conf * ships;
        ships_sum += ships;
        for (key, share) in top {
            if let Some(i) = catalog.iter().position(|d| d.key == key) {
                match comps.iter_mut().find(|c| c.0 == i) {
                    Some(c) => c.1 += ships * share,
                    None => comps.push((i, ships * share)),
                }
            }
        }
    }
    comps.sort_by_key(|c| c.0);
    let conf = if ships_sum > 0.0 {
        conf_sum / ships_sum
    } else {
        0.0
    };
    (comps, conf)
}

// ---- the loss window -------------------------------------------------------------------------

/// ISK lost and killed in the last [`LOSS_WINDOW_S`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LossWindow {
    rows: Vec<(u64, f64, f64)>,
}

impl LossWindow {
    pub fn note(&mut self, at_s: u64, lost: f64, killed: f64) {
        self.rows.push((at_s, lost.max(0.0), killed.max(0.0)));
        if self.rows.len() > 256 {
            self.rows.remove(0);
        }
    }

    /// `lost / killed` over the window; `0` with nothing lost; capped.
    pub fn ratio(&self, now_s: u64) -> f64 {
        let (mut lost, mut killed) = (0.0, 0.0);
        for (at, l, k) in &self.rows {
            if now_s.saturating_sub(*at) <= LOSS_WINDOW_S {
                lost += l;
                killed += k;
            }
        }
        if lost <= 0.0 {
            return 0.0;
        }
        (lost / killed.max(lost / LOSS_RATIO_CAP)).min(LOSS_RATIO_CAP)
    }
}

// ---- the theatre -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Watch,
    Scout,
    /// Committed; the step count.
    Commit(u32),
    Hold,
    Surge,
    Disengage,
    Rebuild,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Watch => "watch",
            State::Scout => "scout",
            State::Commit(_) => "commit",
            State::Hold => "hold",
            State::Surge => "surge",
            State::Disengage => "disengage",
            State::Rebuild => "rebuild",
        }
    }
}

/// A member sent to the theatre and when it is on grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sent {
    pub char_id: i64,
    pub arrives_s: u64,
}

/// **One theatre.**
#[derive(Debug, Clone, PartialEq)]
pub struct Theatre {
    pub system: i64,
    pub state: State,
    pub changed_s: u64,
    pub sent: Vec<Sent>,
    pub lost: BTreeSet<i64>,
    pub steps: u32,
    pub surge_steps: u32,
    pub scouts: u32,
    pub last_step_s: Option<u64>,
    pub losses: LossWindow,
    pub escalated: bool,
    pub until_s: u64,
    /// Enemy ships when a hold began.
    pub held_against: f64,
}

impl Theatre {
    pub fn new(system: i64, now_s: u64) -> Theatre {
        Theatre {
            system,
            state: State::Watch,
            changed_s: now_s,
            sent: Vec::new(),
            lost: BTreeSet::new(),
            steps: 0,
            surge_steps: 0,
            scouts: 0,
            last_step_s: None,
            losses: LossWindow::default(),
            escalated: false,
            until_s: 0,
            held_against: 0.0,
        }
    }

    /// Ships on grid at `now_s`: arrived and not lost.
    pub fn present(&self, now_s: u64) -> usize {
        self.sent
            .iter()
            .filter(|s| s.arrives_s <= now_s && !self.lost.contains(&s.char_id))
            .count()
    }

    /// Ships sent and alive, arrived or not.
    pub fn committed(&self) -> usize {
        self.sent
            .iter()
            .filter(|s| !self.lost.contains(&s.char_id))
            .count()
    }

    /// A ship of the fleet was lost.
    pub fn note_lost(&mut self, char_id: i64, now_s: u64, isk_lost: f64, isk_killed: f64) {
        if self.sent.iter().any(|s| s.char_id == char_id) {
            self.lost.insert(char_id);
        }
        self.losses.note(now_s, isk_lost, isk_killed);
    }

    fn enter(&mut self, s: State, now_s: u64) {
        self.state = s;
        self.changed_s = now_s;
    }

    fn pending(&self, now_s: u64) -> bool {
        self.sent
            .iter()
            .any(|s| s.arrives_s > now_s && !self.lost.contains(&s.char_id))
    }
}

/// What the machine reads each step.
pub struct Ctx<'a> {
    pub now_s: u64,
    pub tick: u64,
    pub seed: u64,
    pub clock: Clock,
    pub knobs: &'a TheatreKnobs,
    /// The group's current doctrine.
    pub mine: &'a Doctrine,
    pub foes: &'a [Foe<'a>],
    /// 0..1, [`foes_of`].
    pub conf: f64,
    pub assets: &'a [Asset],
    /// An enemy in a system we hold or claim, or next to one.
    pub contested: bool,
    /// A counter doctrine that is makeable or in stock (its key).
    pub counter: Option<&'a str>,
    pub feat: Features,
}

/// What the fleet commander does.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Send a scout and read the d-scan.
    Scout { system: i64 },
    /// Form up and warp these members to the theatre.
    Commit { who: Vec<i64> },
    /// More of the shortfall.
    Surge { who: Vec<i64> },
    /// `NeedBackup` to the fleet and the allies.
    NeedBackup { system: i64 },
    /// `HoldPosition`.
    HoldPosition,
    /// Warp to staging, dock, save the ships; mark structures if losing.
    Disengage { abandon_if_losing: bool },
    /// The loss ratio crossed `ESCALATE_ER`: open a size step.
    Escalate,
    /// Order replacement stock for the counter doctrine.
    Rebuild { counter: Option<String> },
}

fn due(t: &Theatre, c: &Ctx<'_>) -> bool {
    t.last_step_s.is_none_or(|l| {
        c.now_s.saturating_sub(l) >= crate::doctrine_sense::BATTLE_TICK_S.max(c.knobs.fc_call_s)
    })
}

fn ships_of(foes: &[Foe<'_>]) -> f64 {
    foes.iter().map(|f| f.ships).sum()
}

fn pick(av: &[Avail], taken: &BTreeSet<i64>, n: usize) -> Vec<Avail> {
    av.iter()
        .filter(|a| !taken.contains(&a.char_id))
        .take(n)
        .copied()
        .collect()
}

fn send(t: &mut Theatre, chosen: &[Avail], now_s: u64) -> Vec<i64> {
    for a in chosen {
        t.sent.push(Sent {
            char_id: a.char_id,
            arrives_s: now_s + a.eta_s.ceil() as u64,
        });
    }
    chosen.iter().map(|a| a.char_id).collect()
}

/// **One step of the machine** (R-DOCTRINE 7.2). Rate-limited to one
/// judgement per `BATTLE_TICK_S` (and `FC_CALL_S`); returns no actions
/// between.
pub fn step(t: &mut Theatre, c: &Ctx<'_>) -> Vec<Action> {
    if !due(t, c) && !matches!(t.state, State::Watch) {
        return Vec::new();
    }
    let k = c.knobs;
    let now = c.now_s;
    let seen = ships_of(c.foes);
    match t.state {
        State::Watch => {
            if c.contested && seen > 0.0 && c.conf >= CONF_MIN {
                t.scouts = 0;
                t.enter(State::Scout, now);
                t.last_step_s = Some(now);
                return vec![Action::Scout { system: t.system }];
            }
            Vec::new()
        }
        State::Scout => {
            t.last_step_s = Some(now);
            if c.conf < CONF_MIN || seen <= 0.0 {
                // The enemy is gone, or unreadable: nothing to answer.
                if seen <= 0.0 && t.scouts >= REPORT_N {
                    t.enter(State::Watch, now);
                }
                return Vec::new();
            }
            t.scouts += 1;
            if t.scouts < REPORT_N || now.saturating_sub(t.changed_s) < k.tactical_delay_s {
                return Vec::new();
            }
            let av = available(c.assets, &c.clock, k, c.seed, c.tick);
            let cap = commit_cap(av.len(), k.reserve);
            let best = er_with(c.mine, cap as f64, c.foes, c.feat);
            if cap > 0 && best >= k.engage_er {
                let need = needed(c.mine, c.foes, k.engage_er, cap, c.feat).unwrap_or(cap);
                let first = ((k.commit_step * need as f64).ceil() as usize).clamp(1, cap);
                let chosen = pick(&av, &BTreeSet::new(), first);
                let who = send(t, &chosen, now);
                t.steps = 1;
                t.enter(State::Commit(0), now);
                vec![Action::Commit { who }]
            } else if let Some(key) = c.counter {
                t.held_against = seen;
                t.until_s = now + c.clock.hours(HOLD_H) as u64;
                t.enter(State::Hold, now);
                vec![
                    Action::Rebuild {
                        counter: Some(key.to_owned()),
                    },
                    Action::HoldPosition,
                ]
            } else {
                t.enter(State::Disengage, now);
                vec![Action::Disengage {
                    abandon_if_losing: false,
                }]
            }
        }
        State::Commit(n) => {
            t.last_step_s = Some(now);
            let lr = t.losses.ratio(now);
            if lr >= LOSS_BAIL {
                return disengage(t, now, lr);
            }
            if t.present(now) == 0 {
                // Nobody is there yet to judge a fight by.
                return Vec::new();
            }
            let er = er_with(c.mine, t.present(now) as f64, c.foes, c.feat);
            if er < k.disengage_er {
                return disengage(t, now, lr);
            }
            let mut out = Vec::new();
            if lr >= k.escalate_er && !t.escalated {
                t.escalated = true;
                out.push(Action::Escalate);
            }
            if er >= k.engage_er && lr <= LOSS_OK {
                t.state = State::Commit(n);
                return out;
            }
            t.surge_steps = 0;
            t.enter(State::Surge, now);
            out.extend(surge(t, c, er, lr));
            out
        }
        State::Surge => {
            let er = er_with(c.mine, t.present(now) as f64, c.foes, c.feat);
            let lr = t.losses.ratio(now);
            if er < k.disengage_er || lr >= LOSS_BAIL {
                t.last_step_s = Some(now);
                return disengage(t, now, lr);
            }
            if t.pending(now) {
                // Waiting for arrivals (ARRIVE_S), not for the next call.
                t.last_step_s = Some(now);
                return Vec::new();
            }
            if er >= k.engage_er && lr <= LOSS_OK {
                t.last_step_s = Some(now);
                t.enter(State::Commit(t.steps), now);
                return Vec::new();
            }
            if now.saturating_sub(t.last_step_s.unwrap_or(0)) < COMMIT_LAG_S {
                return Vec::new();
            }
            t.last_step_s = Some(now);
            if t.surge_steps >= SURGE_MAX {
                return disengage(t, now, lr);
            }
            surge(t, c, er, lr)
        }
        State::Hold => {
            t.last_step_s = Some(now);
            let moved = t.held_against > 0.0
                && ((seen - t.held_against).abs() / t.held_against) >= HOLD_BREAK;
            if moved && c.conf >= CONF_MIN {
                t.scouts = 0;
                t.enter(State::Scout, now);
                return vec![Action::Scout { system: t.system }];
            }
            if seen <= 0.0 || now >= t.until_s {
                t.enter(State::Watch, now);
            }
            Vec::new()
        }
        State::Disengage => {
            t.last_step_s = Some(now);
            t.until_s = now + c.clock.hours(REBUILD_H) as u64;
            t.enter(State::Rebuild, now);
            vec![Action::Rebuild {
                counter: c.counter.map(str::to_owned),
            }]
        }
        State::Rebuild => {
            t.last_step_s = Some(now);
            if now >= t.until_s {
                t.enter(State::Watch, now);
            }
            Vec::new()
        }
    }
}

fn disengage(t: &mut Theatre, now: u64, lr: f64) -> Vec<Action> {
    t.sent.clear();
    t.lost.clear();
    t.enter(State::Disengage, now);
    vec![Action::Disengage {
        abandon_if_losing: lr >= LOSS_OK,
    }]
}

/// One surge step: commit `COMMIT_STEP` of the shortfall from what can come,
/// never past `(1 - RESERVE)` of the pool, and ask for backup.
fn surge(t: &mut Theatre, c: &Ctx<'_>, er: f64, lr: f64) -> Vec<Action> {
    let k = c.knobs;
    let now = c.now_s;
    t.surge_steps += 1;
    t.steps += 1;
    let taken: BTreeSet<i64> = t.sent.iter().map(|s| s.char_id).collect();
    let av = available(c.assets, &c.clock, k, c.seed, c.tick);
    let rest = pick(&av, &taken, usize::MAX);
    let alive = t.committed();
    let pool = alive + rest.len();
    let room = commit_cap(pool, k.reserve).saturating_sub(alive);
    let max = (alive + room).max(1);
    // The shortfall is against what the exchange needs, or against the
    // whole cap when no force can reach the goal.
    let need = needed(c.mine, c.foes, k.engage_er, max, c.feat).unwrap_or(max);
    let shortfall = need.saturating_sub(alive);
    let n = if shortfall == 0 {
        0
    } else {
        ((k.commit_step * shortfall as f64).ceil() as usize)
            .max(1)
            .min(room)
    };
    let chosen: Vec<Avail> = rest.into_iter().take(n).collect();
    let mut out = Vec::new();
    if !chosen.is_empty() {
        let who = send(t, &chosen, now);
        out.push(Action::Surge { who });
    }
    let _ = (er, lr);
    out.push(Action::NeedBackup { system: t.system });
    out
}

// ---- reinforcement and relationship ----------------------------------------------------------

/// A `NeedBackup` and who has answered it.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub asker: Group,
    pub system: i64,
    pub at_s: u64,
    /// Allies already settled (came, or abandoned).
    pub done: BTreeSet<Group>,
}

impl Call {
    pub fn new(asker: Group, system: i64, at_s: u64) -> Call {
        Call {
            asker,
            system,
            at_s,
            done: BTreeSet::new(),
        }
    }
}

/// An ally as the call sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ally {
    pub group: Group,
    /// `R[ally][asker]`.
    pub r_to_asker: f64,
    /// The ally's mean aggression trait.
    pub aggression: f64,
    /// Members that could come ([`available`]).
    pub available: usize,
    /// `ER(ally_available, theirs)`.
    pub er: f64,
    /// A member of the ally is on the caller's grid.
    pub arrived: bool,
}

impl Ally {
    /// Has the ally the standing and the means (whatever its temper)?
    pub fn able(&self) -> bool {
        self.available > 0 && self.r_to_asker >= HELP_AT
    }

    /// **Does the ally answer?** (R-DOCTRINE 7.4)
    pub fn answers(&self) -> bool {
        self.able() && self.aggression >= HELP_AGGRESSION && self.er > HOLD_ER
    }
}

/// **Settle a call** at `now_s`: the events for allies that came, and for
/// those that could and did not within [`HELP_WAIT_S`]. Each ally is settled
/// once per call. `isk_saved` is the value the help kept alive (0: unknown).
pub fn settle(call: &mut Call, allies: &[Ally], now_s: u64, isk_saved: f64) -> Vec<Event> {
    let mut out = Vec::new();
    for a in allies {
        if a.group == call.asker || call.done.contains(&a.group) {
            continue;
        }
        if a.arrived {
            call.done.insert(a.group);
            out.push(Event::Reinforced {
                helper: a.group,
                helped: call.asker,
                isk_saved,
            });
        } else if a.able() && now_s.saturating_sub(call.at_s) >= HELP_WAIT_S {
            call.done.insert(a.group);
            out.push(Event::Abandoned {
                asker: call.asker,
                ally: a.group,
            });
        }
    }
    out
}

/// **Friendly fire**: a ship of a blue (by the shooter's own view) shot
/// another. `None` unless the two were allied.
pub fn friendly_fire(shooter: Group, victim: Group, allied: bool) -> Option<Event> {
    (allied && shooter != victim).then_some(Event::Betrayed {
        betrayer: shooter,
        victim,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctrine::testkit::*;

    const DAY: u64 = 86_400;
    const SYS: i64 = 30_000_142;

    fn clock() -> Clock {
        Clock { day_s: DAY }
    }

    fn asset(id: i64) -> Asset {
        Asset {
            char_id: id,
            online: true,
            wake_in_s: None,
            jumps: Some(1),
            ship: Ship::Staged,
        }
    }

    fn assets(n: usize) -> Vec<Asset> {
        (0..n as i64).map(|i| asset(9_000 + i)).collect()
    }

    /// The knobs with everyone obeying, so counts are exact.
    fn knobs() -> TheatreKnobs {
        TheatreKnobs {
            p_obey: 1.0,
            ..TheatreKnobs::NORMAL
        }
    }

    struct Fx {
        mine: Doctrine,
        foe: Doctrine,
    }

    /// Mine are kiters, theirs bricks: a fleet of kiters wins at modest
    /// numbers (the invented world's `ER` is huge in my favour).
    fn fx() -> Fx {
        let cat = invented_catalog();
        Fx {
            mine: cat[0].clone(),
            foe: cat[1].clone(),
        }
    }

    fn ctx<'a>(
        now_s: u64,
        k: &'a TheatreKnobs,
        f: &'a Fx,
        foes: &'a [Foe<'a>],
        assets: &'a [Asset],
    ) -> Ctx<'a> {
        Ctx {
            now_s,
            tick: now_s / 30,
            seed: 7,
            clock: clock(),
            knobs: k,
            mine: &f.mine,
            foes,
            conf: 0.9,
            assets,
            contested: true,
            counter: None,
            feat: Features::default(),
        }
    }

    /// Drive the machine every 30 s for `secs`, collecting actions.
    fn run(
        t: &mut Theatre,
        from: u64,
        secs: u64,
        k: &TheatreKnobs,
        f: &Fx,
        foes: &[Foe<'_>],
        assets: &[Asset],
    ) -> Vec<(u64, Action)> {
        let mut out = Vec::new();
        let mut now = from;
        while now < from + secs {
            for a in step(t, &ctx(now, k, f, foes, assets)) {
                out.push((now, a));
            }
            now += 30;
        }
        out
    }

    #[test]
    fn a_pool_commits_at_most_its_reserve_cap() {
        for pool in 0..40usize {
            for r in [0.1, 0.3, 0.4, 0.6, 1.0] {
                let cap = commit_cap(pool, r);
                assert!(cap <= pool);
                assert!((cap as f64) <= (1.0 - r) * pool as f64 + 1e-9, "{pool} {r}");
                if pool >= 2 {
                    assert!(cap < pool, "one stays home");
                }
            }
        }
    }

    #[test]
    fn staged_commitment_never_exceeds_the_reserve_cap() {
        let f = fx();
        // A fight the machine will not run from (it keeps asking), too big
        // for the pool: it surges to the cap and no further, step by step.
        let f = Fx {
            mine: f.foe.clone(),
            foe: f.mine.clone(),
        };
        let k = TheatreKnobs {
            disengage_er: -1.0e3,
            ..knobs()
        };
        let foes = [Foe {
            doc: &f.foe,
            ships: 40.0,
        }];
        let pool = assets(10);
        let cap = commit_cap(pool.len(), k.reserve);
        assert_eq!(cap, 7);
        // Committed on a hope, one ship already on grid.
        let mut t = Theatre::new(SYS, 0);
        t.sent.push(Sent {
            char_id: 9_000,
            arrives_s: 0,
        });
        t.state = State::Commit(0);
        t.steps = 1;
        let (mut most, mut grew_at) = (0, Vec::new());
        let mut now = 0;
        while now < 4 * 3_600 {
            let before = t.committed();
            step(&mut t, &ctx(now, &k, &f, &foes, &pool));
            assert!(t.committed() <= cap, "{} > {cap} at {now}", t.committed());
            if t.committed() > before {
                grew_at.push(now);
            }
            most = most.max(t.committed());
            now += 30;
        }
        assert!(most >= 2, "the machine did surge: {most}");
        // Never twice inside the lag: the steps are spaced.
        for w in grew_at.windows(2) {
            assert!(w[1] - w[0] >= COMMIT_LAG_S, "steps {grew_at:?}");
        }
        assert!(
            grew_at.len() >= 2,
            "it was staged, not one lump: {grew_at:?}"
        );
    }

    #[test]
    fn a_fight_it_can_win_commits_in_stages_not_all_at_once() {
        let f = fx();
        let k = knobs();
        let foes = [Foe {
            doc: &f.foe,
            ships: 12.0,
        }];
        let pool = assets(10);
        let mut t = Theatre::new(SYS, 0);
        let acts = run(&mut t, 0, 3_600, &k, &f, &foes, &pool);
        let first = acts
            .iter()
            .find_map(|(_, a)| match a {
                Action::Commit { who } => Some(who.len()),
                _ => None,
            })
            .expect("it commits");
        assert!(first >= 1);
        assert!(first < commit_cap(pool.len(), k.reserve) || first == 1);
        assert!(matches!(
            t.state,
            State::Commit(_) | State::Surge | State::Hold
        ));
    }

    #[test]
    fn offline_members_are_not_committed() {
        let k = knobs();
        let mut a = assets(6);
        // Three sleep for a day, one wakes within the hour.
        for x in a.iter_mut().take(3) {
            x.online = false;
            x.wake_in_s = Some(DAY);
        }
        a[3].online = false;
        a[3].wake_in_s = Some(600);
        let av = available(&a, &clock(), &k, 1, 1);
        assert_eq!(av.len(), 3, "two online and the one that wakes soon");
        assert!(av.iter().all(|x| x.char_id >= 9_003));
        let late = av.iter().find(|x| x.char_id == 9_003).unwrap();
        let early = av.iter().find(|x| x.char_id == 9_004).unwrap();
        assert!(late.eta_s >= 600.0 && early.eta_s < late.eta_s);
        // The machine sends only those.
        let f = fx();
        let foes = [Foe {
            doc: &f.foe,
            ships: 6.0,
        }];
        let mut t = Theatre::new(SYS, 0);
        run(&mut t, 0, 2_000, &k, &f, &foes, &a);
        assert!(t.sent.iter().all(|s| s.char_id >= 9_003));
        // Unreachable, too far, and no ship at all are skipped as well.
        let mut b = assets(4);
        b[0].jumps = None;
        b[1].jumps = Some(10_000);
        b[2].ship = Ship::Nobody;
        assert_eq!(available(&b, &clock(), &k, 1, 1).len(), 1);
    }

    #[test]
    fn obedience_is_a_share_of_the_fleet() {
        let a = assets(200);
        let n = |p: f64| {
            let k = TheatreKnobs {
                p_obey: p,
                ..knobs()
            };
            available(&a, &clock(), &k, 3, 5).len()
        };
        assert_eq!(n(1.0), 200);
        assert!(n(0.6) < n(0.95));
        assert!((100..140).contains(&n(0.6)), "{}", n(0.6));
        // A different step throws different dice.
        let k = TheatreKnobs {
            p_obey: 0.5,
            ..knobs()
        };
        assert_ne!(
            available(&a, &clock(), &k, 3, 5),
            available(&a, &clock(), &k, 3, 6)
        );
    }

    #[test]
    fn reship_time_delays_the_surge() {
        // A thin fight the machine means to stay in (it will not run yet).
        let k = TheatreKnobs {
            disengage_er: -1.0e3,
            ..knobs()
        };
        let mut hangar = assets(6);
        for a in &mut hangar {
            a.ship = Ship::Hangar;
        }
        let mut market = assets(6);
        for a in &mut market {
            a.ship = Ship::Market;
        }
        let staged = assets(6);
        let eta = |a: &[Asset]| available(a, &clock(), &k, 1, 1)[0].eta_s;
        assert!(eta(&staged) < eta(&hangar), "a hangar hull takes minutes");
        assert!(eta(&hangar) < eta(&market), "a bought hull takes an hour");
        assert!(eta(&market) >= clock().hours(RESHIP_MARKET_H));
        // In a fight that needs a surge, the market-sourced reinforcements
        // are not "present" until the hour is up.
        let f = fx();
        let f = Fx {
            mine: f.foe.clone(),
            foe: f.mine.clone(),
        };
        let foes = [Foe {
            doc: &f.foe,
            ships: 30.0,
        }];
        let mut t = Theatre::new(SYS, 0);
        t.sent.push(Sent {
            char_id: 9_000,
            arrives_s: 0,
        });
        t.state = State::Surge;
        t.steps = 1;
        t.last_step_s = Some(0);
        let acts = step(&mut t, &ctx(600, &k, &f, &foes, &market));
        assert!(
            acts.iter().any(|a| matches!(a, Action::Surge { .. })),
            "{acts:?}"
        );
        let new: Vec<&Sent> = t.sent.iter().filter(|s| s.char_id != 9_000).collect();
        assert!(!new.is_empty());
        assert!(
            new.iter().all(|s| s.arrives_s >= 600 + 3_600),
            "an hour of reship"
        );
        assert_eq!(t.present(700), 1, "only the first ship is on grid");
        assert!(t.present(600 + 4_000) > 1);
        // While they are on the way the machine waits and asks nothing more.
        assert!(step(&mut t, &ctx(900, &k, &f, &foes, &market)).is_empty());
        assert!(t.pending(900));
    }

    #[test]
    fn a_losing_fight_disengages_and_rebuilds() {
        let f = fx();
        let k = knobs();
        let foes = [Foe {
            doc: &f.foe,
            ships: 12.0,
        }];
        let pool = assets(10);
        let mut t = Theatre::new(SYS, 0);
        run(&mut t, 0, 1_200, &k, &f, &foes, &pool);
        assert!(t.committed() >= 1);
        // The fleet bleeds: a loss ratio past LOSS_BAIL.
        let now = 1_300;
        let who = t.sent[0].char_id;
        t.note_lost(who, now, 5.0e9, 1.0e9);
        let acts = step(&mut t, &ctx(now + 30, &k, &f, &foes, &pool));
        assert!(
            acts.iter().any(|a| matches!(
                a,
                Action::Disengage {
                    abandon_if_losing: true
                }
            )),
            "{acts:?}"
        );
        assert_eq!(t.state, State::Disengage);
        assert!(t.sent.is_empty());
        let acts = step(&mut t, &ctx(now + 90, &k, &f, &foes, &pool));
        assert!(matches!(acts[0], Action::Rebuild { .. }));
        assert_eq!(t.state, State::Rebuild);
        // After the rebuild it watches again.
        step(&mut t, &ctx(now + 90 + 5 * 3_600, &k, &f, &foes, &pool));
        assert_eq!(t.state, State::Watch);
    }

    #[test]
    fn a_fight_it_cannot_win_with_a_counter_to_make_holds() {
        let f = fx();
        // Bricks against kiters: hopeless. A counter exists.
        let hopeless = Fx {
            mine: f.foe.clone(),
            foe: f.mine.clone(),
        };
        let k = knobs();
        let foes = [Foe {
            doc: &hopeless.foe,
            ships: 10.0,
        }];
        let pool = assets(8);
        let mut t = Theatre::new(SYS, 0);
        let mut got = Vec::new();
        for now in (0..2_000).step_by(30) {
            let mut c = ctx(now, &k, &hopeless, &foes, &pool);
            c.counter = Some("the-counter");
            got.extend(step(&mut t, &c));
        }
        assert!(got.contains(&Action::Rebuild {
            counter: Some("the-counter".to_owned())
        }));
        assert!(got.contains(&Action::HoldPosition));
        assert_eq!(t.state, State::Hold);
        assert!(t.sent.is_empty(), "nothing was committed");
        // With no counter it declines the fight altogether.
        let mut t = Theatre::new(SYS, 0);
        let acts = run(&mut t, 0, 2_000, &k, &hopeless, &foes, &pool);
        assert!(acts.iter().any(|(_, a)| matches!(
            a,
            Action::Disengage {
                abandon_if_losing: false
            }
        )));
        assert!(t.sent.is_empty());
    }

    #[test]
    fn an_unreadable_or_unseen_enemy_starts_nothing() {
        let f = fx();
        let k = knobs();
        let pool = assets(8);
        let foes = [Foe {
            doc: &f.foe,
            ships: 10.0,
        }];
        let mut t = Theatre::new(SYS, 0);
        let mut c = ctx(0, &k, &f, &foes, &pool);
        c.conf = 0.1;
        assert!(step(&mut t, &c).is_empty());
        assert_eq!(t.state, State::Watch);
        let mut c = ctx(0, &k, &f, &foes, &pool);
        c.contested = false;
        assert!(step(&mut t, &c).is_empty());
        assert_eq!(t.state, State::Watch);
        let c = ctx(0, &k, &f, &[], &pool);
        assert!(step(&mut t, &c).is_empty());
    }

    #[test]
    fn needed_is_the_smallest_force_that_reaches_the_goal() {
        let f = fx();
        let foes = [Foe {
            doc: &f.foe,
            ships: 12.0,
        }];
        let feat = Features::default();
        let n = needed(&f.mine, &foes, 0.35, 30, feat).expect("kiters beat bricks");
        assert!(er_with(&f.mine, n as f64, &foes, feat) >= 0.35 + ENGAGE_MARGIN);
        if n > 1 {
            assert!(er_with(&f.mine, (n - 1) as f64, &foes, feat) < 0.35 + ENGAGE_MARGIN);
        }
        // Nothing reaches it against a hopeless matchup.
        let hopeless = [Foe {
            doc: &f.mine,
            ships: 12.0,
        }];
        assert_eq!(needed(&f.foe, &hopeless, 0.35, 30, feat), None);
        assert!(er_with(&f.mine, 0.0, &foes, feat) < 0.0);
    }

    fn ally(group: Group, r: f64, available: usize, arrived: bool) -> Ally {
        Ally {
            group,
            r_to_asker: r,
            aggression: 0.6,
            available,
            er: 0.5,
            arrived,
        }
    }

    #[test]
    fn an_ally_that_can_help_and_does_not_loses_standing() {
        let mut call = Call::new(100, SYS, 1_000);
        let allies = [
            ally(200, HELP_AT + 1.0, 3, false), // could, did not
            ally(300, HELP_AT + 1.0, 3, true),  // came
            ally(400, HELP_AT - 1.0, 5, false), // no standing: owes nothing
            ally(500, HELP_AT + 1.0, 0, false), // no means: owes nothing
        ];
        // Before the wait: only the arrival is settled.
        let early = settle(&mut call, &allies, 1_000 + HELP_WAIT_S - 1, 0.0);
        assert_eq!(
            early,
            vec![Event::Reinforced {
                helper: 300,
                helped: 100,
                isk_saved: 0.0
            }]
        );
        let late = settle(&mut call, &allies, 1_000 + HELP_WAIT_S, 0.0);
        assert_eq!(
            late,
            vec![Event::Abandoned {
                asker: 100,
                ally: 200
            }]
        );
        // Each ally is settled once.
        assert!(settle(&mut call, &allies, 9_999, 0.0).is_empty());
        // The events move the asker's view of the absent ally down, both
        // views of the one that came up.
        let mut r = crate::society::Relations::default();
        for e in early.iter().chain(&late) {
            r.apply(e);
        }
        assert!(r.get(100, 200) < 0.0);
        assert_eq!(r.get(200, 100), 0.0, "the ally's own view does not move");
        assert!(r.get(100, 300) > 0.0 && r.get(300, 100) > 0.0);
    }

    #[test]
    fn help_needs_standing_temper_and_odds() {
        let a = ally(1, HELP_AT, 2, false);
        assert!(a.answers());
        assert!(
            !Ally {
                r_to_asker: HELP_AT - 0.1,
                ..a
            }
            .answers()
        );
        assert!(
            !Ally {
                aggression: HELP_AGGRESSION - 0.1,
                ..a
            }
            .answers()
        );
        assert!(
            !Ally {
                er: HOLD_ER - 0.1,
                ..a
            }
            .answers()
        );
        assert!(!Ally { available: 0, ..a }.answers());
        // A pacifist ally that could help is still owed the abandonment:
        // the asker's view does not read temper.
        assert!(
            Ally {
                aggression: 0.0,
                ..a
            }
            .able()
        );
    }

    #[test]
    fn friendly_fire_betrays_only_between_blues() {
        assert_eq!(
            friendly_fire(1, 2, true),
            Some(Event::Betrayed {
                betrayer: 1,
                victim: 2
            })
        );
        assert_eq!(friendly_fire(1, 2, false), None);
        assert_eq!(friendly_fire(1, 1, true), None);
        let mut r = crate::society::Relations::default();
        r.apply(&Event::Betrayed {
            betrayer: 1,
            victim: 2,
        });
        assert!(r.get(2, 1) < -crate::society::K_ABANDON);
        assert_eq!(r.get(1, 2), 0.0);
    }

    #[test]
    fn the_knobs_are_monotone_in_level() {
        let l = [
            TheatreKnobs::EASY,
            TheatreKnobs::NORMAL,
            TheatreKnobs::HARD,
            TheatreKnobs::BRUTAL,
        ];
        for w in l.windows(2) {
            assert!(w[0].commit_step < w[1].commit_step);
            assert!(w[0].reserve > w[1].reserve);
            assert!(w[0].engage_er > w[1].engage_er);
            assert!(w[0].disengage_er > w[1].disengage_er);
            assert!(w[0].max_theatres <= w[1].max_theatres);
            assert!(w[0].p_obey < w[1].p_obey);
            assert!(w[0].arrive_spread_s > w[1].arrive_spread_s);
            assert!(w[0].fc_call_s > w[1].fc_call_s);
            assert!(w[0].tactical_delay_s > w[1].tactical_delay_s);
            assert!(w[0].escalate_er > w[1].escalate_er);
        }
        assert_eq!(TheatreKnobs::at(2), TheatreKnobs::NORMAL);
        assert_eq!(TheatreKnobs::at(0).reserve, 0.6);
    }

    #[test]
    fn the_loss_window_forgets() {
        let mut w = LossWindow::default();
        assert_eq!(w.ratio(0), 0.0);
        w.note(0, 300.0, 100.0);
        assert!((w.ratio(10) - 3.0).abs() < 1e-9);
        assert_eq!(w.ratio(LOSS_WINDOW_S + 1), 0.0);
        w.note(2_000, 100.0, 100.0);
        assert!((w.ratio(2_100) - 1.0).abs() < 1e-9);
    }
}
