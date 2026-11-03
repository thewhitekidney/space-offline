//! **ROAM-GANG: a hostile gang that spreads out, talks and fights as one**
//! (the owner's ask: gangs that "spawn either in system or in the nearby
//! systems and then they fan out to find us, communicate in-between when
//! they find us and think tactically how to engage, instead of just feeding
//! 1 by 1").
//!
//! A `roamgang` bot is a hunter ([`crate::hunt`]) that does not hunt alone.
//! The bots of one gang share **one in-process state** ([`GangHall`], like
//! the doctrine hall), keyed by the gang id (the roster group). Every bot
//! posts what it sees each turn ([`Post`]); the **FC** (the lowest-ranked
//! member still posting, the roster's first bot) reads the posts, thinks
//! ([`Gang::think`]) and the state hands every member its order
//! ([`Gang::order`]). The bots act on the order with the verbs that already
//! exist; nothing here fires a gun, steers or warps by its own rules
//! (ENGINE-FIX owns those): a fight is [`crate::hunt::fight`] on the gang's
//! primary.
//!
//! 1. **Form up** ([`Plan::Form`]): the members undock; the roster's boss
//!    forms the fleet ([`crate::dev::Fleeter`]); when all (or, after
//!    [`FORM_TIMEOUT_S`], two or more) are in space the FC names the
//!    **staging** system -- the one it is in -- and says so.
//! 2. **Fan out** ([`assign_routes`], pure): the fastest hulls ([`pick_scouts`],
//!    [`GangKnobs::scouts`] of them, never the FC) each get different
//!    branches of the gate graph within [`GangKnobs::radius`] jumps of the
//!    staging; only hunting ground, high sec too while a war is on. The main
//!    body holds at staging.
//! 3. **Report**: a scout that finds hostile pilots on its grid (after a
//!    directional scan sent it to a belt, [`crate::hunt::pick_belt`]) posts
//!    them ([`Post::foes`]) and says one intel line in fleet chat
//!    ([`intel_line`]).
//! 4. **Think** ([`decide`], pure): the report's hulls and power
//!    ([`crate::hunt::power`] over the doctrine's classes) against the
//!    gang's present and potential power, at the knob's required odds
//!    ([`GangKnobs::required_odds`], the hunter's [`crate::hunt::required_odds`]
//!    scaled): **engage**, **wait for more**, **bait** or **leave**. The
//!    commitment is staged ([`staged`]): the smallest set of ships that
//!    clears the bar goes first and the rest are the reserve, released in a
//!    surge ([`crate::doctrine_theatre::SURGE_MAX`]) when the fight needs it.
//! 5. **Engage as one** ([`Plan::Gather`], [`Plan::Land`], [`Plan::Fight`]):
//!    the committed ships fly to the scout's system together and wait at the
//!    gate until every one is in; only then the FC calls the landing, and
//!    all warp to the scout, which tackles ([`Order::Land`]). Nobody is sent
//!    ahead: an order to fight is given only to a ship on a grid where at
//!    least [`MIN_COMMIT`] of the gang are (or will be inside the landing
//!    window). The FC calls the **primary** ([`choose_primary`], the
//!    hunter's [`crate::hunt::score`]) and re-calls it when it dies or
//!    warps off.
//! 6. **Retreat together** ([`should_retreat`], pure): when the present
//!    power over the foes on grid falls under the bar's abort fraction, or
//!    half the committed ships are lost, the FC calls [`Plan::Withdraw`] and
//!    everyone goes home to repair. The gang re-forms at staging and roams
//!    on; the system is avoided for [`LEAVE_COOLDOWN_S`].
//!
//! 7. **Come to the owner** (GANG-SUMMON, [`Quarry`], [`Plan::Hunt`]): a gang
//!    spawned `hunt me` (`/bots 5 hostile roamers`), or summoned
//!    (`/bots come`, [`summon`]), knows a character and his system. It forms
//!    at half strength, or [`QUARRY_FORM_S`] after the first ship is up,
//!    never waiting for the slowest of 150; then every member flies to his
//!    system and warps to his character ([`Order::Rally`]). A hostile gang
//!    that sees pilots on his grid fights them without weighing the odds
//!    ([`decide`]): the owner asked for this fight; it breaks off only on
//!    [`should_retreat`]'s losses, never its odds. An allied gang stays on
//!    his grid.
//!
//! **Every number here is [A]** with its falsifier on the constant.
//! Difficulty ([`GangKnobs`]): aggression, scouts, radius and required odds.

use crate::brain::{self, Act, Ball, Homing, Memory, Observation, ParkView, Place};
use crate::escort;
use crate::hostile::Class;
use crate::hunt::{self, Ground, HuntAct, Pilot};
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::travel::{ANY_SECURITY, Map};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---- named constants (all [A]) --------------------------------------------------------------

/// A member that has not posted for this long is gone (offline): it is not
/// counted and cannot be the FC **[A]**. A flight over the gates is one
/// blocking act of the bot's thread (about 30 s a jump on the test server),
/// so this is longer than a flight of a few jumps. Falsified by a gang that
/// loses its FC or a committed ship to a long flight, or that keeps a
/// silent ship in its count for minutes.
pub const POST_STALE_S: u64 = 150;
/// A gang hunter goes home to repair under this hull fraction **[A]**: light
/// rat fire on a solo hunter in a belt must not end the hunt (the escort's
/// 0.95 is for a miner's escort). Falsified by a hunter that dies with hull
/// above this, or one that still docks over a scratch.
pub const GANG_REPAIR_HULL: f64 = 0.7;
/// After this long the gang forms with whoever is up, if two or more **[A]**.
/// Falsified by a gang that waits forever for a bot that never logs in.
pub const FORM_TIMEOUT_S: u64 = 120;
/// A report older than this is not acted on **[A]**. Falsified by a gang
/// that goes to a system the enemy left minutes ago.
pub const REPORT_TTL_S: u64 = 180;
/// A system the gang decided to leave alone is not looked at again for this
/// long **[A]**. Falsified by a gang that keeps coming back to the fight it
/// refused.
pub const LEAVE_COOLDOWN_S: u64 = 600;
/// The committed ships have this long to be in the scout's system **[A]**.
pub const GATHER_TIMEOUT_S: u64 = 240;
/// After the landing call the ships have this long to be on the grid **[A]**.
pub const LAND_TIMEOUT_S: u64 = 90;
/// A withdrawal is over after this long whatever the ships are doing **[A]**.
pub const WITHDRAW_TIMEOUT_S: u64 = 300;
/// A bait is held this long before the FC thinks again **[A]**.
pub const BAIT_S: u64 = 90;
/// A fight with no foe on any grid for this long is won **[A]**.
pub const CLEAR_S: u64 = 8;
/// Fewest ships that make a fight (the gang never sends one ship) **[A]**.
pub const MIN_COMMIT: usize = 2;
/// Present over the bar's odds, times this, is the least a bait is tried
/// at **[A]**: a bait draws a bigger gang apart, it does not fix a hopeless
/// fight. Falsified by baits that always end in a retreat.
pub const BAIT_BAND: f64 = 0.7;
/// The committed set is sized to clear the bar by this margin
/// ([`crate::doctrine_theatre::ENGAGE_MARGIN`]'s idea) **[A]**.
pub const COMMIT_MARGIN: f64 = 0.1;
/// A member under this armour fraction is not counted as fit to commit **[A]**.
pub const FIT_HEALTH: f64 = 0.8;
/// The fraction of the committed ships lost at which the gang withdraws **[A]**.
pub const LOSS_RETREAT: f64 = 0.5;
/// The scout's own line is said again no sooner than this **[A]**.
pub const SAY_EVERY_S: u64 = 60;
/// The abort fraction of the bar at aggression 0 and at aggression 1 **[A]**:
/// the hunter's single constant ([`hunt::ABORT_ODDS`]) becomes a range.
pub const ABORT_TIMID: f64 = 0.8;
pub const ABORT_RECKLESS: f64 = 0.4;
/// A gang with a quarry forms this long after its first ship is in space,
/// whatever the rest are doing, if half is not up sooner **[A]**. Falsified
/// by a 150-ship gang that arrives a handful at a time, or one that waits
/// minutes for undockers.
pub const QUARRY_FORM_S: u64 = 45;
/// How close a hostile gang warps to its quarry, metres **[A]**: inside the
/// guns' reach of a frigate gang and outside a smartbomb's. Falsified by a
/// gang that lands and cannot shoot, or one that lands inside his guns'
/// blind spot.
pub const RALLY_HOSTILE_RANGE: f64 = 10_000.0;
/// How close an allied gang warps to the character it joins **[A]**.
/// Falsified by escorts that land on top of him or too far to help.
pub const RALLY_ALLIED_RANGE: f64 = 2_500.0;
/// The rally warp is repeated no sooner than this while the quarry is not
/// on the grid **[A]**. Falsified by a warp that is still under way when it
/// is repeated.
pub const RALLY_EVERY_S: u64 = 20;
/// A quarry nobody has seen for this long, whose warp target the server
/// refused, is dropped and the gang roams again **[A]**. Falsified by a gang
/// that gives up on a pilot who is merely in warp.
pub const QUARRY_LOST_S: u64 = 60;
/// The hull-class rank above which a hull is not a scout **[A]**: only the
/// small hulls scout while any exist ([`pick_scouts`]).
pub const SCOUT_CLASS_MAX: i32 = 1;

// ---- difficulty ------------------------------------------------------------------------------

/// **The gang's tunables per difficulty** (`content/bots/difficulty.tsv`
/// rows `GANG_*`). The bar is the hunter's required odds, scaled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GangKnobs {
    /// `[0, 1]`: how readily it fights -- scales the retreat fraction
    /// ([`ABORT_TIMID`]..[`ABORT_RECKLESS`]) and the bait band.
    pub aggression: f64,
    /// How many ships scout (never the FC; at most half the gang).
    pub scouts: usize,
    /// How many jumps from staging the scouts go.
    pub radius: u32,
    /// Present power over the foes' that the gang asks before it engages.
    pub required_odds: f64,
}

impl GangKnobs {
    pub const EASY: GangKnobs = GangKnobs {
        aggression: 0.3,
        scouts: 1,
        radius: 2,
        required_odds: 2.0,
    };
    pub const NORMAL: GangKnobs = GangKnobs {
        aggression: 0.5,
        scouts: 1,
        radius: 3,
        required_odds: 1.5,
    };
    pub const HARD: GangKnobs = GangKnobs {
        aggression: 0.7,
        scouts: 2,
        radius: 4,
        required_odds: 1.3,
    };
    pub const BRUTAL: GangKnobs = GangKnobs {
        aggression: 0.9,
        scouts: 2,
        radius: 5,
        required_odds: 1.1,
    };

    /// Level 1 easy .. 4 brutal; 2 normal; 0 is normal that never takes a
    /// fight (the bar is out of reach), like the peaceful world.
    pub fn at(level: u8) -> GangKnobs {
        match level {
            0 => GangKnobs {
                required_odds: PEACEFUL_ODDS,
                ..Self::NORMAL
            },
            1 => Self::EASY,
            3 => Self::HARD,
            4.. => Self::BRUTAL,
            _ => Self::NORMAL,
        }
    }

    /// The fraction of the bar under which a fight is broken off.
    pub fn abort_fraction(&self) -> f64 {
        lerp(ABORT_TIMID, ABORT_RECKLESS, self.aggression)
    }
}

impl Default for GangKnobs {
    fn default() -> Self {
        Self::NORMAL
    }
}

/// The bar of a peaceful world: no gang reaches it **[A]**.
pub const PEACEFUL_ODDS: f64 = 50.0;

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

// ---- what is seen and said --------------------------------------------------------------------

/// A hostile pilot's ship on a grid, as a member posted it.
#[derive(Debug, Clone, PartialEq)]
pub struct Foe {
    pub ball: i64,
    pub type_id: Option<i64>,
    pub class: Class,
}

impl Foe {
    /// Its [`hunt::power`].
    pub fn power(&self) -> f64 {
        hunt::power(self.class)
    }
}

/// What a member says of itself, once a turn.
#[derive(Debug, Clone)]
pub struct Post {
    pub at: Instant,
    /// `None`: docked.
    pub system: Option<i64>,
    pub class: Class,
    /// A fitted gun and the ship is not a pod.
    pub armed: bool,
    /// The worse of armour and hull, `[0, 1]`; 1 untouched.
    pub health: f64,
    /// The bot is going home to repair.
    pub homing: bool,
    /// Hostile pilots on its grid, within the fight radius.
    pub foes: Vec<Foe>,
    /// The scout's directional scan and grid have shown this system clean.
    pub clean: bool,
    /// The belt it stands at, for the intel line.
    pub belt: Option<i64>,
}

impl Post {
    fn fresh(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.at) <= Duration::from_secs(POST_STALE_S)
    }

    /// In space, armed, whole enough and not going home.
    fn fit(&self) -> bool {
        self.system.is_some() && self.armed && !self.homing && self.health >= FIT_HEALTH
    }
}

/// What the scouts found in one system.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub system: i64,
    pub at: Instant,
    pub scout: String,
    pub foes: Vec<Foe>,
    pub belt: Option<i64>,
}

impl Report {
    /// The foes' power over the doctrine's classes ([`hunt::power`]).
    pub fn power(&self) -> f64 {
        self.foes.iter().map(Foe::power).sum()
    }
}

/// The intel line a scout says in fleet chat **[A]** (hull names are not
/// known to the bot, so the hulls are classes and type ids): `3 in 30000007:
/// small 587, medium 12, small 587 at belt 40000003`.
pub fn intel_line(system: i64, belt: Option<i64>, foes: &[Foe]) -> String {
    let hulls: Vec<String> = foes
        .iter()
        .map(|f| {
            let class = match f.class {
                Class::Capsule => "pod",
                Class::Small => "small",
                Class::Medium => "medium",
                Class::Large => "large",
                Class::Capital => "capital",
                Class::Unknown => "unknown",
            };
            match f.type_id {
                Some(t) => format!("{class} {t}"),
                None => class.to_owned(),
            }
        })
        .collect();
    let at = belt.map(|b| format!(" at belt {b}")).unwrap_or_default();
    format!("{} in {system}: {}{at}", foes.len(), hulls.join(", "))
}

// ---- the decisions (pure) -------------------------------------------------------------------

/// What the FC chooses about a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Nobody there.
    Ignore,
    /// Go, with the committed set.
    Engage,
    /// The gang would clear the bar with the ships that are not here yet.
    WaitForMore,
    /// A fight at long odds: show a ship, let them come to the body.
    Bait,
    /// Too strong: leave the system alone for a while.
    Leave,
}

/// The two powers a decision weighs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Odds {
    /// The fit ships at hand (the body at staging and the reporting scout).
    pub present: f64,
    pub present_ships: usize,
    /// Plus the ships that could be called in: docked repairing, other
    /// scouts.
    pub potential: f64,
    pub foes: f64,
    pub foe_ships: usize,
}

/// **Report to decision** (pure). The bar is [`GangKnobs::required_odds`].
pub fn decide(o: &Odds, k: &GangKnobs) -> Decision {
    if o.foe_ships == 0 || o.foes <= 0.0 {
        return Decision::Ignore;
    }
    let ratio = |p: f64| p / o.foes;
    let bar = k.required_odds;
    if o.present_ships >= MIN_COMMIT && ratio(o.present) >= bar {
        return Decision::Engage;
    }
    if ratio(o.potential) >= bar {
        return Decision::WaitForMore;
    }
    // A bait draws a bigger gang's members apart; it only helps against
    // several, and only at a bar not far off.
    let band = lerp(1.0, BAIT_BAND, k.aggression);
    if o.foe_ships >= 2 && o.present_ships >= MIN_COMMIT && ratio(o.present) >= bar * band {
        return Decision::Bait;
    }
    Decision::Leave
}

/// **The staged commitment** (pure): of `pool` -- `(name, power)` -- the
/// strongest first, the smallest set whose power clears `bar * (1 +
/// COMMIT_MARGIN)` over `foes`, at least [`MIN_COMMIT`]. The rest is the
/// reserve. Empty when even all of the pool does not clear the bar.
pub fn staged(pool: &[(String, f64)], foes: f64, bar: f64) -> Vec<String> {
    let mut sorted: Vec<&(String, f64)> = pool.iter().collect();
    sorted.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let need = foes * bar * (1.0 + COMMIT_MARGIN);
    let mut sum = 0.0;
    let mut out = Vec::new();
    for (name, p) in sorted {
        out.push(name.clone());
        sum += p;
        if out.len() >= MIN_COMMIT && sum >= need {
            return out;
        }
    }
    // Everyone is needed: the bar itself, without the margin, is enough.
    if out.len() >= MIN_COMMIT && sum >= foes * bar {
        out
    } else {
        Vec::new()
    }
}

/// **Whether to break off** (pure): the present power over the foes on grid
/// is under the abort fraction of the bar, or `lost` of `committed` ships
/// reached [`LOSS_RETREAT`].
pub fn should_retreat(
    present: f64,
    foes: f64,
    lost: usize,
    committed: usize,
    k: &GangKnobs,
) -> bool {
    if foes > 0.0 && present / foes < k.required_odds * k.abort_fraction() {
        return true;
    }
    committed > 0 && lost as f64 >= LOSS_RETREAT * committed as f64 && lost >= 1
}

/// **The primary** (pure): the FC's target among the hostile `foes` on the
/// grid. The one already called stays while it is there; else the
/// best [`hunt::score`] for the weakest (a quick kill thins the fire), then
/// the lower ball id. A pod is never a primary.
pub fn choose_primary(
    current: Option<i64>,
    foes: &[Pilot],
    mine: Class,
    ego: &Ball,
    doc: &Ground,
    priority: &[i64],
) -> Option<i64> {
    // BP-COUNTER: a tackler or jammer on the FC is the primary before the
    // one already called, which is the only time a call is dropped.
    let tackler = |p: &Pilot| p.hostile && priority.contains(&p.ball.id);
    if let Some(t) = foes.iter().filter(|p| tackler(p)).min_by(|x, y| {
        ego.surface_to(&x.ball)
            .total_cmp(&ego.surface_to(&y.ball))
            .then(x.ball.id.cmp(&y.ball.id))
    }) && current.is_none_or(|c| !priority.contains(&c))
    {
        return Some(t.ball.id);
    }
    if let Some(c) = current
        && foes.iter().any(|p| p.ball.id == c)
    {
        return Some(c);
    }
    foes.iter()
        .filter(|p| p.hostile && doc.class_of(p.type_id) != Class::Capsule)
        .max_by(|x, y| {
            hunt::score(x, mine, ego, doc, false)
                .total_cmp(&hunt::score(y, mine, ego, doc, false))
                .then(y.ball.id.cmp(&x.ball.id))
        })
        .map(|p| p.ball.id)
}

/// **The re-call** (pure), when the FC cannot see the grid itself: the
/// current primary while any member still sees it, else the weakest foe
/// posted (a quick kill thins the fire), then the lower ball id; never a pod.
pub fn pick_primary(current: Option<i64>, foes: &BTreeMap<i64, Foe>) -> Option<i64> {
    if let Some(c) = current
        && foes.contains_key(&c)
    {
        return Some(c);
    }
    foes.values()
        .filter(|f| f.class != Class::Capsule)
        .min_by(|a, b| a.power().total_cmp(&b.power()).then(a.ball.cmp(&b.ball)))
        .map(|f| f.ball)
}

/// **Who scouts** (pure): `cands` -- `(name, class)` in rank order, the FC
/// left out by the caller. The smallest hulls first (the fastest; the bot
/// cannot read a speed [A]), the later ranks before the earlier, at most
/// `n`, and never more than half the gang.
pub fn pick_scouts(cands: &[(String, Class)], n: usize) -> Vec<String> {
    let n = n.min(cands.len().div_ceil(2)).min(cands.len());
    let mut idx: Vec<usize> = (0..cands.len()).collect();
    idx.sort_by(|&a, &b| {
        hunt::rank(cands[a].1)
            .cmp(&hunt::rank(cands[b].1))
            .then(b.cmp(&a))
    });
    let mut out: Vec<String> = idx
        .into_iter()
        .take(n)
        .map(|i| cands[i].0.clone())
        .collect();
    out.sort();
    out
}

/// **The fan-out** (pure): each scout gets different branches of the gate
/// graph around `staging`, within `radius` jumps. A branch is the set of
/// systems reached through one neighbour first; branches go round-robin,
/// biggest first, so scouts split the neighbourhood instead of following
/// each other. A route visits its systems nearest-first from staging.
/// Only hunting ground, or any system while a war is on.
pub fn assign_routes(
    map: &Map,
    doc: &Ground,
    staging: i64,
    scouts: &[String],
    radius: u32,
    war_on: bool,
) -> BTreeMap<String, Vec<i64>> {
    // BFS with the first hop remembered.
    let mut seen: BTreeMap<i64, (u32, i64)> = BTreeMap::new();
    let mut queue = std::collections::VecDeque::new();
    seen.insert(staging, (0, staging));
    queue.push_back(staging);
    while let Some(s) = queue.pop_front() {
        let (d, first) = seen[&s];
        if d == radius {
            continue;
        }
        for n in map.neighbours(s) {
            if seen.contains_key(&n) {
                continue;
            }
            let first = if s == staging { n } else { first };
            seen.insert(n, (d + 1, first));
            queue.push_back(n);
        }
    }
    let mut branches: BTreeMap<i64, Vec<(u32, i64)>> = BTreeMap::new();
    for (&s, &(d, first)) in &seen {
        if s == staging {
            continue;
        }
        if doc.is_ground(s) || war_on {
            branches.entry(first).or_default().push((d, s));
        }
    }
    let mut order: Vec<(i64, Vec<(u32, i64)>)> = branches.into_iter().collect();
    order.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    let mut routes: BTreeMap<String, Vec<(u32, i64)>> =
        scouts.iter().map(|s| (s.clone(), Vec::new())).collect();
    if scouts.is_empty() {
        return BTreeMap::new();
    }
    for (i, (_, systems)) in order.into_iter().enumerate() {
        let who = &scouts[i % scouts.len()];
        if let Some(r) = routes.get_mut(who) {
            r.extend(systems);
        }
    }
    // GANG-SUMMON: the staging system is searched too (a human at another
    // station or belt of the gang's own system is found): by the first
    // scout, first.
    let first = scouts.first().cloned();
    routes
        .into_iter()
        .map(|(who, mut r)| {
            r.sort();
            let mut route: Vec<i64> = r.into_iter().map(|(_, s)| s).collect();
            if Some(&who) == first.as_ref() {
                route.insert(0, staging);
            }
            (who, route)
        })
        .collect()
}

// ---- the shared state -------------------------------------------------------------------------

/// One member the gang knows.
#[derive(Debug, Clone)]
pub struct Member {
    pub name: String,
    /// The roster rank: 0 is the boss and the FC while it posts.
    pub rank: usize,
    /// Its character, for `WarpToMember` (the driver registers it).
    pub char_id: Option<i64>,
    pub post: Option<Post>,
}

/// **A character the gang comes for** (GANG-SUMMON): hunted when the gang is
/// hostile, joined when it is allied. The system is his, as the server said.
#[derive(Debug, Clone, PartialEq)]
pub struct Quarry {
    pub char_id: i64,
    pub system: i64,
    pub since: Instant,
    /// A fight for him began ([`Plan::Fight`] from [`Plan::Hunt`]): the
    /// quarry is done with when it ends.
    pub engaged: bool,
    /// A member last saw his ship on its grid.
    pub last_seen: Option<Instant>,
    /// The server last refused a warp to him.
    pub refused: Option<Instant>,
}

/// What the gang is doing.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// Undocking and gathering at the home system.
    Form,
    /// Scouts out, the body holds at staging.
    Roam,
    /// The committed ships fly to the scout's system; none lands before all
    /// are in.
    Gather {
        system: i64,
        scout: String,
        committed: Vec<String>,
        since: Instant,
    },
    /// All in: warp to the scout, which tackles.
    Land {
        system: i64,
        scout: String,
        committed: Vec<String>,
        since: Instant,
    },
    /// On grid, shooting the primary.
    Fight {
        system: i64,
        scout: String,
        committed: Vec<String>,
        /// Reserve ships called in (flying to the system, not yet on grid).
        pending: Vec<String>,
        surges: u32,
        since: Instant,
        last_foe: Instant,
    },
    /// Everyone home to repair.
    Withdraw {
        system: i64,
        who: Vec<String>,
        since: Instant,
    },
    /// BP-CAMP: every fit member flies to the camp system, takes the gate and
    /// holds it: no scouting, no chase off the spot, the fight comes to the
    /// gate. Ends when the camp is lifted ([`Gang::lift_camp`]).
    Camp {
        system: i64,
        committed: Vec<String>,
        since: Instant,
    },
    /// GANG-SUMMON: every fit member flies to the quarry's system and warps
    /// to his character; no scouting.
    Hunt {
        system: i64,
        char_id: i64,
        committed: Vec<String>,
        since: Instant,
    },
}

/// What one member is told to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Order {
    /// Stay at (or go back to) staging.
    Hold,
    /// Search the route's systems.
    Scout,
    /// Stay where you are on this grid and keep looking; do not shoot.
    Linger,
    /// Fly to this system (the gate's grid) and wait there.
    Leap(i64),
    /// Warp to the scout in this system.
    Land { system: i64, scout: String },
    /// Shoot the primary.
    Fight { system: i64, primary: Option<i64> },
    /// Lock and point (the tackle modules only) the primary until the body
    /// has landed: the scout does not start the fight alone.
    Tackle { system: i64, primary: Option<i64> },
    /// Go home to repair.
    Withdraw,
    /// GANG-SUMMON: in the quarry's system, warp to his character and stay
    /// on his grid.
    Rally { char_id: i64 },
    /// BP-CAMP: fly to the camp system, take its gate and hold there; shoot
    /// what comes to the grid, never leave it.
    Camp { system: i64 },
}

/// What the FC thinks with.
pub struct Ctx<'a> {
    pub doc: &'a Ground,
    pub map: Option<&'a Map>,
    pub knobs: GangKnobs,
    pub war_on: bool,
}

/// The best engagement so far: its odds ratio, report and the pool.
type Best = (f64, Report, Vec<(String, f64)>);

/// One gang's shared state.
#[derive(Debug, Clone)]
pub struct Gang {
    pub size: usize,
    pub members: Vec<Member>,
    pub plan: Plan,
    pub staging: Option<i64>,
    pub born: Option<Instant>,
    pub scouts: Vec<String>,
    pub routes: BTreeMap<String, Vec<i64>>,
    /// Systems left alone until then.
    pub cooldown: BTreeMap<i64, Instant>,
    pub primary: Option<i64>,
    /// A bait was shown at this moment.
    pub baited: Option<Instant>,
    /// "Waiting for more" was said for a system at this moment.
    pub waited: BTreeMap<i64, Instant>,
    /// The kills the gang made.
    pub fights: u32,
    pub retreats: u32,
    /// GANG-SUMMON: the character the gang comes for, if any.
    pub quarry: Option<Quarry>,
    /// BP-CAMP: the null or wormhole system the gang camps, if any.
    pub camp: Option<i64>,
    /// The first moment a member posted from space (the quarry's form
    /// timer starts here).
    pub first_up: Option<Instant>,
}

impl Gang {
    pub fn new(size: usize) -> Gang {
        Gang {
            size,
            members: Vec::new(),
            plan: Plan::Form,
            staging: None,
            born: None,
            scouts: Vec::new(),
            routes: BTreeMap::new(),
            cooldown: BTreeMap::new(),
            primary: None,
            baited: None,
            waited: BTreeMap::new(),
            fights: 0,
            retreats: 0,
            quarry: None,
            camp: None,
            first_up: None,
        }
    }

    /// BP-CAMP: camp the gate of `system`. The same call again changes
    /// nothing (every bot of a group makes it); another system moves the camp.
    pub fn set_camp(&mut self, system: i64, now: Instant) {
        if self.camp == Some(system) {
            return;
        }
        self.camp = Some(system);
        if let Plan::Camp { committed, .. } = &self.plan {
            self.plan = Plan::Camp {
                system,
                committed: committed.clone(),
                since: now,
            };
        }
    }

    /// BP-CAMP: the camp is over; a camping gang roams again.
    pub fn lift_camp(&mut self) {
        self.camp = None;
    }

    /// GANG-SUMMON: come for `char_id` in `system`. The same call again
    /// changes nothing (every bot of a group makes it); another quarry
    /// replaces the first, and a gang already hunting turns to him.
    pub fn set_quarry(&mut self, char_id: i64, system: i64, now: Instant) {
        if self
            .quarry
            .as_ref()
            .is_some_and(|q| q.char_id == char_id && q.system == system)
        {
            return;
        }
        self.quarry = Some(Quarry {
            char_id,
            system,
            since: now,
            engaged: false,
            last_seen: None,
            refused: None,
        });
        if let Plan::Hunt { committed, .. } = &self.plan {
            self.plan = Plan::Hunt {
                system,
                char_id,
                committed: committed.clone(),
                since: now,
            };
        }
    }

    /// What a member's turn learned of the quarry: his ship was on its grid,
    /// or the server refused its warp to him.
    pub fn note_quarry(&mut self, now: Instant, seen: bool, refused: bool) {
        if let Some(q) = &mut self.quarry {
            if seen {
                q.last_seen = Some(now);
            }
            if refused {
                q.refused = Some(now);
            }
        }
    }

    fn quarry_engaged(&self) -> Option<i64> {
        self.quarry
            .as_ref()
            .filter(|q| q.engaged)
            .map(|q| q.char_id)
    }

    /// The fit members, in rank order, in space anywhere.
    fn fit_names(&self, now: Instant) -> Vec<String> {
        self.members
            .iter()
            .filter(|m| m.post.as_ref().is_some_and(|p| p.fresh(now) && p.fit()))
            .map(|m| m.name.clone())
            .collect()
    }

    /// A member posts.
    pub fn post(&mut self, name: &str, rank: usize, post: Post) {
        if self.born.is_none() {
            self.born = Some(post.at);
        }
        if self.first_up.is_none() && post.system.is_some() {
            self.first_up = Some(post.at);
        }
        match self.members.iter_mut().find(|m| m.name == name) {
            Some(m) => m.post = Some(post),
            None => {
                self.members.push(Member {
                    name: name.to_owned(),
                    rank,
                    char_id: None,
                    post: Some(post),
                });
                self.members.sort_by_key(|m| m.rank);
            }
        }
    }

    fn member(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.name == name)
    }

    fn fresh_post(&self, name: &str, now: Instant) -> Option<&Post> {
        self.member(name)
            .and_then(|m| m.post.as_ref())
            .filter(|p| p.fresh(now))
    }

    /// The FC: the lowest rank still posting from space (a boss that docked
    /// to repair hands the call to the next).
    pub fn fc(&self, now: Instant) -> Option<&str> {
        self.members
            .iter()
            .find(|m| {
                m.post
                    .as_ref()
                    .is_some_and(|p| p.fresh(now) && p.system.is_some())
            })
            .map(|m| m.name.as_str())
    }

    /// The fit members in `system`, in rank order.
    fn fit_in(&self, system: i64, now: Instant) -> Vec<&Member> {
        self.members
            .iter()
            .filter(|m| {
                m.post
                    .as_ref()
                    .is_some_and(|p| p.fresh(now) && p.fit() && p.system == Some(system))
            })
            .collect()
    }

    /// Reports built from the scouts' posts (and anyone's in the engaged
    /// system): the freshest post per system with its foes.
    pub fn reports(&self, now: Instant) -> Vec<Report> {
        let mut by_system: BTreeMap<i64, Report> = BTreeMap::new();
        for m in &self.members {
            let Some(p) = m.post.as_ref().filter(|p| p.fresh(now)) else {
                continue;
            };
            let Some(system) = p.system else { continue };
            if !self.scouts.contains(&m.name)
                && Some(system) != self.staging
                && !self.engaged_in(system)
            {
                continue;
            }
            if p.foes.is_empty() && !p.clean {
                continue;
            }
            let r = Report {
                system,
                at: p.at,
                scout: m.name.clone(),
                foes: p.foes.clone(),
                belt: p.belt,
            };
            // The report with the most foes wins within a tick: a second
            // pair of eyes only adds.
            match by_system.get(&system) {
                Some(old) if old.foes.len() >= r.foes.len() => {}
                _ => {
                    by_system.insert(system, r);
                }
            }
        }
        by_system.into_values().collect()
    }

    fn engaged_in(&self, system: i64) -> bool {
        match &self.plan {
            Plan::Gather { system: s, .. }
            | Plan::Land { system: s, .. }
            | Plan::Fight { system: s, .. } => *s == system,
            _ => false,
        }
    }

    fn on_cooldown(&self, system: i64, now: Instant) -> bool {
        self.cooldown.get(&system).is_some_and(|t| *t > now)
    }

    /// The odds of a report: the body at staging and the reporting scout are
    /// present; the docked and other scouts are potential.
    pub fn odds_of(&self, r: &Report, now: Instant, doc: &Ground) -> (Odds, Vec<(String, f64)>) {
        let mut pool: Vec<(String, f64)> = Vec::new();
        let mut potential = 0.0;
        let power = |p: &Post| hunt::power(p.class);
        for m in &self.members {
            let Some(p) = m.post.as_ref().filter(|p| p.fresh(now)) else {
                continue;
            };
            if p.class == Class::Capsule || !p.armed {
                continue;
            }
            let at_hand = if m.name == r.scout {
                p.fit()
            } else {
                p.fit() && (p.system == self.staging || p.system == Some(r.system))
            };
            if at_hand {
                pool.push((m.name.clone(), power(p)));
            } else {
                potential += power(p);
            }
        }
        let _ = doc;
        let present: f64 = pool.iter().map(|(_, p)| p).sum();
        (
            Odds {
                present,
                present_ships: pool.len(),
                potential: present + potential,
                foes: r.power(),
                foe_ships: r.foes.len(),
            },
            pool,
        )
    }

    /// **The FC's turn** (call it on every member's tick; it acts only when
    /// `me` is the FC). Returns the lines the FC says in fleet chat.
    pub fn think(&mut self, me: &str, now: Instant, c: &Ctx<'_>) -> Vec<String> {
        if self.fc(now) != Some(me) {
            return Vec::new();
        }
        let mut said = Vec::new();
        self.cooldown.retain(|_, t| *t > now);
        match self.plan.clone() {
            Plan::Form => self.think_form(me, now, c, &mut said),
            Plan::Roam if self.quarry.is_some() => self.start_hunt(now, &mut said),
            Plan::Roam if self.camp.is_some() => self.start_camp(now, &mut said),
            Plan::Camp { .. } => self.think_camp(now, &mut said),
            Plan::Roam => self.think_roam(now, c, &mut said),
            Plan::Hunt { .. } => self.think_hunt(now, &mut said),
            Plan::Gather {
                system,
                scout,
                committed,
                since,
            } => self.think_gather(system, scout, committed, since, now, c, &mut said),
            Plan::Land {
                system,
                scout,
                committed,
                since,
            } => self.think_land(system, scout, committed, since, now, c, &mut said),
            Plan::Fight { .. } => self.think_fight(now, c, &mut said),
            Plan::Withdraw { system, who, since } => {
                self.think_withdraw(system, &who, since, now, &mut said);
            }
        }
        said
    }

    fn think_form(&mut self, me: &str, now: Instant, c: &Ctx<'_>, said: &mut Vec<String>) {
        let Some(fc_system) = self
            .fresh_post(me, now)
            .and_then(|p| p.system.filter(|_| p.armed))
        else {
            return;
        };
        let up = self.fit_in(fc_system, now).len();
        let waited = self.born.is_some_and(|b| {
            now.saturating_duration_since(b) >= Duration::from_secs(FORM_TIMEOUT_S)
        });
        // GANG-SUMMON: with a quarry, half the gang up is enough, and so is
        // one ship once [`QUARRY_FORM_S`] have passed since the first.
        let quarried = self.quarry.is_some()
            && (up >= self.size.div_ceil(2)
                || (up >= 1
                    && self.first_up.is_some_and(|t| {
                        now.saturating_duration_since(t) >= Duration::from_secs(QUARRY_FORM_S)
                    })));
        if up >= self.size || (waited && up >= MIN_COMMIT) || quarried {
            self.staging = Some(fc_system);
            self.plan = Plan::Roam;
            self.assign(now, c);
            if self.quarry.is_some() {
                said.push(format!("Staging in {fc_system}, {up} ships"));
                self.start_hunt(now, said);
            } else {
                said.push(format!("Staging in {fc_system}, {up} ships; scouts out"));
            }
        }
    }

    /// GANG-SUMMON: the plan is to come for the quarry with every fit ship.
    fn start_hunt(&mut self, now: Instant, said: &mut Vec<String>) {
        let Some(q) = self.quarry.clone() else { return };
        let committed = self.fit_names(now);
        said.push(format!(
            "On the way to {}: {} ships",
            q.system,
            committed.len()
        ));
        self.plan = Plan::Hunt {
            system: q.system,
            char_id: q.char_id,
            committed,
            since: now,
        };
    }

    /// BP-CAMP: the plan is to hold the camp system's gate with every fit
    /// ship.
    fn start_camp(&mut self, now: Instant, said: &mut Vec<String>) {
        let Some(system) = self.camp else { return };
        let committed = self.fit_names(now);
        said.push(format!(
            "Camping the gate in {system}: {} ships",
            committed.len()
        ));
        self.plan = Plan::Camp {
            system,
            committed,
            since: now,
        };
    }

    /// BP-CAMP: the camp stands until it is lifted, a quarry takes over, or
    /// it moves; the committed set follows the fit ships.
    fn think_camp(&mut self, now: Instant, said: &mut Vec<String>) {
        let Plan::Camp { system, since, .. } = self.plan.clone() else {
            return;
        };
        if self.quarry.is_some() || self.camp.is_none() {
            self.plan = Plan::Roam;
            said.push("Leaving the gate".to_owned());
            return;
        }
        let want = self.camp.unwrap_or(system);
        self.plan = Plan::Camp {
            system: want,
            committed: self.fit_names(now),
            since,
        };
    }

    /// GANG-SUMMON: foes on a member's grid in the quarry's system start the
    /// fight at once (the owner asked for it, so no [`decide`]); a quarry no
    /// one has seen whose warp is refused is dropped and the gang roams.
    fn think_hunt(&mut self, now: Instant, said: &mut Vec<String>) {
        let Plan::Hunt { system, since, .. } = self.plan.clone() else {
            return;
        };
        let Some(q) = self.quarry.clone() else {
            self.plan = Plan::Roam;
            return;
        };
        let fit = self.fit_names(now);
        let scout = self
            .fit_in(system, now)
            .into_iter()
            .find(|m| m.post.as_ref().is_some_and(|p| !p.foes.is_empty()))
            .map(|m| m.name.clone());
        if let Some(scout) = scout {
            let committed: Vec<String> = self
                .fit_in(system, now)
                .into_iter()
                .map(|m| m.name.clone())
                .filter(|n| *n != scout)
                .collect();
            if let Some(q) = &mut self.quarry {
                q.engaged = true;
            }
            self.baited = None;
            self.plan = Plan::Fight {
                system,
                scout,
                committed,
                pending: Vec::new(),
                surges: 0,
                since: now,
                last_foe: now,
            };
            said.push(format!(
                "Contact in {system}: {} of ours on the grid, engaging",
                self.committed_len() + 1
            ));
            return;
        }
        let lost_for = Duration::from_secs(QUARRY_LOST_S);
        let unseen = q
            .last_seen
            .is_none_or(|t| now.saturating_duration_since(t) >= lost_for);
        let refused = q.refused.is_some_and(|t| t >= q.since);
        if refused && unseen && now.saturating_duration_since(since) >= lost_for {
            self.quarry = None;
            self.plan = Plan::Roam;
            said.push(format!("Lost the trail in {system}, back to roaming"));
            return;
        }
        self.plan = Plan::Hunt {
            system,
            char_id: q.char_id,
            committed: fit,
            since,
        };
    }

    /// Pick the scouts and give them routes.
    fn assign(&mut self, now: Instant, c: &Ctx<'_>) {
        let Some(staging) = self.staging else { return };
        let fc = self.fc(now).map(str::to_owned);
        let cands: Vec<(String, Class)> = self
            .members
            .iter()
            .filter(|m| Some(&m.name) != fc.as_ref())
            .filter_map(|m| {
                m.post
                    .as_ref()
                    .filter(|p| p.fresh(now) && p.armed)
                    .map(|p| (m.name.clone(), p.class))
            })
            .collect();
        self.scouts = pick_scouts(&cands, c.knobs.scouts);
        self.routes = match c.map {
            Some(map) => assign_routes(map, c.doc, staging, &self.scouts, c.knobs.radius, c.war_on),
            None => BTreeMap::new(),
        };
    }

    fn think_roam(&mut self, now: Instant, c: &Ctx<'_>, said: &mut Vec<String>) {
        // The scouts are kept while they live: a scout in flight posts
        // nothing for a while ([`POST_STALE_S`]); only a lost one is replaced.
        let lost = |s: &String| {
            !self
                .member(s)
                .and_then(|m| m.post.as_ref())
                .is_some_and(|p| p.armed)
        };
        if self.scouts.is_empty() || self.scouts.iter().any(lost) {
            self.assign(now, c);
        }
        let Some(staging) = self.staging else { return };
        let ttl = Duration::from_secs(REPORT_TTL_S);
        let mut best: Option<Best> = None;
        let mut wait: Option<Report> = None;
        let mut bait: Option<Report> = None;
        for r in self.reports(now) {
            if r.foes.is_empty()
                || now.saturating_duration_since(r.at) > ttl
                || self.on_cooldown(r.system, now)
            {
                continue;
            }
            let (odds, pool) = self.odds_of(&r, now, c.doc);
            match decide(&odds, &c.knobs) {
                Decision::Engage => {
                    let ratio = odds.present / odds.foes;
                    if best.as_ref().is_none_or(|(b, _, _)| ratio > *b) {
                        best = Some((ratio, r, pool));
                    }
                }
                Decision::WaitForMore => wait = wait.or(Some(r)),
                Decision::Bait => bait = bait.or(Some(r)),
                Decision::Leave => {
                    self.cooldown
                        .insert(r.system, now + Duration::from_secs(LEAVE_COOLDOWN_S));
                    said.push(format!(
                        "{} in {}: too many, leaving it",
                        r.foes.len(),
                        r.system
                    ));
                }
                Decision::Ignore => {}
            }
        }
        if let Some((_, r, pool)) = best {
            let committed = staged(&pool, r.power(), c.knobs.required_odds);
            let committed: Vec<String> = committed.into_iter().filter(|n| *n != r.scout).collect();
            // The scout is on the grid already; at least one more ship goes
            // with it ([`MIN_COMMIT`] counts the scout).
            if committed.is_empty() {
                return;
            }
            self.baited = None;
            if r.system == staging {
                self.plan = Plan::Fight {
                    system: r.system,
                    scout: r.scout.clone(),
                    committed,
                    pending: Vec::new(),
                    surges: 0,
                    since: now,
                    last_foe: now,
                };
            } else {
                self.plan = Plan::Gather {
                    system: r.system,
                    scout: r.scout.clone(),
                    committed,
                    since: now,
                };
            }
            said.push(format!(
                "Engaging {} in {}: {} of ours, going together",
                r.foes.len(),
                r.system,
                self.committed_len() + 1
            ));
            return;
        }
        if let Some(r) = bait {
            match self.baited {
                None => {
                    self.baited = Some(now);
                    said.push(format!(
                        "{} in {}: baiting with {}, body holds",
                        r.foes.len(),
                        r.system,
                        r.scout
                    ));
                }
                Some(t) if now.saturating_duration_since(t) >= Duration::from_secs(BAIT_S) => {
                    // The bait did not thin them: leave it.
                    self.baited = None;
                    self.cooldown
                        .insert(r.system, now + Duration::from_secs(LEAVE_COOLDOWN_S));
                    said.push(format!(
                        "{} in {}: bait failed, leaving it",
                        r.foes.len(),
                        r.system
                    ));
                }
                Some(_) => {}
            }
            return;
        }
        self.baited = None;
        if let Some(r) = wait {
            let due = self.waited.get(&r.system).is_none_or(|t| {
                now.saturating_duration_since(*t) >= Duration::from_secs(SAY_EVERY_S)
            });
            if due {
                self.waited.insert(r.system, now);
                said.push(format!(
                    "{} in {}: waiting for more",
                    r.foes.len(),
                    r.system
                ));
            }
        }
    }

    fn committed_len(&self) -> usize {
        match &self.plan {
            Plan::Gather { committed, .. }
            | Plan::Land { committed, .. }
            | Plan::Fight { committed, .. } => committed.len(),
            _ => 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn think_gather(
        &mut self,
        system: i64,
        scout: String,
        mut committed: Vec<String>,
        since: Instant,
        now: Instant,
        c: &Ctx<'_>,
        said: &mut Vec<String>,
    ) {
        let abort = |g: &mut Gang, why: &str, said: &mut Vec<String>| {
            g.plan = Plan::Roam;
            g.cooldown
                .insert(system, now + Duration::from_secs(LEAVE_COOLDOWN_S / 4));
            said.push(format!("{system}: {why}, standing down"));
        };
        // Ships that went away (docked, lost, silent) are dropped.
        committed.retain(|n| self.fresh_post(n, now).is_some_and(Post::fit));
        let report = self.reports(now).into_iter().find(|r| r.system == system);
        let Some(report) = report.filter(|r| !r.foes.is_empty()) else {
            abort(self, "target gone", said);
            return;
        };
        if committed.is_empty() || self.fresh_post(&scout, now).is_none() {
            abort(self, "too few ships", said);
            return;
        }
        // The bar again with what is committed plus the scout.
        let mine: f64 = committed
            .iter()
            .chain(std::iter::once(&scout))
            .filter_map(|n| self.fresh_post(n, now))
            .map(|p| hunt::power(p.class))
            .sum();
        let odds = Odds {
            present: mine,
            present_ships: committed.len() + 1,
            potential: mine,
            foes: report.power(),
            foe_ships: report.foes.len(),
        };
        if decide(&odds, &c.knobs) != Decision::Engage {
            abort(self, "they grew", said);
            return;
        }
        if now.saturating_duration_since(since) > Duration::from_secs(GATHER_TIMEOUT_S) {
            abort(self, "gathering took too long", said);
            return;
        }
        let all_in = committed.iter().all(|n| {
            self.fresh_post(n, now)
                .is_some_and(|p| p.system == Some(system))
        });
        if all_in {
            // Pick the primary from what the scout sees, so the scout
            // tackles it as the body lands.
            let seen: BTreeMap<i64, Foe> =
                report.foes.iter().map(|f| (f.ball, f.clone())).collect();
            self.primary = pick_primary(None, &seen);
            self.plan = Plan::Land {
                system,
                scout: scout.clone(),
                committed,
                since: now,
            };
            said.push(format!("All in {system}: landing on {scout}"));
        } else {
            self.plan = Plan::Gather {
                system,
                scout,
                committed,
                since,
            };
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn think_land(
        &mut self,
        system: i64,
        scout: String,
        mut committed: Vec<String>,
        since: Instant,
        now: Instant,
        c: &Ctx<'_>,
        said: &mut Vec<String>,
    ) {
        committed.retain(|n| self.fresh_post(n, now).is_some_and(Post::fit));
        let landed = |g: &Gang, n: &String| {
            g.fresh_post(n, now)
                .is_some_and(|p| p.system == Some(system) && !p.foes.is_empty())
        };
        let on_grid = committed.iter().filter(|n| landed(self, n)).count();
        let late = now.saturating_duration_since(since) > Duration::from_secs(LAND_TIMEOUT_S);
        if on_grid == committed.len() || late {
            if on_grid + 1 < MIN_COMMIT || on_grid == 0 {
                self.begin_withdraw(system, &scout, &committed, now, said, "landing failed");
                return;
            }
            let _ = c;
            self.plan = Plan::Fight {
                system,
                scout,
                committed,
                pending: Vec::new(),
                surges: 0,
                since: now,
                last_foe: now,
            };
        } else {
            self.plan = Plan::Land {
                system,
                scout,
                committed,
                since,
            };
        }
    }

    fn begin_withdraw(
        &mut self,
        system: i64,
        scout: &str,
        committed: &[String],
        now: Instant,
        said: &mut Vec<String>,
        why: &str,
    ) {
        let mut who: Vec<String> = committed.to_vec();
        who.push(scout.to_owned());
        self.retreats += 1;
        // A retreat ends this fight, not the owner's hunt: the quarry stays
        // listed, so the gang comes back for him once its ships are repaired
        // (`Plan::Roam` with a quarry starts the hunt over).
        if let Some(q) = &mut self.quarry {
            q.engaged = false;
        }
        self.primary = None;
        self.cooldown
            .insert(system, now + Duration::from_secs(LEAVE_COOLDOWN_S));
        self.plan = Plan::Withdraw {
            system,
            who,
            since: now,
        };
        said.push(format!("{why} in {system} -- everyone out"));
    }

    fn think_fight(&mut self, now: Instant, c: &Ctx<'_>, said: &mut Vec<String>) {
        let Plan::Fight {
            system,
            scout,
            mut committed,
            mut pending,
            mut surges,
            since,
            mut last_foe,
        } = self.plan.clone()
        else {
            return;
        };
        // A ship on the grid system and still in the fight (armed, not going
        // home); health is not asked: a hurt ship fights until it leaves.
        let here = |g: &Gang, n: &String| {
            g.fresh_post(n, now).is_some_and(|p| {
                p.system == Some(system) && p.armed && !p.homing && p.class != Class::Capsule
            })
        };
        // Reserve ships that are all in the system join together.
        if !pending.is_empty() && pending.iter().all(|n| here(self, n)) {
            committed.append(&mut pending);
        }
        // GANG-SUMMON: for the owner's fight every ship that arrives joins as
        // it comes (a 150-ship gang does not land together).
        if self.quarry_engaged().is_some() {
            let joined: Vec<String> = self
                .fit_in(system, now)
                .into_iter()
                .map(|m| m.name.clone())
                .filter(|n| *n != scout && !committed.contains(n))
                .collect();
            committed.extend(joined);
        }
        let all: Vec<String> = committed
            .iter()
            .cloned()
            .chain(std::iter::once(scout.clone()))
            .collect();
        let alive: Vec<&String> = all.iter().filter(|n| here(self, n)).collect();
        let lost = all.len() - alive.len();
        // Foes on any of our grids, by ball.
        let mut foes: BTreeMap<i64, Foe> = BTreeMap::new();
        for n in &all {
            if let Some(p) = self.fresh_post(n, now)
                && p.system == Some(system)
            {
                for f in &p.foes {
                    foes.insert(f.ball, f.clone());
                }
            }
        }
        let foe_power: f64 = foes.values().map(Foe::power).sum();
        let present: f64 = alive
            .iter()
            .filter_map(|n| self.fresh_post(n, now))
            .map(|p| hunt::power(p.class))
            .sum();
        if !foes.is_empty() {
            last_foe = now;
        }
        // Re-call: the primary died or warped off.
        self.primary = pick_primary(self.primary, &foes);
        // Won: no foe on any grid for a while.
        // GANG-SUMMON: in the owner's fight the foe may have warped off while
        // the rest are still in warp to him: the same wait as a lost trail.
        let clear_s = if self.quarry_engaged().is_some() {
            QUARRY_LOST_S
        } else {
            CLEAR_S
        };
        if foes.is_empty()
            && now.saturating_duration_since(last_foe) >= Duration::from_secs(clear_s)
        {
            self.fights += 1;
            self.primary = None;
            self.plan = Plan::Roam;
            self.cooldown.remove(&system);
            if self.quarry_engaged().is_some() {
                self.quarry = None;
            }
            said.push(format!("{system} clear -- regrouping"));
            return;
        }
        // The owner's fight is not weighed: the first ships to land would read
        // as losing before the rest are in. It still ends on losses.
        let weighed = if self.quarry_engaged().is_some() {
            0.0
        } else {
            foe_power
        };
        if should_retreat(present, weighed, lost, all.len(), &c.knobs) {
            let why = format!(
                "Losing (lost {lost} of {}, power {present:.1} against {weighed:.1})",
                all.len()
            );
            self.begin_withdraw(system, &scout, &committed, now, said, &why);
            return;
        }
        // A surge: the fight is under the bar and a reserve exists.
        if foe_power > 0.0
            && present / foe_power < c.knobs.required_odds
            && pending.is_empty()
            && surges < crate::doctrine_theatre::SURGE_MAX
        {
            let reserve: Vec<String> = self
                .members
                .iter()
                .filter(|m| !all.contains(&m.name) && !self.scouts.contains(&m.name))
                .filter(|m| {
                    m.post
                        .as_ref()
                        .is_some_and(|p| p.fresh(now) && p.fit() && p.system == self.staging)
                })
                .map(|m| m.name.clone())
                .collect();
            if !reserve.is_empty() {
                surges += 1;
                said.push(format!(
                    "Surge {surges}: {} more ships coming",
                    reserve.len()
                ));
                pending = reserve;
            }
        }
        self.plan = Plan::Fight {
            system,
            scout,
            committed,
            pending,
            surges,
            since,
            last_foe,
        };
    }

    fn think_withdraw(
        &mut self,
        _system: i64,
        who: &[String],
        since: Instant,
        now: Instant,
        said: &mut Vec<String>,
    ) {
        let home = |g: &Gang, n: &String| {
            g.fresh_post(n, now)
                .is_none_or(|p| p.system.is_none() || p.system == g.staging)
        };
        let done = who.iter().all(|n| home(self, n));
        let late = now.saturating_duration_since(since) > Duration::from_secs(WITHDRAW_TIMEOUT_S);
        if done || late {
            self.plan = Plan::Roam;
            said.push("Regrouped -- back to roaming".to_owned());
        }
    }

    /// **The FC's call on the primary** (the FC passes its own view): keeps
    /// the one called while it is on the grid, else picks and sets a new one.
    /// Returns a line when the call changes.
    pub fn call_primary(
        &mut self,
        foes: &[Pilot],
        mine: Class,
        ego: &Ball,
        doc: &Ground,
        priority: &[i64],
    ) -> Option<i64> {
        let pick = choose_primary(self.primary, foes, mine, ego, doc, priority);
        if pick != self.primary {
            self.primary = pick;
        }
        pick
    }

    /// **What `name` is told to do** (pure over the state).
    pub fn order(&self, name: &str, now: Instant) -> Order {
        let is_scout = self.scouts.iter().any(|s| s == name);
        let order = match &self.plan {
            Plan::Form => Order::Hold,
            Plan::Roam => {
                if is_scout {
                    Order::Scout
                } else {
                    Order::Hold
                }
            }
            Plan::Gather {
                system,
                scout,
                committed,
                ..
            } => {
                if name == scout {
                    Order::Linger
                } else if committed.iter().any(|n| n == name) {
                    Order::Leap(*system)
                } else if is_scout {
                    Order::Scout
                } else {
                    Order::Hold
                }
            }
            Plan::Land {
                system,
                scout,
                committed,
                ..
            } => {
                if name == scout {
                    // The scout tackles what it sees as the body warps in.
                    Order::Tackle {
                        system: *system,
                        primary: self.primary,
                    }
                } else if committed.iter().any(|n| n == name) {
                    Order::Land {
                        system: *system,
                        scout: scout.clone(),
                    }
                } else if is_scout {
                    Order::Scout
                } else {
                    Order::Hold
                }
            }
            Plan::Fight {
                system,
                scout,
                committed,
                pending,
                ..
            } => {
                if name == scout || committed.iter().any(|n| n == name) {
                    Order::Fight {
                        system: *system,
                        primary: self.primary,
                    }
                } else if pending.iter().any(|n| n == name) {
                    // Fly in together; the fight order comes when they are
                    // all in and the FC moves them into the committed set.
                    Order::Leap(*system)
                } else if let Some(char_id) = self.quarry_engaged() {
                    // The owner's fight: whoever is not in it yet comes.
                    self.come_order(name, *system, char_id, now)
                } else if is_scout {
                    Order::Scout
                } else {
                    Order::Hold
                }
            }
            Plan::Withdraw { who, .. } => {
                if who.iter().any(|n| n == name) {
                    Order::Withdraw
                } else if is_scout {
                    Order::Scout
                } else {
                    Order::Hold
                }
            }
            Plan::Hunt {
                system, char_id, ..
            } => self.come_order(name, *system, *char_id, now),
            Plan::Camp { system, .. } => Order::Camp { system: *system },
        };
        // A scout with a live report in its system stays on it instead of
        // moving on (the FC may be weighing it).
        if order == Order::Scout && self.lingers(name, now) {
            Order::Linger
        } else {
            order
        }
    }

    /// GANG-SUMMON: fly to the quarry's system, then warp to his character.
    fn come_order(&self, name: &str, system: i64, char_id: i64, now: Instant) -> Order {
        match self.fresh_post(name, now).and_then(|p| p.system) {
            Some(here) if here == system => Order::Rally { char_id },
            _ => Order::Leap(system),
        }
    }

    /// A scout lingers while its own system holds a live report the FC has
    /// not decided to leave.
    fn lingers(&self, name: &str, now: Instant) -> bool {
        let Some(p) = self.fresh_post(name, now) else {
            return false;
        };
        let Some(system) = p.system else { return false };
        !p.foes.is_empty() && !self.on_cooldown(system, now)
    }
}

/// The gangs of the process: one lock, a state per gang id.
#[derive(Debug, Default)]
pub struct GangHall {
    gangs: Mutex<BTreeMap<String, Gang>>,
}

impl GangHall {
    pub fn new() -> GangHall {
        GangHall::default()
    }

    /// Run `f` on gang `id` (made with `size` members on first use).
    pub fn with<R>(&self, id: &str, size: usize, f: impl FnOnce(&mut Gang) -> R) -> R {
        let mut all = self.gangs.lock().unwrap_or_else(|e| e.into_inner());
        f(all.entry(id.to_owned()).or_insert_with(|| Gang::new(size)))
    }

    /// A copy of gang `id`'s state, for tests and logs.
    pub fn snapshot(&self, id: &str) -> Option<Gang> {
        self.gangs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }
}

/// One bot's seat in a gang ([`crate::brain::Policy::gang`]).
#[derive(Debug, Clone)]
pub struct Seat {
    pub hall: Arc<GangHall>,
    /// The gang id (the roster group).
    pub gang: String,
    /// The roster rank: 0 is the boss.
    pub rank: usize,
    /// How many bots the gang has.
    pub size: usize,
    pub knobs: GangKnobs,
}

impl PartialEq for Seat {
    fn eq(&self, o: &Seat) -> bool {
        self.gang == o.gang && self.rank == o.rank && self.knobs == o.knobs
    }
}

/// What a bot remembers of its gang duty. Never world state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mind {
    /// The system the scout's belt visits belong to.
    pub at_system: Option<i64>,
    /// Where on the route the scout is.
    pub leg: usize,
    /// The sweep of this system showed nothing to find.
    pub clean: bool,
    /// Belts looked at here.
    pub looked: Vec<i64>,
    /// The intel line last said, and when.
    pub said: Option<(String, Instant)>,
    /// The landing warp was ordered at this moment.
    pub landed_at: Option<Instant>,
    /// The belt last warped to.
    pub belt: Option<i64>,
    /// The fly-in last failed (so a trip is not retried every turn).
    pub fly_failed: u32,
}

// ---- the acts ---------------------------------------------------------------------------------

/// What the gang asks of the driver beyond the shared acts.
#[derive(Debug, Clone, PartialEq)]
pub enum GangAct {
    /// Fly over the gates to this system and stay in space on the arrival
    /// grid (a scout's leg, a leap, the way back to staging).
    Fly { system: i64 },
}

impl GangAct {
    pub fn describe(&self) -> (String, Vec<i64>) {
        match self {
            GangAct::Fly { system } => (format!("gang: fly to system {system}"), vec![*system]),
        }
    }
}

/// **Fly to `system`** over the gates at any security, one gate at a time
/// as [`crate::travel::go`] does, and stop on the arrival grid in space.
/// Returns the system the bot ends in.
pub fn fly(c: &mut crate::Conn, map: &Map, system: i64) -> crate::Result<i64> {
    use crate::travel::{self, Aim, Here, Move};
    let start = c
        .session()
        .solar_system_id2()
        .ok_or_else(|| crate::Error::NotSent("gang: no system".to_owned()))?;
    let Some(jumps) = map.jumps(start, system, ANY_SECURITY) else {
        return Err(crate::Error::NotSent(format!(
            "gang: no route from {start} to {system}"
        )));
    };
    let budget = travel::MOVES_PER_SYSTEM * (jumps as usize + 1);
    for _ in 0..budget {
        let deadline = Instant::now() + travel::TRAVEL_CEILING;
        let now_in = c
            .session()
            .solar_system_id2()
            .ok_or_else(|| crate::Error::NotSent("gang: no system".to_owned()))?;
        if now_in == system && c.session().station_id().is_none() {
            return Ok(now_in);
        }
        let aim = match map.route(now_in, system, ANY_SECURITY) {
            Some(r) if r.len() >= 2 => match map.gate(now_in, r[1]) {
                Some((gate, partner)) => Aim::Gate {
                    gate,
                    partner,
                    to: r[1],
                },
                None => Aim::Unrouted,
            },
            _ => Aim::Unrouted,
        };
        let ball = match aim {
            Aim::Gate { gate, .. } => Some(gate),
            _ => None,
        };
        let here = Here {
            docked_at: c.session().station_id(),
            bound: c.ballpark().is_some() && c.world().has_state(),
            warping: c.world_now().ego_ball().is_some_and(|b| b.warp_commanded()),
            gap: ball.and_then(|b| {
                let w = c.world_now();
                w.ego.and_then(|e| w.surface_distance(e, b))
            }),
        };
        // `station` -1 is never a station: docked anywhere means undock.
        let step = travel::next_move(&here, -1, aim);
        match step {
            Move::Undock => {
                c.undock_repaired()?;
            }
            Move::Bind => {
                let sys = c
                    .session()
                    .solar_system_id()
                    .ok_or_else(|| crate::Error::NotSent("gang: in space with no system".into()))?;
                if c.ballpark().is_none() {
                    c.bind_ballpark(sys)?;
                }
                pump_until(c, deadline, |c| c.world().has_state())?;
            }
            Move::Wait => pump_until(c, deadline, |c| {
                !c.world_now().ego_ball().is_some_and(|b| b.warp_commanded())
            })?,
            Move::Warp(id) => {
                c.warp_to_item(id, 0.0)?;
                pump_until(c, deadline, |c| {
                    let w = c.world_now();
                    !w.ego_ball().is_some_and(|b| b.warp_commanded())
                        && w.ego
                            .and_then(|e| w.surface_distance(e, id))
                            .is_some_and(|g| g < crate::MIN_WARP_DISTANCE)
                })?;
            }
            Move::Approach(id) => {
                c.follow(id, 0.0)?;
                pump_until(c, deadline, |c| {
                    let w = c.world_now();
                    w.ego
                        .and_then(|e| w.surface_distance(e, id))
                        .is_some_and(|g| g < eve_server::jump::MAX_JUMP_DISTANCE)
                })?;
            }
            Move::Jump { gate, partner } => {
                c.stargate_jump(gate, partner)?;
            }
            Move::NotHere(id) => {
                return Err(crate::Error::NotSent(format!(
                    "gang: {id} is not in system {now_in}'s park"
                )));
            }
            Move::NoRoute | Move::Dock(_) | Move::Arrived => {
                return Err(crate::Error::NotSent(format!(
                    "gang: no route from {now_in} to {system}"
                )));
            }
        }
    }
    Err(crate::Error::Refused(format!(
        "gang: not in {system} after {budget} moves"
    )))
}

fn pump_until(
    c: &mut crate::Conn,
    deadline: Instant,
    mut done: impl FnMut(&mut crate::Conn) -> bool,
) -> crate::Result<()> {
    while !done(c) {
        if Instant::now() >= deadline {
            return Err(crate::Error::Refused(
                "gang: still moving after the travel ceiling".into(),
            ));
        }
        c.pump(Duration::from_millis(250))?;
    }
    Ok(())
}

// ---- the role -----------------------------------------------------------------------------------

/// A hunter in a gang: the hunter's kit, and a seat in a [`GangHall`].
pub static ROLE: RoleSpec = RoleSpec {
    name: "roamgang",
    greeting: "o7 -- gang up",
    civilian: false,
    docked,
    in_space,
    banner,
    setup,
};

/// **The opt-in**: a gang hunts pilots, so it needs `--society-pvp` like
/// the hunter.
pub fn check_opt_in(
    roles: impl IntoIterator<Item = brain::RoleRef>,
    opted_in: bool,
) -> std::result::Result<(), String> {
    if !opted_in && roles.into_iter().any(|r| *r == ROLE) {
        return Err(format!(
            "--loop {}: a gang that hunts pilots is opt-in; add --society-pvp",
            ROLE.name
        ));
    }
    Ok(())
}

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, kills {}, reconnects {}",
        s.name, s.phase, s.kills, s.reconnects
    )
}

fn setup(lacks: Lacks) -> Vec<String> {
    let mut lines = vec![format!("/giveskill {} 1", escort::SKILL_GUNNERY)];
    if lacks.ship {
        lines.push(format!("/createitem {}", escort::RIFTER));
    }
    if lacks.module {
        lines.push(format!("/createitem {} {}", escort::GUN, escort::WANT_GUNS));
        lines.push(format!("/giveskill {} 1", hunt::SKILL_PROPULSION_JAMMING));
        lines.push(format!("/createitem {}", hunt::SCRAMBLER));
        lines.push(format!("/createitem {}", hunt::STASIS_WEBIFIER));
    }
    if lacks.ammo {
        lines.push(format!(
            "/createitem {} {}",
            escort::AMMO,
            escort::SETUP_ROUNDS
        ));
    }
    lines
}

/// This bot's `Post` from its observation.
fn post_of(m: &Memory, o: &Observation, doc: &Ground, system: Option<i64>, clean: bool) -> Post {
    let mine = match doc.class_of(o.ship.and_then(|s| s.type_id)) {
        Class::Unknown => Class::Small,
        c => c,
    };
    let health = o
        .health
        .map_or(1.0, |h| h.armour.min(h.hull).clamp(0.0, 1.0));
    let ego = o.park.as_ref().map(|p| p.ego);
    let foes: Vec<Foe> = match ego {
        Some(ego) if system.is_some() => o
            .hunt
            .pilots
            .iter()
            .filter(|p| p.hostile && ego.surface_to(&p.ball) <= hunt::NEAR)
            .map(|p| Foe {
                ball: p.ball.id,
                type_id: p.type_id,
                class: doc.class_of(p.type_id),
            })
            .collect(),
        _ => Vec::new(),
    };
    Post {
        at: o.now,
        system,
        class: mine,
        armed: !o.guns.is_empty() && mine != Class::Capsule,
        health,
        homing: m.homing.is_some(),
        foes,
        clean,
        belt: m.gang.belt,
    }
}

fn say(o: &Observation, line: String) -> Vec<Act> {
    if o.chat {
        vec![Act::Say(line)]
    } else {
        Vec::new()
    }
}

/// Docked: the hunter's kit and repairs, then undock with the gang.
fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let Some(seat) = m.policy.gang.clone() else {
        return (hunt::ROLE.docked)(m, o);
    };
    let Some(doc) = m.policy.pvp.clone() else {
        return vec![
            Act::Missing(hunt::MISSING_PVP),
            Act::Wait(brain::MISSING_WAIT),
        ];
    };
    if !m.hunt.safety_set {
        m.hunt.safety_set = true;
        return vec![Act::Hunt(HuntAct::Safety)];
    }
    // The gang knows a docked member, so a repair is not a loss.
    let post = post_of(m, o, &doc, None, false);
    seat.hall
        .with(&seat.gang, seat.size, |g| g.post(&m.name, seat.rank, post));
    m.hunt.visited.clear();
    m.hunt.prey = None;
    m.hunt.gone = None;
    let due = m
        .hunt
        .settled_at
        .is_none_or(|t| o.now.saturating_duration_since(t) >= hunt::SETTLE_EVERY);
    if m.hunt.unsettled || due {
        m.hunt.settled_at = Some(o.now);
        m.hunt.unsettled = false;
        let allies = std::mem::take(&mut m.hunt.allies);
        return vec![Act::Hunt(HuntAct::Settle { allies })];
    }
    let acts = escort::docked(m, o);
    if !acts.contains(&Act::Undock) {
        return acts;
    }
    // The first pass is [Stock, Undock]: the prep rides with each fit, or the
    // driver undocks before the kit is fitted.
    let prep: Vec<Act> = acts
        .iter()
        .filter(|a| **a != Act::Undock)
        .cloned()
        .collect();
    if let Some(item) = o.hangar.iter().find(|r| {
        hunt::is_tackle(r.type_id)
            && !m.hunt.tackle_tried.contains(&r.item_id)
            && !o
                .guns
                .iter()
                .any(|g| hunt::tackle_kind(g.type_id) == hunt::tackle_kind(r.type_id))
    }) {
        m.hunt.tackle_tried.push(item.item_id);
        return [prep, vec![Act::FitAuto(item.item_id)]].concat();
    }
    // BP-KIT, nothing from nowhere: a smartbomb or drive in the hangar.
    if let Some(act) = crate::pvp_kit::stock_to_fit(m, o) {
        return [prep, vec![act]].concat();
    }
    if crate::camp::kit_pending(m, o) {
        return [prep, vec![Act::Wait(brain::SHORT_WAIT)]].concat();
    }
    acts
}

/// In space: post, let the FC think, and act on the order.
fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let (Some(park), Place::InSpace { system, .. }) = (&o.park, o.place) else {
        return vec![Act::Wait(brain::SHORT_WAIT)];
    };
    let (Some(seat), Some(doc)) = (m.policy.gang.clone(), m.policy.pvp.clone()) else {
        return (hunt::ROLE.in_space)(m, o);
    };
    // BP-CAMP: a field left running is stopped before any order.
    if let Some(act) = crate::camp::field_check(m, o) {
        return vec![act];
    }
    if !m.hunt.safety_set {
        m.hunt.safety_set = true;
        return vec![Act::Hunt(HuntAct::Safety)];
    }
    // The ship is hurt or unarmed: home, whatever the gang does.
    let mut homing_note: Option<String> = None;
    if m.homing.is_none() {
        if o.guns.is_empty() {
            m.homing = Some(Homing::Unarmed);
            homing_note = Some("Homing: unarmed (no gun fitted)".to_owned());
        } else if let Some(h) = o
            .health
            .filter(|h| h.armour < escort::REPAIR_ARMOUR || h.hull < GANG_REPAIR_HULL)
        {
            m.homing = Some(Homing::Repair);
            homing_note = Some(format!(
                "Homing: repair (armour {:.2}, hull {:.2}, {} gun(s))",
                h.armour,
                h.hull,
                o.guns.len()
            ));
        }
    }
    if m.gang.at_system != Some(system) {
        m.gang.at_system = Some(system);
        m.gang.looked.clear();
        m.gang.clean = false;
        m.gang.belt = None;
        m.hunt.visited.clear();
    }
    let mut said: Vec<Act> = Vec::new();
    if let Some(note) = homing_note {
        said.extend(say(o, note));
    }
    hunt_book(m, o, &mut said);

    // 1. Post what I see; 2. the FC thinks; 3. my order.
    let post = post_of(m, o, &doc, Some(system), m.gang.clean);
    let ctx_map = m.policy.travel.clone();
    let knobs = seat.knobs;
    let war_on = o.hunt.war_on;
    let name = m.name.clone();
    let (order, lines, staging) = seat.hall.with(&seat.gang, seat.size, |g| {
        g.post(&name, seat.rank, post.clone());
        if let Some(q) = g.quarry.as_ref().map(|q| q.char_id) {
            let seen = o.hunt.pilots.iter().any(|p| p.char_id == q);
            let refused = o.boss_unreachable_until.is_some_and(|t| o.now < t);
            g.note_quarry(o.now, seen, refused);
        }
        let ctx = Ctx {
            doc: &doc,
            map: ctx_map.as_deref(),
            knobs,
            war_on,
        };
        let lines = g.think(&name, o.now, &ctx);
        // The FC calls the primary from its own grid while a fight is on.
        if matches!(g.plan, Plan::Fight { .. } | Plan::Land { .. }) && g.fc(o.now) == Some(&name) {
            let ego = park.ego;
            let mine = post.class;
            g.call_primary(&o.hunt.pilots, mine, &ego, &doc, &o.hunt.priority);
        }
        (g.order(&name, o.now), lines, g.staging)
    });
    if o.chat {
        for l in lines {
            said.push(Act::Say(l));
        }
    }
    let acts = act_on(m, o, park, system, &doc, &seat, staging, order);
    // NW-11: no warp of the gang's passes through a known bubble.
    let acts = crate::warp_bubble::guard(acts, o);
    [said, acts].concat()
}

/// The prey bookkeeping the hunter does, for the gang's primary.
fn hunt_book(m: &mut Memory, o: &Observation, said: &mut Vec<Act>) {
    hunt::book_prey(m, o, said);
}

#[allow(clippy::too_many_arguments)]
fn act_on(
    m: &mut Memory,
    o: &Observation,
    park: &ParkView,
    system: i64,
    doc: &Ground,
    seat: &Seat,
    staging: Option<i64>,
    order: Order,
) -> Vec<Act> {
    // BP-EWAR-AIM: out of a fight, a scramble or a bubble first (see
    // `hunt::in_space`).
    let fighting =
        matches!(order, Order::Fight { .. } | Order::Tackle { .. }) && m.homing.is_none();
    // NW-11b: the camp is over; the unit it anchored is taken back if safe.
    if !fighting
        && !matches!(order, Order::Camp { .. })
        && let Some(act) = crate::camp::unit_retrieve(m, o, true)
    {
        return vec![act];
    }
    if !fighting
        && let Some(acts) = crate::scram::decide(m, o).or_else(|| crate::warp_bubble::decide(m, o))
    {
        return acts;
    }
    // Going home beats any order but a fight in progress.
    if m.homing.is_some() {
        return hold(m, o, system, staging);
    }
    match order {
        Order::Hold => hold(m, o, system, staging),
        Order::Withdraw => {
            m.homing = Some(Homing::Repair);
            m.target = None;
            m.hunt.prey = None;
            let mut acts = say(o, "Withdrawing".to_owned());
            acts.extend(hold(m, o, system, staging));
            acts
        }
        Order::Linger => {
            // Keep eyes on the grid; nothing is shot. A scout that is hit
            // goes home on its own (the health rule above).
            let mut acts = intel(m, o, system, doc);
            acts.push(Act::Wait(brain::SHORT_WAIT));
            acts
        }
        Order::Leap(target) => {
            if system == target {
                return vec![Act::Wait(brain::SHORT_WAIT)];
            }
            if m.gang.fly_failed >= 3 {
                return hold(m, o, system, staging);
            }
            vec![Act::Gang(GangAct::Fly { system: target })]
        }
        Order::Land {
            system: target,
            scout,
        } => {
            if system != target {
                return vec![Act::Gang(GangAct::Fly { system: target })];
            }
            if !o.hunt.pilots.iter().any(|p| p.hostile) {
                // Not on the grid yet: warp to the scout, once per landing
                // window and again if it did not take.
                let char_id = seat.hall.with(&seat.gang, seat.size, |g| {
                    g.member(&scout).and_then(|mm| mm.char_id)
                });
                let due = m
                    .gang
                    .landed_at
                    .is_none_or(|t| o.now.saturating_duration_since(t) >= Duration::from_secs(20));
                if let (Some(char_id), true, false) = (char_id, due, park.ego_warping) {
                    m.gang.landed_at = Some(o.now);
                    return vec![Act::WarpToMember {
                        char_id,
                        range: escort::WARP_STANDOFF,
                    }];
                }
                return vec![Act::Wait(brain::SHORT_WAIT)];
            }
            vec![Act::Wait(brain::SHORT_WAIT)]
        }
        Order::Fight {
            system: target,
            primary,
        } => {
            if system != target {
                return vec![Act::Gang(GangAct::Fly { system: target })];
            }
            let prey =
                primary.and_then(|id| o.hunt.pilots.iter().find(|p| p.ball.id == id && p.hostile));
            match prey {
                Some(p) => {
                    let p = p.clone();
                    let acts = hunt::fight(m, o, park, &p, Vec::new());
                    hunt::guard_with(acts, o, Some(p.ball.id), &crate::ewar_aim::legal_ids(m, o))
                }
                None => {
                    // Primary dead or off the grid: the FC re-calls; until
                    // then nobody shoots anything else.
                    m.target = None;
                    // GANG-SUMMON: in the owner's fight a ship that has not
                    // found him on its grid yet warps to him.
                    let quarry = seat
                        .hall
                        .with(&seat.gang, seat.size, |g| g.quarry_engaged());
                    match quarry {
                        Some(char_id) if !o.hunt.pilots.iter().any(|p| p.hostile) => {
                            rally(m, o, park, char_id)
                        }
                        _ => hunt::guard(vec![Act::Wait(brain::SHORT_WAIT)], o, None),
                    }
                }
            }
        }
        Order::Tackle {
            system: target,
            primary,
        } => {
            if system != target {
                return vec![Act::Gang(GangAct::Fly { system: target })];
            }
            let prey =
                primary.and_then(|id| o.hunt.pilots.iter().find(|p| p.ball.id == id && p.hostile));
            let Some(p) = prey.cloned() else {
                return vec![Act::Wait(brain::SHORT_WAIT)];
            };
            // The fight's own moves, but only the tackle modules fire.
            let acts = hunt::fight(m, o, park, &p, Vec::new());
            let acts = tackle_only(acts, &o.guns);
            hunt::guard_with(acts, o, Some(p.ball.id), &crate::ewar_aim::legal_ids(m, o))
        }
        Order::Scout => scout(m, o, park, system, doc, seat),
        Order::Rally { char_id } => rally(m, o, park, char_id),
        Order::Camp { system: target } => hold_gate(m, o, park, system, target),
    }
}

/// **Hold the gate** (BP-CAMP): in another system, fly there. On the grid
/// with a hostile ship: the fight's own acts with every move dropped
/// ([`crate::camp::hold_position`]), so a camper shoots, launches and runs
/// its field without leaving the spot. Nobody there: the camp's own upkeep,
/// then the gate ([`crate::camp::post`]), then wait.
fn hold_gate(
    m: &mut Memory,
    o: &Observation,
    park: &ParkView,
    system: i64,
    target: i64,
) -> Vec<Act> {
    if system != target {
        if m.gang.fly_failed >= 3 {
            return vec![Act::Wait(brain::SHORT_WAIT)];
        }
        return vec![Act::Gang(GangAct::Fly { system: target })];
    }
    // NW-11b: a unit the camp owns goes out at the gate and is anchored.
    if let Some(act) = crate::camp::unit_deploy(m, o) {
        return vec![act];
    }
    let prey = o
        .hunt
        .pilots
        .iter()
        .filter(|p| p.hostile)
        .min_by(|a, b| {
            park.ego
                .surface_to(&a.ball)
                .total_cmp(&park.ego.surface_to(&b.ball))
        })
        .cloned();
    if let Some(p) = prey {
        let acts = hunt::fight(m, o, park, &p, Vec::new());
        let acts = crate::camp::hold_position(acts);
        return hunt::guard_with(acts, o, Some(p.ball.id), &crate::ewar_aim::legal_ids(m, o));
    }
    m.target = None;
    let upkeep = crate::camp::quiet(m, o);
    if !upkeep.is_empty() {
        return upkeep;
    }
    if let Some(a) = crate::camp::post(m, o, park, system) {
        return vec![a];
    }
    hunt::guard(vec![Act::Wait(brain::SHORT_WAIT)], o, None)
}

/// **Warp to the quarry and stay on his grid** (GANG-SUMMON). Not on his
/// grid, or on it but far: warp to his character, no oftener than
/// [`RALLY_EVERY_S`] and not while in warp; near enough: wait (the FC sees
/// the foes on the grid and calls the fight, or an allied gang just stays).
fn rally(m: &mut Memory, o: &Observation, park: &ParkView, char_id: i64) -> Vec<Act> {
    let pilot = o.hunt.pilots.iter().find(|p| p.char_id == char_id);
    // His ship unseen, the range is the gang's own side's, not "hostile" by
    // default: an allied gang landing at 10 km is out of the fight.
    let hostile_side = pilot.map_or(m.policy.hostile.is_some(), |p| p.hostile);
    let range = if hostile_side {
        RALLY_HOSTILE_RANGE
    } else {
        RALLY_ALLIED_RANGE
    };
    let near = pilot.is_some_and(|p| {
        let gap = park.ego.surface_to(&p.ball);
        if p.hostile {
            gap <= hunt::NEAR
        } else {
            gap <= range * 2.0
        }
    });
    let due = m
        .gang
        .landed_at
        .is_none_or(|t| o.now.saturating_duration_since(t) >= Duration::from_secs(RALLY_EVERY_S));
    if near || park.ego_warping || !due {
        return vec![Act::Wait(brain::SHORT_WAIT)];
    }
    m.gang.landed_at = Some(o.now);
    vec![Act::WarpToMember { char_id, range }]
}

/// **Only the tackle fires** (pure): the shots of guns that are not tackle
/// modules are dropped; everything else stays.
pub fn tackle_only(acts: Vec<Act>, guns: &[brain::GunInfo]) -> Vec<Act> {
    acts.into_iter()
        .filter(|a| match a {
            Act::Fire { gun, .. } => guns.iter().any(|g| g.item_id == *gun && g.kind.is_tackle()),
            // The tackler does not bomb (BP-KIT).
            Act::Kit(crate::pvp_kit::KitAct::Bomb(_)) => false,
            _ => true,
        })
        .collect()
}

/// Stay at (or go back to) staging; go home when going home.
fn hold(m: &mut Memory, o: &Observation, system: i64, staging: Option<i64>) -> Vec<Act> {
    if let Some(s) = staging
        && s != system
        && m.gang.fly_failed < 3
    {
        return vec![Act::Gang(GangAct::Fly { system: s })];
    }
    if m.homing.is_some() {
        return escort::in_space(m, o);
    }
    vec![Act::Wait(brain::SHORT_WAIT)]
}

/// The intel line, when it is new.
fn intel(m: &mut Memory, o: &Observation, system: i64, doc: &Ground) -> Vec<Act> {
    let foes: Vec<Foe> = o
        .hunt
        .pilots
        .iter()
        .filter(|p| p.hostile)
        .map(|p| Foe {
            ball: p.ball.id,
            type_id: p.type_id,
            class: doc.class_of(p.type_id),
        })
        .collect();
    if foes.is_empty() {
        return Vec::new();
    }
    let line = intel_line(system, m.gang.belt, &foes);
    let key = format!("{}:{}", system, foes.len());
    let stale = m.gang.said.as_ref().is_none_or(|(k, t)| {
        *k != key || o.now.saturating_duration_since(*t) >= Duration::from_secs(SAY_EVERY_S)
    });
    if stale {
        m.gang.said = Some((key, o.now));
        return say(o, line);
    }
    Vec::new()
}

/// A scout's turn: its route, the d-scan in each system, the belt that
/// sees pilots; a hostile on the grid is a report (the post), not a fight.
fn scout(
    m: &mut Memory,
    o: &Observation,
    park: &ParkView,
    system: i64,
    doc: &Ground,
    seat: &Seat,
) -> Vec<Act> {
    let name = m.name.clone();
    let route: Vec<i64> = seat
        .hall
        .with(&seat.gang, seat.size, |g| g.routes.get(&name).cloned())
        .unwrap_or_default();
    if route.is_empty() {
        return vec![Act::Wait(brain::SHORT_WAIT)];
    }
    let target = route[m.gang.leg % route.len()];
    if system != target {
        if m.gang.fly_failed >= 3 {
            // Unreachable: skip the system.
            m.gang.leg += 1;
            m.gang.fly_failed = 0;
            return vec![Act::Wait(brain::SHORT_WAIT)];
        }
        return vec![Act::Gang(GangAct::Fly { system: target })];
    }
    // In the target system: hostile pilots on the grid are the report.
    let mut acts = intel(m, o, system, doc);
    if o.hunt.pilots.iter().any(|p| p.hostile) {
        acts.push(Act::Wait(brain::SHORT_WAIT));
        return acts;
    }
    // Otherwise: scan, go to the belt that sees pilots, else move on.
    let fresh = o
        .hunt
        .sweep
        .as_ref()
        .filter(|s| s.fresh(o.now, system, &park.ego));
    let Some(sweep) = fresh else {
        let belts: Vec<(i64, [f64; 3])> = park
            .belts
            .iter()
            .filter(|b| park.ego.surface_to(b) >= crate::MIN_WARP_DISTANCE)
            .map(|b| (b.id, b.position))
            .collect();
        if belts.is_empty() || m.hunt.scan_failed >= hunt::SCAN_TRIES {
            m.gang.clean = true;
            m.gang.leg += 1;
            return vec![Act::Wait(brain::SHORT_WAIT)];
        }
        return vec![Act::Hunt(HuntAct::DScan {
            belts,
            ego: park.ego.position,
            system,
        })];
    };
    // The sweep counts every pilot's ship in the sphere, own included.
    let _ = doc;
    if sweep.ships == 0 {
        m.gang.clean = true;
        m.gang.leg += 1;
        m.hunt.visited.clear();
        return vec![Act::Wait(brain::SHORT_WAIT)];
    }
    match hunt::pick_belt(sweep, &m.hunt.visited, park) {
        Some(belt) => {
            m.hunt.visited.push(belt.id);
            m.gang.belt = Some(belt.id);
            vec![Act::WarpToItem {
                id: belt.id,
                range: escort::WARP_STANDOFF,
            }]
        }
        None => {
            // Everything the scan pointed at was looked at: next system.
            m.gang.clean = true;
            m.gang.leg += 1;
            m.hunt.visited.clear();
            vec![Act::Wait(brain::SHORT_WAIT)]
        }
    }
}

/// **Come for a character** (GANG-SUMMON): the seat's gang hunts (hostile)
/// or joins (allied) `char_id`, who is in `system`. Every bot of a group
/// calls it; the first one counts.
pub fn summon(seat: &Seat, system: i64, char_id: i64) {
    seat.hall.with(&seat.gang, seat.size, |g| {
        g.set_quarry(char_id, system, Instant::now());
    });
}

/// **Camp a gate** (BP-CAMP): the seat's gang holds the gate of `system`
/// (null or wormhole; the bot checks the band, the server refuses elsewhere).
/// Every bot of a group calls it; the first one counts.
pub fn camp(seat: &Seat, system: i64) {
    seat.hall.with(&seat.gang, seat.size, |g| {
        g.set_camp(system, Instant::now());
    });
}

/// A member's character id, registered by the driver once logged in.
pub fn register(seat: &Seat, name: &str, char_id: i64) {
    seat.hall.with(&seat.gang, seat.size, |g| {
        match g.members.iter_mut().find(|m| m.name == name) {
            Some(m) => m.char_id = Some(char_id),
            None => {
                g.members.push(Member {
                    name: name.to_owned(),
                    rank: seat.rank,
                    char_id: Some(char_id),
                    post: None,
                });
                g.members.sort_by_key(|m| m.rank);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    //! The gang's decisions over invented ids and the constants by name: no
    //! [A] number is an expected value.
    use super::*;
    use crate::hunt::Ground;
    use std::collections::BTreeSet;

    const STAGING: i64 = 30_001_000;
    const A: i64 = 30_001_001;
    const B: i64 = 30_001_002;
    const C: i64 = 30_001_003;
    const A2: i64 = 30_001_011;
    const B2: i64 = 30_001_012;
    const HIGH: i64 = 30_001_099;
    const SMALL: i64 = 9_101;
    const MEDIUM: i64 = 9_102;
    const LARGE: i64 = 9_103;
    const POD: i64 = 9_104;

    fn doc() -> Ground {
        Ground::new()
            .with_security(STAGING, 0.2)
            .with_security(A, 0.1)
            .with_security(B, -0.1)
            .with_security(C, 0.3)
            .with_security(A2, -0.3)
            .with_security(B2, 0.0)
            .with_security(HIGH, 0.9)
            .with_class(SMALL, Class::Small)
            .with_class(MEDIUM, Class::Medium)
            .with_class(LARGE, Class::Large)
            .with_class(POD, Class::Capsule)
    }

    /// STAGING -- A -- A2, STAGING -- B -- B2, STAGING -- C, STAGING -- HIGH.
    fn map() -> Map {
        let mut m = Map::new();
        let mut gate = 80_000_000;
        for (x, y) in [
            (STAGING, A),
            (A, A2),
            (STAGING, B),
            (B, B2),
            (STAGING, C),
            (STAGING, HIGH),
        ] {
            m = m
                .with_gate(x, gate, y, gate + 1)
                .with_gate(y, gate + 1, x, gate);
            gate += 2;
        }
        m
    }

    fn foe(ball: i64, hull: i64) -> Foe {
        let class = doc().class_of(Some(hull));
        Foe {
            ball,
            type_id: Some(hull),
            class,
        }
    }

    fn post(at: Instant, system: Option<i64>, class: Class) -> Post {
        Post {
            at,
            system,
            class,
            armed: true,
            health: 1.0,
            homing: false,
            foes: Vec::new(),
            clean: false,
            belt: None,
        }
    }

    fn ctx<'a>(d: &'a Ground, m: &'a Map, knobs: GangKnobs) -> Ctx<'a> {
        Ctx {
            doc: d,
            map: Some(m),
            knobs,
            war_on: false,
        }
    }

    fn secs(t: Instant, s: u64) -> Instant {
        t + Duration::from_secs(s)
    }

    // ---- the fan-out ----

    #[test]
    fn scouts_take_different_branches_inside_the_radius_and_stay_on_ground() {
        let (d, m) = (doc(), map());
        let scouts = vec!["s1".to_owned(), "s2".to_owned()];
        let routes = assign_routes(&m, &d, STAGING, &scouts, 2, false);
        // The first scout looks at the staging system first (checked in its
        // own test); the rest of the routes are the branches.
        let strip = |r: &Vec<i64>| {
            r.iter()
                .copied()
                .filter(|s| *s != STAGING)
                .collect::<Vec<_>>()
        };
        let (r1, r2) = (&strip(&routes["s1"]), &strip(&routes["s2"]));
        assert!(
            r1.iter().all(|s| !r2.contains(s)),
            "disjoint: {r1:?} {r2:?}"
        );
        let all: BTreeSet<i64> = r1.iter().chain(r2).copied().collect();
        assert_eq!(
            all,
            BTreeSet::from([A, B, C, A2, B2]),
            "all ground, all in reach"
        );
        assert!(!all.contains(&HIGH), "high sec is left alone without a war");
        assert!(!all.contains(&STAGING));
        // A branch is kept whole: A and A2 are one scout's.
        assert_eq!(r1.contains(&A), r1.contains(&A2));
        // Nearest first within a route.
        for r in [r1, r2] {
            let depth = |s: &i64| m.jumps(STAGING, *s, ANY_SECURITY).unwrap();
            assert!(r.windows(2).all(|w| depth(&w[0]) <= depth(&w[1])), "{r:?}");
        }
        // The radius bounds the reach; a war opens high sec.
        let near = assign_routes(&m, &d, STAGING, &scouts, 1, false);
        let near_all: BTreeSet<i64> = near
            .values()
            .flatten()
            .copied()
            .filter(|s| *s != STAGING)
            .collect();
        assert_eq!(near_all, BTreeSet::from([A, B, C]));
        let war = assign_routes(&m, &d, STAGING, &scouts, 1, true);
        assert!(war.values().flatten().any(|s| *s == HIGH));
        // Same inputs, same routes.
        assert_eq!(routes, assign_routes(&m, &d, STAGING, &scouts, 2, false));
    }

    #[test]
    fn the_first_scout_looks_at_the_staging_system_first() {
        let (d, m) = (doc(), map());
        let scouts = vec!["s1".to_owned(), "s2".to_owned()];
        let routes = assign_routes(&m, &d, STAGING, &scouts, 2, false);
        assert_eq!(routes["s1"].first(), Some(&STAGING));
        assert!(!routes["s2"].contains(&STAGING));
        // One scout alone gets it too; none, no routes.
        let one = assign_routes(&m, &d, STAGING, &scouts[..1], 1, false);
        assert_eq!(one["s1"].first(), Some(&STAGING));
        assert!(assign_routes(&m, &d, STAGING, &[], 1, false).is_empty());
    }

    // ---- GANG-SUMMON: coming for a character ----

    /// The quarry's character (invented) and his system.
    const WHO: i64 = 90_001_777;

    /// A gang of four, `up` of them in space at staging (small, armed).
    fn four(t: Instant, up: usize) -> Gang {
        let mut g = Gang::new(4);
        for (i, name) in ["fc", "b1", "b2", "b3"].iter().enumerate() {
            g.post(name, i, post(t, (i < up).then_some(STAGING), Class::Small));
        }
        g
    }

    #[test]
    fn a_gang_with_a_quarry_forms_at_half_strength_and_never_waits_for_the_slowest() {
        let (d, m) = (doc(), map());
        let c = ctx(&d, &m, GangKnobs::NORMAL);
        let t = Instant::now();
        // Without a quarry two of four are not enough, soon after.
        let mut g = four(t, 2);
        g.think("fc", t, &c);
        assert_eq!(g.plan, Plan::Form);
        // With one, half is: the plan is the hunt, to his system.
        g.set_quarry(WHO, A, t);
        let said = g.think("fc", t, &c);
        assert!(
            matches!(&g.plan, Plan::Hunt { system, char_id, committed, .. }
                if *system == A && *char_id == WHO && committed.len() == 2),
            "{:?}",
            g.plan
        );
        assert!(!said.is_empty());
        assert_eq!(g.staging, Some(STAGING), "the body still has a staging");
        // One ship up forms it after the timer and not before.
        let mut g = four(t, 1);
        g.set_quarry(WHO, A, t);
        g.think("fc", secs(t, QUARRY_FORM_S - 1), &c);
        assert_eq!(g.plan, Plan::Form);
        g.think("fc", secs(t, QUARRY_FORM_S), &c);
        assert!(matches!(g.plan, Plan::Hunt { .. }), "{:?}", g.plan);
        // The same summon again changes nothing; another one turns the hunt.
        let before = g.quarry.clone();
        g.set_quarry(WHO, A, secs(t, 5));
        assert_eq!(g.quarry, before);
        g.set_quarry(WHO, B, secs(t, 6));
        assert!(matches!(&g.plan, Plan::Hunt { system, .. } if *system == B));
    }

    #[test]
    fn a_hunt_flies_to_his_system_then_warps_to_his_character() {
        let t = Instant::now();
        let mut g = four(t, 2);
        g.post("b2", 2, post(t, Some(A), Class::Small));
        g.set_quarry(WHO, A, t);
        g.plan = Plan::Hunt {
            system: A,
            char_id: WHO,
            committed: vec![],
            since: t,
        };
        assert_eq!(g.order("fc", t), Order::Leap(A), "elsewhere: fly");
        assert_eq!(
            g.order("b2", t),
            Order::Rally { char_id: WHO },
            "there: warp"
        );
        // A member still docked is told to leap too (it acts when it is up).
        assert_eq!(g.order("b3", t), Order::Leap(A));
    }

    /// BP-CAMP: a formed gang with a camp set holds the gate; a quarry takes
    /// the plan over, and lifting the camp sends it roaming again.
    #[test]
    fn a_gang_with_a_camp_holds_the_gate_until_a_quarry_or_the_end() {
        let (d, m) = (doc(), map());
        let t = Instant::now();
        let mut g = four(t, 4);
        g.set_camp(B2, t);
        g.set_camp(B2, t + Duration::from_secs(1)); // the same call again: nothing
        let c = ctx(&d, &m, GangKnobs::NORMAL);
        // Formed: staged, then the camp (the quarry-less Roam goes to Camp).
        g.think("fc", t, &c);
        assert_eq!(g.plan, Plan::Roam, "formed and staged first");
        let said = g.think("fc", t, &c);
        assert!(said.iter().any(|l| l.contains("Camping")), "{said:?}");
        assert!(
            matches!(&g.plan, Plan::Camp { system, committed, .. }
                if *system == B2 && committed.len() == 4),
            "{:?}",
            g.plan
        );
        for n in ["fc", "b1", "b3"] {
            assert_eq!(g.order(n, t), Order::Camp { system: B2 });
        }
        // The camp moves.
        g.set_camp(A2, t);
        assert!(matches!(&g.plan, Plan::Camp { system, .. } if *system == A2));
        // A quarry beats the camp; the camp stays set for after the hunt.
        g.set_quarry(WHO, A, t);
        g.think("fc", t, &c);
        assert_eq!(g.plan, Plan::Roam);
        assert!(g.camp.is_some());
        g.think("fc", t, &c);
        assert!(matches!(g.plan, Plan::Hunt { .. }), "{:?}", g.plan);
        // Lifted: nothing camps.
        let mut h = four(t, 4);
        h.set_camp(B2, t);
        h.think("fc", t, &c);
        h.lift_camp();
        h.think("fc", t, &c);
        assert_eq!(h.plan, Plan::Roam);
    }

    #[test]
    fn foes_on_his_grid_start_the_fight_even_when_the_odds_say_leave() {
        let (d, m) = (doc(), map());
        let c = ctx(&d, &m, GangKnobs::NORMAL);
        let t = Instant::now();
        let mut g = Gang::new(2);
        let mut p = post(t, Some(A), Class::Small);
        p.foes = vec![foe(700, LARGE)];
        g.post("fc", 0, p);
        g.post("b1", 1, post(t, Some(A), Class::Small));
        g.staging = Some(STAGING);
        g.plan = Plan::Roam;
        // As a report this is a "leave".
        let report = g.reports(t);
        let _ = report;
        let odds = Odds {
            present: 2.0 * hunt::power(Class::Small),
            present_ships: 2,
            potential: 2.0 * hunt::power(Class::Small),
            foes: hunt::power(Class::Large),
            foe_ships: 1,
        };
        assert_eq!(decide(&odds, &c.knobs), Decision::Leave);
        g.set_quarry(WHO, A, t);
        g.think("fc", t, &c);
        assert!(matches!(g.plan, Plan::Hunt { .. }), "{:?}", g.plan);
        g.think("fc", t, &c);
        assert!(
            matches!(&g.plan, Plan::Fight { system, scout, committed, .. }
                if *system == A && scout == "fc" && committed == &vec!["b1".to_owned()]),
            "{:?}",
            g.plan
        );
        // Outgunned on landing, it stays: only losses end the owner's fight.
        g.think("fc", t, &c);
        assert!(matches!(g.plan, Plan::Fight { .. }), "{:?}", g.plan);
        // The ship that has not found him yet still warps to him.
        g.post("b2", 2, post(t, Some(A), Class::Small));
        assert_eq!(
            g.order("b2", t),
            Order::Rally { char_id: WHO },
            "joins the owner's fight as it arrives"
        );
        // And it clears with the quarry when the fight is won (nobody on
        // any grid for as long as a lost trail takes).
        let later = secs(t, QUARRY_LOST_S + 1);
        g.post("fc", 0, post(later, Some(A), Class::Small));
        g.post("b1", 1, post(later, Some(A), Class::Small));
        g.think("fc", later, &c);
        g.think("fc", later, &c);
        assert_eq!(g.plan, Plan::Roam);
        assert_eq!(g.quarry, None);
    }

    #[test]
    fn a_rally_to_an_unseen_pilot_takes_the_range_of_the_gangs_own_side() {
        let t = Instant::now();
        let park = ParkView {
            ego: Ball {
                id: 1,
                position: [0.0; 3],
                radius: 10.0,
            },
            ego_warping: false,
            rocks: Vec::new(),
            belts: Vec::new(),
            station: None,
            boss: None,
            taken: std::collections::BTreeMap::new(),
            hostiles: Vec::new(),
            wrecks: Vec::new(),
            ego_tether: None,
        };
        let range_of = |hostile: bool| {
            let mut m = Memory::new("b1", brain::Policy::fast());
            if hostile {
                m.policy.hostile = Some(std::sync::Arc::new(crate::hostile::Rules::default()));
            }
            let o = Observation::empty(t);
            match rally(&mut m, &o, &park, WHO).as_slice() {
                [Act::WarpToMember { range, .. }] => *range,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(range_of(true), RALLY_HOSTILE_RANGE);
        assert_eq!(range_of(false), RALLY_ALLIED_RANGE);
    }

    #[test]
    fn an_allied_gang_never_fights_the_quarry_and_a_lost_one_is_dropped() {
        let (d, m) = (doc(), map());
        let c = ctx(&d, &m, GangKnobs::NORMAL);
        let t = Instant::now();
        // He is allied, so he is on nobody's foe list: nothing to fight.
        let mut g = Gang::new(2);
        g.post("fc", 0, post(t, Some(A), Class::Small));
        g.post("b1", 1, post(t, Some(A), Class::Small));
        g.staging = Some(STAGING);
        g.plan = Plan::Roam;
        g.set_quarry(WHO, A, t);
        g.think("fc", t, &c);
        for k in 1..5 {
            let at = secs(t, k);
            g.post("fc", 0, post(at, Some(A), Class::Small));
            g.post("b1", 1, post(at, Some(A), Class::Small));
            g.note_quarry(at, true, false);
            g.think("fc", at, &c);
            assert!(matches!(g.plan, Plan::Hunt { .. }), "{:?}", g.plan);
            assert_eq!(g.order("b1", at), Order::Rally { char_id: WHO });
        }
        // The server refuses the warp and nobody has seen him for a while.
        let gone = secs(t, 5 + QUARRY_LOST_S + 1);
        g.post("fc", 0, post(gone, Some(A), Class::Small));
        g.post("b1", 1, post(gone, Some(A), Class::Small));
        g.note_quarry(gone, false, true);
        g.think("fc", gone, &c);
        assert_eq!(g.plan, Plan::Roam);
        assert_eq!(g.quarry, None);
    }

    #[test]
    fn the_fastest_hulls_scout_never_more_than_half_and_the_fc_is_left_to_the_caller() {
        let cands: Vec<(String, Class)> = vec![
            ("g2".into(), Class::Medium),
            ("g3".into(), Class::Small),
            ("g4".into(), Class::Small),
            ("g5".into(), Class::Large),
        ];
        assert_eq!(
            pick_scouts(&cands, 1),
            vec!["g4".to_owned()],
            "smallest, later rank first"
        );
        assert_eq!(
            pick_scouts(&cands, 2),
            vec!["g3".to_owned(), "g4".to_owned()]
        );
        assert_eq!(pick_scouts(&cands, 9).len(), 2, "at most half of four");
        assert!(pick_scouts(&[], 2).is_empty());
        assert_eq!(pick_scouts(&cands[..1], 2).len(), 1);
    }

    // ---- the report, then the decision ----

    fn odds(present: f64, ships: usize, potential: f64, foes: f64, foe_ships: usize) -> Odds {
        Odds {
            present,
            present_ships: ships,
            potential,
            foes,
            foe_ships,
        }
    }

    #[test]
    fn a_report_becomes_engage_wait_bait_leave_or_ignore() {
        let k = GangKnobs::NORMAL;
        let bar = k.required_odds;
        // Nobody there.
        assert_eq!(decide(&odds(9.0, 3, 9.0, 0.0, 0), &k), Decision::Ignore);
        // Clears the bar with the ships at hand.
        assert_eq!(
            decide(&odds(bar * 4.0, 3, bar * 4.0, 4.0, 2), &k),
            Decision::Engage
        );
        // Does not clear it now but would with the ships on their way.
        assert_eq!(
            decide(&odds(bar * 2.0, 2, bar * 4.0, 4.0, 2), &k),
            Decision::WaitForMore
        );
        // Under the bar with several foes, inside the bait band.
        let band = lerp(1.0, BAIT_BAND, k.aggression);
        assert_eq!(
            decide(
                &odds(bar * 4.0 * band * 1.01, 3, bar * 4.0 * band * 1.01, 4.0, 3),
                &k
            ),
            Decision::Bait
        );
        // Hopeless.
        assert_eq!(decide(&odds(1.0, 3, 1.0, 20.0, 5), &k), Decision::Leave);
        // A single foe is never a bait.
        assert_eq!(
            decide(&odds(bar * 0.9, 3, bar * 0.9, 1.0, 1), &k),
            Decision::Leave
        );
        // One ship alone is not a fight, however weak the foe: it waits for
        // company if company would do, else leaves.
        assert_eq!(
            decide(&odds(9.0, 1, 9.0, 1.0, 1), &k),
            Decision::WaitForMore
        );
        assert_eq!(decide(&odds(1.0, 1, 1.0, 1.0, 1), &k), Decision::Leave);
    }

    #[test]
    fn the_peaceful_world_never_engages_and_a_harder_level_asks_less() {
        let calm = GangKnobs::at(0);
        assert_eq!(decide(&odds(30.0, 6, 30.0, 3.0, 1), &calm), Decision::Leave);
        assert!(GangKnobs::at(1).required_odds >= GangKnobs::at(2).required_odds);
        assert!(GangKnobs::at(3).required_odds >= GangKnobs::at(4).required_odds);
        assert_eq!(GangKnobs::at(2), GangKnobs::default());
    }

    #[test]
    fn the_commitment_is_staged_the_smallest_set_first_and_the_rest_is_reserve() {
        let pool: Vec<(String, f64)> = vec![
            ("a".into(), hunt::power(Class::Medium)),
            ("b".into(), hunt::power(Class::Small)),
            ("c".into(), hunt::power(Class::Small)),
            ("d".into(), hunt::power(Class::Small)),
        ];
        let foes = hunt::power(Class::Small);
        let bar = GangKnobs::NORMAL.required_odds;
        let first = staged(&pool, foes, bar);
        assert!(
            first.len() >= MIN_COMMIT && first.len() < pool.len(),
            "{first:?}"
        );
        assert_eq!(first[0], "a", "the strongest first");
        let sum: f64 = first
            .iter()
            .map(|n| pool.iter().find(|p| &p.0 == n).unwrap().1)
            .sum();
        assert!(sum >= foes * bar);
        // Everything is not enough: nothing is committed.
        assert!(staged(&pool, 100.0, bar).is_empty());
    }

    // ---- no piecemeal engagement ----

    /// A gang of five at staging: the FC, three body ships, one scout (a
    /// small hull). The scout is in system A with `n` medium foes.
    fn five(t: Instant, foes_at_a: usize) -> Gang {
        let mut g = Gang::new(5);
        for (i, name) in ["fc", "b1", "b2", "b3", "sc"].iter().enumerate() {
            let (system, class) = match *name {
                "sc" => (A, Class::Small),
                _ => (STAGING, Class::Medium),
            };
            let mut p = post(t, Some(system), class);
            if *name == "sc" {
                p.foes = (0..foes_at_a).map(|i| foe(500 + i as i64, SMALL)).collect();
            }
            g.post(name, i, p);
        }
        g
    }

    fn formed(g: &mut Gang, d: &Ground, m: &Map, t: Instant) {
        // Everyone is in space at the home system: the FC forms the gang and
        // picks the scout, which then goes out.
        let c = ctx(d, m, GangKnobs::NORMAL);
        let out = g.member("sc").and_then(|m| m.post.clone()).expect("scout");
        g.post("sc", 4, post(t, Some(STAGING), Class::Small));
        let said = g.think("fc", t, &c);
        g.post("sc", 4, out);
        assert!(said.iter().any(|l| l.contains("Staging")), "{said:?}");
        assert_eq!(g.plan, Plan::Roam);
        assert_eq!(g.staging, Some(STAGING));
        assert_eq!(g.scouts, vec!["sc".to_owned()], "the small hull scouts");
    }

    #[test]
    fn the_body_leaps_together_and_lands_only_when_every_ship_is_in() {
        let (d, m) = (doc(), map());
        let t = Instant::now();
        let mut g = five(t, 2);
        // Not formed: nothing is ordered but a hold.
        assert_eq!(g.order("b1", t), Order::Hold);
        formed(&mut g, &d, &m, t);
        // The scout reports two small foes; the gang clears the bar.
        let c = ctx(&d, &m, GangKnobs::NORMAL);
        let said = g.think("fc", secs(t, 1), &c);
        assert!(said.iter().any(|l| l.contains("Engaging")), "{said:?}");
        let Plan::Gather { committed, .. } = g.plan.clone() else {
            panic!("a gather: {:?}", g.plan)
        };
        assert!(!committed.is_empty());
        // Everyone committed is told to leap; the scout holds its grid; the
        // ones left behind hold. Nobody is told to land or fight.
        for n in &committed {
            assert_eq!(g.order(n, t), Order::Leap(A));
        }
        assert_eq!(g.order("sc", t), Order::Linger);
        for n in ["fc", "b1", "b2", "b3", "sc"] {
            assert!(
                !matches!(g.order(n, t), Order::Land { .. } | Order::Fight { .. }),
                "{n} is sent early"
            );
        }
        // One committed ship arrives, the others do not: still gathering,
        // and the early one is told to wait where it is, not to land.
        let first = committed[0].clone();
        let t2 = secs(t, 5);
        let mut at_a = post(t2, Some(A), Class::Medium);
        at_a.foes = Vec::new();
        g.post(&first, rank_of(&first), at_a);
        refresh_others(&mut g, t2, &first);
        g.think("fc", t2, &c);
        if committed.len() > 1 {
            assert!(matches!(g.plan, Plan::Gather { .. }), "{:?}", g.plan);
            assert!(!matches!(
                g.order(&first, t2),
                Order::Land { .. } | Order::Fight { .. }
            ));
        }
        // All in: the landing is called for every committed ship at once.
        let t3 = secs(t, 10);
        for n in &committed {
            g.post(n, rank_of(n), post(t3, Some(A), Class::Medium));
        }
        g.post("sc", 4, scout_post(t3, 2));
        refresh_staging(&mut g, t3, &committed);
        let said = g.think("fc", t3, &c);
        assert!(said.iter().any(|l| l.contains("landing")), "{said:?}");
        for n in &committed {
            assert_eq!(
                g.order(n, t3),
                Order::Land {
                    system: A,
                    scout: "sc".into()
                }
            );
        }
        assert!(
            matches!(
                g.order("sc", t3),
                Order::Tackle {
                    primary: Some(_),
                    ..
                }
            ),
            "the scout tackles the primary as the body lands"
        );
        // Landed: the fight order goes to all of them together.
        let t4 = secs(t, 15);
        for n in &committed {
            let mut p = post(t4, Some(A), Class::Medium);
            p.foes = (0..2).map(|i| foe(500 + i, SMALL)).collect();
            g.post(n, rank_of(n), p);
        }
        g.post("sc", 4, scout_post(t4, 2));
        refresh_staging(&mut g, t4, &committed);
        g.think("fc", t4, &c);
        assert!(matches!(g.plan, Plan::Fight { .. }), "{:?}", g.plan);
        for n in &committed {
            assert!(matches!(g.order(n, t4), Order::Fight { .. }));
        }
    }

    fn rank_of(name: &str) -> usize {
        ["fc", "b1", "b2", "b3", "sc"]
            .iter()
            .position(|n| *n == name)
            .unwrap()
    }

    fn scout_post(t: Instant, foes: usize) -> Post {
        let mut p = post(t, Some(A), Class::Small);
        p.foes = (0..foes).map(|i| foe(500 + i as i64, SMALL)).collect();
        p
    }

    /// Keep the members that did not move posting at staging.
    fn refresh_others(g: &mut Gang, t: Instant, moved: &str) {
        for n in ["fc", "b1", "b2", "b3"] {
            if n != moved {
                g.post(n, rank_of(n), post(t, Some(STAGING), Class::Medium));
            }
        }
        g.post("sc", 4, scout_post(t, 2));
    }

    fn refresh_staging(g: &mut Gang, t: Instant, away: &[String]) {
        for n in ["fc", "b1", "b2", "b3"] {
            if !away.iter().any(|a| a == n) {
                g.post(n, rank_of(n), post(t, Some(STAGING), Class::Medium));
            }
        }
    }

    #[test]
    fn a_lone_scout_never_makes_a_fight_and_a_hopeless_report_is_left() {
        let (d, m) = (doc(), map());
        let t = Instant::now();
        // The body is docked (repairing): only the scout is at hand.
        let mut g = five(t, 2);
        let c = ctx(&d, &m, GangKnobs::NORMAL);
        formed(&mut g, &d, &m, t);
        for n in ["b1", "b2", "b3"] {
            g.post(n, rank_of(n), post(t, None, Class::Medium));
        }
        g.post("fc", 0, post(t, None, Class::Medium));
        // The scout is the only ship in space: it is the acting FC, alone.
        g.think("sc", t, &c);
        assert!(
            !matches!(
                g.plan,
                Plan::Gather { .. } | Plan::Land { .. } | Plan::Fight { .. }
            ),
            "{:?}",
            g.plan
        );
        assert_eq!(
            g.order("sc", t),
            Order::Linger,
            "the scout watches, it does not shoot"
        );
        // Many large foes against the whole gang: it leaves the system alone.
        let mut g = five(t, 0);
        formed(&mut g, &d, &m, t);
        let mut p = post(t, Some(A), Class::Small);
        p.foes = (0..8).map(|i| foe(600 + i, LARGE)).collect();
        g.post("sc", 4, p);
        let said = g.think("fc", t, &c);
        assert!(said.iter().any(|l| l.contains("too many")), "{said:?}");
        assert!(g.cooldown.contains_key(&A));
        assert_eq!(g.plan, Plan::Roam);
        // And the scout moves on rather than lingering on a system left alone.
        assert_eq!(g.order("sc", t), Order::Scout);
    }

    #[test]
    fn only_the_fc_thinks_and_a_docked_boss_hands_over() {
        let (d, m) = (doc(), map());
        let t = Instant::now();
        let mut g = five(t, 0);
        let c = ctx(&d, &m, GangKnobs::NORMAL);
        assert!(g.think("b1", t, &c).is_empty(), "b1 is not the FC");
        assert_eq!(g.fc(t), Some("fc"));
        g.post("fc", 0, post(t, None, Class::Medium));
        assert_eq!(g.fc(t), Some("b1"), "the next rank in space");
        // A silent member is not the FC either.
        assert_eq!(g.fc(secs(t, POST_STALE_S + 1)), None);
    }

    // ---- the primary ----

    fn pilot(id: i64, hull: i64, x: f64, hostile: bool) -> Pilot {
        Pilot {
            ball: Ball {
                id,
                position: [x, 0.0, 0.0],
                radius: 20.0,
            },
            char_id: id + 1_000,
            type_id: Some(hull),
            who: eve_server::group_contacts::Affiliation {
                character_id: id + 1_000,
                corporation_id: 98_000_001,
                alliance_id: None,
            },
            hostile,
            relation: 0.0,
            war: false,
        }
    }

    #[test]
    fn the_primary_is_called_by_score_stays_while_present_and_is_re_called() {
        let d = doc();
        let ego = Ball {
            id: 1,
            position: [0.0; 3],
            radius: 20.0,
        };
        let foes = vec![
            pilot(21, MEDIUM, 10_000.0, true),
            pilot(22, SMALL, 10_000.0, true),
            pilot(23, POD, 1_000.0, true),
            pilot(24, SMALL, 1_000.0, false),
        ];
        let first = choose_primary(None, &foes, Class::Medium, &ego, &d, &[]).expect("a primary");
        assert_eq!(
            first, 22,
            "the weaker hull: a quick kill; never a pod or a friend"
        );
        // It stays while it is on the grid.
        assert_eq!(
            choose_primary(Some(21), &foes, Class::Medium, &ego, &d, &[]),
            Some(21)
        );
        // Dead or warped off: re-called among the rest.
        let rest: Vec<Pilot> = foes.iter().filter(|p| p.ball.id != 22).cloned().collect();
        assert_eq!(
            choose_primary(Some(22), &rest, Class::Medium, &ego, &d, &[]),
            Some(21)
        );
        // Nothing hostile: no call.
        assert_eq!(
            choose_primary(Some(22), &[], Class::Medium, &ego, &d, &[]),
            None
        );

        // The same from the foes the members posted.
        let mut seen: BTreeMap<i64, Foe> = BTreeMap::new();
        for (b, h) in [(31, MEDIUM), (32, SMALL), (33, POD)] {
            seen.insert(b, foe(b, h));
        }
        assert_eq!(pick_primary(None, &seen), Some(32));
        assert_eq!(pick_primary(Some(31), &seen), Some(31));
        seen.remove(&31);
        seen.remove(&32);
        assert_eq!(
            pick_primary(Some(31), &seen),
            None,
            "only a pod is left: nobody shoots it"
        );
    }

    #[test]
    fn a_tackler_on_the_fc_takes_the_call_from_the_primary_already_called() {
        let d = doc();
        let ego = Ball {
            id: 1,
            position: [0.0; 3],
            radius: 20.0,
        };
        let foes = vec![
            pilot(21, MEDIUM, 3_000.0, true),
            pilot(22, SMALL, 40_000.0, true),
        ];
        assert_eq!(
            choose_primary(Some(21), &foes, Class::Medium, &ego, &d, &[]),
            Some(21)
        );
        assert_eq!(
            choose_primary(Some(21), &foes, Class::Medium, &ego, &d, &[22]),
            Some(22),
            "the tackler first"
        );
        assert_eq!(
            choose_primary(Some(22), &foes, Class::Medium, &ego, &d, &[22]),
            Some(22)
        );
    }

    #[test]
    fn everyone_in_the_fight_is_ordered_onto_the_same_primary_and_it_is_re_called_on_death() {
        let (d, m) = (doc(), map());
        let t = Instant::now();
        let mut g = five(t, 2);
        let c = ctx(&d, &m, GangKnobs::NORMAL);
        g.staging = Some(A);
        g.plan = Plan::Fight {
            system: A,
            scout: "sc".into(),
            committed: vec!["fc".into(), "b1".into(), "b2".into()],
            pending: Vec::new(),
            surges: 0,
            since: t,
            last_foe: t,
        };
        for n in ["fc", "b1", "b2", "sc"] {
            let mut p = post(
                t,
                Some(A),
                if n == "sc" {
                    Class::Small
                } else {
                    Class::Medium
                },
            );
            p.foes = vec![foe(701, MEDIUM), foe(702, SMALL)];
            g.post(n, rank_of(n), p);
        }
        g.think("fc", t, &c);
        let first = g.primary.expect("called");
        assert_eq!(first, 702);
        let primaries: BTreeSet<Option<i64>> = ["fc", "b1", "b2", "sc"]
            .iter()
            .map(|n| match g.order(n, t) {
                Order::Fight { primary, .. } => primary,
                other => panic!("{n}: {other:?}"),
            })
            .collect();
        assert_eq!(
            primaries,
            BTreeSet::from([Some(first)]),
            "one primary for all"
        );
        // The primary dies: nobody posts it any more; the call moves.
        let t2 = secs(t, 3);
        for n in ["fc", "b1", "b2", "sc"] {
            let mut p = post(
                t2,
                Some(A),
                if n == "sc" {
                    Class::Small
                } else {
                    Class::Medium
                },
            );
            p.foes = vec![foe(701, MEDIUM)];
            g.post(n, rank_of(n), p);
        }
        g.think("fc", t2, &c);
        assert_eq!(g.primary, Some(701));
    }

    // ---- retreat together ----

    #[test]
    fn the_gang_retreats_when_the_odds_fall_or_half_the_ships_are_lost() {
        let k = GangKnobs::NORMAL;
        let bar = k.required_odds;
        let abort = bar * k.abort_fraction();
        assert!(
            !should_retreat(abort * 3.0 * 1.01, 3.0, 0, 4, &k),
            "winning"
        );
        assert!(
            should_retreat(abort * 3.0 * 0.9, 3.0, 0, 4, &k),
            "under the abort line"
        );
        assert!(
            !should_retreat(bar * 9.0, 3.0, 1, 4, &k),
            "one of four lost, odds fine"
        );
        assert!(should_retreat(bar * 9.0, 3.0, 2, 4, &k), "half lost");
        assert!(
            !should_retreat(1.0, 0.0, 0, 4, &k),
            "no foes: nothing to flee"
        );
        // A bolder gang tolerates more.
        assert!(GangKnobs::BRUTAL.abort_fraction() < GangKnobs::EASY.abort_fraction());
    }

    #[test]
    fn a_losing_fight_withdraws_everyone_then_the_gang_regroups_and_roams_on() {
        let (d, m) = (doc(), map());
        let t = Instant::now();
        let mut g = five(t, 2);
        let c = ctx(&d, &m, GangKnobs::NORMAL);
        g.staging = Some(STAGING);
        g.scouts = vec!["sc".into()];
        g.plan = Plan::Fight {
            system: A,
            scout: "sc".into(),
            committed: vec!["fc".into(), "b1".into(), "b2".into()],
            pending: Vec::new(),
            surges: crate::doctrine_theatre::SURGE_MAX,
            since: t,
            last_foe: t,
        };
        // Two of four down (pods or silent), a big enemy on the grid.
        let mut fcp = post(t, Some(A), Class::Medium);
        fcp.foes = (0..4).map(|i| foe(800 + i, LARGE)).collect();
        g.post("fc", 0, fcp);
        let mut scp = post(t, Some(A), Class::Small);
        scp.foes = (0..4).map(|i| foe(800 + i, LARGE)).collect();
        g.post("sc", 4, scp);
        for n in ["b1", "b2"] {
            let mut pod = post(t, Some(A), Class::Capsule);
            pod.armed = false;
            g.post(n, rank_of(n), pod);
        }
        let said = g.think("fc", t, &c);
        assert!(matches!(g.plan, Plan::Withdraw { .. }), "{:?}", g.plan);
        assert!(said.iter().any(|l| l.contains("out")), "{said:?}");
        for n in ["fc", "b1", "b2", "sc"] {
            assert_eq!(g.order(n, t), Order::Withdraw);
        }
        assert!(g.cooldown.contains_key(&A), "the system is avoided");
        assert_eq!(g.retreats, 1);
        // Everyone home (docked or at staging): back to roaming.
        let t2 = secs(t, 20);
        for n in ["fc", "b1", "b2"] {
            g.post(n, rank_of(n), post(t2, None, Class::Medium));
        }
        g.post("sc", 4, post(t2, Some(STAGING), Class::Small));
        g.post("b3", 3, post(t2, Some(STAGING), Class::Medium));
        let said = g.think("b3", t2, &c);
        assert_eq!(g.plan, Plan::Roam, "{said:?}");
    }

    #[test]
    fn a_won_fight_ends_in_a_regroup_and_a_thin_one_calls_the_reserve_in_one_surge() {
        let (d, m) = (doc(), map());
        let t = Instant::now();
        let mut g = five(t, 0);
        let c = ctx(&d, &m, GangKnobs::NORMAL);
        g.staging = Some(STAGING);
        g.scouts = vec!["sc".into()];
        g.plan = Plan::Fight {
            system: A,
            scout: "sc".into(),
            committed: vec!["fc".into(), "b1".into()],
            pending: Vec::new(),
            surges: 0,
            since: t,
            last_foe: t,
        };
        // A fight that is under the bar with a ship (b3) in reserve at staging.
        let strong = |t: Instant| {
            let mut p = post(t, Some(A), Class::Medium);
            p.foes = (0..2).map(|i| foe(900 + i, MEDIUM)).collect();
            p
        };
        g.post("fc", 0, strong(t));
        g.post("b1", 1, strong(t));
        let mut sp = strong(t);
        sp.class = Class::Small;
        g.post("sc", 4, sp);
        g.post("b2", 2, post(t, Some(STAGING), Class::Medium));
        g.post("b3", 3, post(t, Some(STAGING), Class::Medium));
        let said = g.think("fc", t, &c);
        assert!(said.iter().any(|l| l.contains("Surge")), "{said:?}");
        let Plan::Fight { pending, .. } = g.plan.clone() else {
            panic!("{:?}", g.plan)
        };
        assert_eq!(pending.len(), 2, "the whole reserve comes together");
        for n in &pending {
            assert_eq!(g.order(n, t), Order::Leap(A));
        }
        // Won: no foe for CLEAR_S.
        let mut g = five(t, 0);
        g.staging = Some(STAGING);
        g.plan = Plan::Fight {
            system: A,
            scout: "sc".into(),
            committed: vec!["fc".into(), "b1".into()],
            pending: Vec::new(),
            surges: 0,
            since: t,
            last_foe: t,
        };
        let t2 = secs(t, CLEAR_S + 1);
        for (n, class) in [
            ("fc", Class::Medium),
            ("b1", Class::Medium),
            ("sc", Class::Small),
        ] {
            g.post(n, rank_of(n), post(t2, Some(A), class));
        }
        let said = g.think("fc", t2, &c);
        assert_eq!(g.plan, Plan::Roam);
        assert_eq!(g.fights, 1);
        assert!(said.iter().any(|l| l.contains("clear")), "{said:?}");
    }

    // ---- bait, the form-up, the line ----

    #[test]
    fn a_bait_is_shown_once_then_given_up_if_it_does_not_change_the_odds() {
        let (d, m) = (doc(), map());
        let t = Instant::now();
        let mut g = five(t, 0);
        let k = GangKnobs {
            required_odds: GangKnobs::NORMAL.required_odds * 1.07,
            ..GangKnobs::NORMAL
        };
        let c = ctx(&d, &m, k);
        g.post("sc", 4, post(t, Some(STAGING), Class::Small));
        let said = g.think("fc", t, &c);
        assert!(said.iter().any(|l| l.contains("Staging")));
        // Three mediums on the grid: the gang (4 mediums + a small) is just
        // under the bar, inside the bait band.
        let band = lerp(1.0, BAIT_BAND, k.aggression);
        let mine = hunt::power(Class::Medium) * 4.0 + hunt::power(Class::Small);
        let n_foes = 3usize;
        let foe_power = n_foes as f64 * hunt::power(Class::Medium);
        assert!(mine / foe_power < k.required_odds && mine / foe_power >= k.required_odds * band);
        let mut p = post(t, Some(A), Class::Small);
        p.foes = (0..n_foes).map(|i| foe(950 + i as i64, MEDIUM)).collect();
        g.post("sc", 4, p.clone());
        let said = g.think("fc", t, &c);
        assert!(said.iter().any(|l| l.contains("baiting")), "{said:?}");
        assert_eq!(g.plan, Plan::Roam, "the body holds");
        assert_eq!(g.order("b1", t), Order::Hold);
        assert_eq!(
            g.order("sc", t),
            Order::Linger,
            "the bait stays on the grid"
        );
        // Not said twice.
        assert!(g.think("fc", secs(t, 1), &c).is_empty());
        // After BAIT_S with the same picture: given up.
        let t2 = secs(t, BAIT_S + 1);
        for n in ["fc", "b1", "b2", "b3"] {
            g.post(n, rank_of(n), post(t2, Some(STAGING), Class::Medium));
        }
        let mut p2 = p;
        p2.at = t2;
        g.post("sc", 4, p2);
        let said = g.think("fc", t2, &c);
        assert!(said.iter().any(|l| l.contains("bait failed")), "{said:?}");
        assert!(g.cooldown.contains_key(&A));
    }

    #[test]
    fn the_gang_forms_when_all_are_up_or_two_are_up_after_the_timeout() {
        let (d, m) = (doc(), map());
        let t = Instant::now();
        let c = ctx(&d, &m, GangKnobs::NORMAL);
        let mut g = Gang::new(4);
        g.post("fc", 0, post(t, Some(STAGING), Class::Medium));
        g.post("b1", 1, post(t, Some(STAGING), Class::Medium));
        assert!(
            g.think("fc", t, &c).is_empty(),
            "two of four, no timeout yet"
        );
        assert_eq!(g.plan, Plan::Form);
        let t2 = secs(t, FORM_TIMEOUT_S);
        g.post("fc", 0, post(t2, Some(STAGING), Class::Medium));
        g.post("b1", 1, post(t2, Some(STAGING), Class::Medium));
        let said = g.think("fc", t2, &c);
        assert_eq!(g.plan, Plan::Roam, "{said:?}");
        assert_eq!(g.staging, Some(STAGING));
        assert_eq!(g.scouts.len(), 1);
        // One ship is not a gang.
        let mut g = Gang::new(4);
        g.post("fc", 0, post(t, Some(STAGING), Class::Medium));
        assert!(g.think("fc", secs(t, FORM_TIMEOUT_S), &c).is_empty());
        assert_eq!(g.plan, Plan::Form);
    }

    #[test]
    fn the_scout_fires_only_its_tackle_until_the_body_has_landed() {
        let gun = |item_id, type_id| brain::GunInfo {
            item_id,
            type_id,
            flag_id: 27,
            online: true,
            active: false,
            target: None,
            rounds: None,
            capacity: 0,
            range: 0.0,
            reload: Duration::ZERO,
            kind: match hunt::tackle_kind(type_id) {
                Some(hunt::TackleKind::Warp) => brain::GunKind::Scram,
                Some(hunt::TackleKind::Web) => brain::GunKind::Web,
                None => brain::GunKind::Weapon,
            },
            reach: 0.0,
        };
        let guns = [gun(1, 5_001), gun(2, hunt::SCRAMBLER)];
        let acts = vec![
            Act::Lock(9),
            Act::Fire { gun: 1, target: 9 },
            Act::Fire { gun: 2, target: 9 },
        ];
        assert_eq!(
            tackle_only(acts, &guns),
            vec![Act::Lock(9), Act::Fire { gun: 2, target: 9 }]
        );
    }

    #[test]
    fn the_intel_line_counts_and_names_the_hulls_and_the_belt() {
        let foes = vec![foe(1, SMALL), foe(2, MEDIUM), foe(3, SMALL)];
        let line = intel_line(A, Some(777), &foes);
        assert_eq!(
            line,
            format!("3 in {A}: small {SMALL}, medium {MEDIUM}, small {SMALL} at belt 777")
        );
        assert_eq!(
            intel_line(A, None, &foes[..1]),
            format!("1 in {A}: small {SMALL}")
        );
    }

    /// fix-w8b B1: a gang hunter's first pass is [Stock, Undock]; the
    /// scrambler is still fitted before the undock, with the Stock kept.
    #[test]
    fn a_gang_hunters_first_undock_keeps_the_stock_and_fits_the_tackle() {
        const SCRAMBLER_ROW: i64 = 7_001;
        let mut policy = crate::brain::Policy::fast();
        policy.role = &ROLE;
        policy.combat_drones = true;
        policy.pvp = Some(Arc::new(doc()));
        policy.gang = Some(Seat {
            hall: Arc::new(GangHall::new()),
            gang: "g".to_owned(),
            rank: 0,
            size: 1,
            knobs: GangKnobs::NORMAL,
        });
        let mut m = Memory::new("Invented Gang Hunter", policy);
        m.hunt.safety_set = true;
        m.hunt.settled_at = Some(Instant::now());
        let mut o = Observation::empty(Instant::now());
        o.place = Place::Docked { station: STAGING };
        o.ship = Some(crate::brain::ShipInfo {
            ship_id: 1,
            type_id: Some(escort::RIFTER),
        });
        o.guns = vec![crate::brain::GunInfo {
            item_id: 1,
            type_id: escort::GUN,
            flag_id: 27,
            online: true,
            active: false,
            target: None,
            rounds: Some(10),
            capacity: 10,
            range: 2_000.0,
            reload: Duration::ZERO,
            kind: crate::brain::GunKind::Weapon,
            reach: 0.0,
        }];
        o.hangar = vec![crate::ItemRow {
            item_id: SCRAMBLER_ROW,
            type_id: hunt::SCRAMBLER,
            owner_id: 1,
            location_id: STAGING,
            flag_id: crate::FLAG_HANGAR,
            quantity: 1,
            group_id: 0,
            category_id: 0,
            singleton: true,
        }];
        let acts = docked(&mut m, &o);
        assert!(
            acts.contains(&Act::Drone(crate::drone_use::DroneAct::Stock)),
            "{acts:?}"
        );
        assert!(acts.contains(&Act::FitAuto(SCRAMBLER_ROW)), "{acts:?}");
        assert!(!acts.contains(&Act::Undock), "{acts:?}");
    }

    #[test]
    fn the_hall_keeps_one_state_per_gang_id_and_the_levels_are_ordered() {
        let hall = GangHall::new();
        hall.with("g1", 4, |g| g.fights = 3);
        hall.with("g2", 4, |g| g.fights = 1);
        assert_eq!(hall.snapshot("g1").map(|g| g.fights), Some(3));
        assert_eq!(hall.snapshot("g2").map(|g| g.fights), Some(1));
        assert!(hall.snapshot("none").is_none());
        for w in [1u8, 2, 3].windows(2) {
            let (a, b) = (GangKnobs::at(w[0]), GangKnobs::at(w[1]));
            assert!(a.aggression <= b.aggression && a.scouts <= b.scouts && a.radius <= b.radius);
            assert!(a.required_odds >= b.required_odds);
        }
        assert!(check_opt_in([&ROLE as brain::RoleRef], false).is_err());
        assert!(check_opt_in([&ROLE as brain::RoleRef], true).is_ok());
        assert!(check_opt_in([crate::roles::default()], false).is_ok());
    }

    /// A solo gang hunter in space, one gun, safety set, `hull` and `armour`
    /// as read from the client (invented ids).
    fn hurt_hunter(armour: f64, hull: f64) -> (Memory, Observation) {
        let mut policy = crate::brain::Policy::fast();
        policy.role = &ROLE;
        policy.pvp = Some(Arc::new(doc()));
        policy.gang = Some(Seat {
            hall: Arc::new(GangHall::new()),
            gang: "g".to_owned(),
            rank: 0,
            size: 1,
            knobs: GangKnobs::NORMAL,
        });
        let mut m = Memory::new("Invented Solo Hunter", policy);
        m.hunt.safety_set = true;
        let mut o = Observation::empty(Instant::now());
        o.place = Place::InSpace {
            system: A,
            bound: true,
        };
        o.park = Some(ParkView {
            ego: Ball {
                id: 1,
                position: [0.0, 0.0, 0.0],
                radius: 20.0,
            },
            ego_warping: false,
            rocks: Vec::new(),
            belts: Vec::new(),
            station: None,
            boss: None,
            taken: BTreeMap::new(),
            hostiles: Vec::new(),
            wrecks: Vec::new(),
            ego_tether: None,
        });
        o.guns = vec![crate::brain::GunInfo {
            item_id: 1,
            type_id: escort::GUN,
            flag_id: 27,
            online: true,
            active: false,
            target: None,
            rounds: Some(10),
            capacity: 10,
            range: 2_000.0,
            reload: Duration::ZERO,
            kind: crate::brain::GunKind::Weapon,
            reach: 0.0,
        }];
        o.health = Some(brain::Health {
            shield: 0.0,
            armour,
            hull,
        });
        (m, o)
    }

    /// hunter-red: a scratch on the hull (light rat fire in a belt) did not
    /// ought to send a solo hunter home; real damage does, and says why.
    #[test]
    fn light_hull_damage_does_not_send_a_gang_hunter_home() {
        let (mut m, o) = hurt_hunter(0.9, 0.9);
        let _ = in_space(&mut m, &o);
        assert_eq!(m.homing, None, "hull 0.9 with armour 0.9 is a scratch");
        let (mut m, o) = hurt_hunter(0.3, 1.0);
        let _ = in_space(&mut m, &o);
        assert_eq!(m.homing, Some(Homing::Repair), "armour under half");
        let (mut m, mut o) = hurt_hunter(0.9, 0.5);
        o.chat = true;
        let acts = in_space(&mut m, &o);
        assert_eq!(
            m.homing,
            Some(Homing::Repair),
            "hull under the gang's floor"
        );
        assert!(
            acts.iter()
                .any(|a| matches!(a, Act::Say(l) if l.contains("Homing: repair"))),
            "{acts:?}"
        );
    }

    /// hunter-red: a retreat from the owner's fight keeps the quarry listed,
    /// so the gang hunts him again after the repair.
    #[test]
    fn a_retreat_keeps_the_owners_quarry_listed() {
        let (d, m) = (doc(), map());
        let t = Instant::now();
        let c = ctx(&d, &m, GangKnobs::NORMAL);
        let mut g = Gang::new(1);
        g.set_quarry(WHO, A, t);
        if let Some(q) = &mut g.quarry {
            q.engaged = true;
        }
        let mut said = Vec::new();
        g.begin_withdraw(A, "fc", &[], t, &mut said, "Losing (test)");
        assert!(matches!(g.plan, Plan::Withdraw { .. }));
        assert!(g.quarry.is_some(), "the owner is still the quarry");
        assert_eq!(g.quarry_engaged(), None, "but the fight is over");
        let _ = c;
    }
}
