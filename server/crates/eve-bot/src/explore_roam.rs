//! **Exploration as a roaming career** (plan-deep-queue wave Y, packet
//! Y-ROAM; the follow-up to HK4 and SOC-B3): the rules the hacker
//! ([`crate::hacker`]) and the prober ([`crate::probe`]) share when they
//! leave their home system for sites.
//!
//! What a player-explorer does, and where it lives:
//!
//! 1. **Chooses how far from safety to go** by nerve ([`Nerve`], from the
//!    mind's risk and aggression traits): high-sec only, low-sec too, or
//!    null as well ([`floor_for`], [`Space`]). A bot with no mind has the
//!    neutral nerve ([`Nerve::NEUTRAL`]).
//! 2. **Plans a loop** over nearby systems ([`plan_tour`], pure): the
//!    systems the shared probe ledger says hold unworked data or relic
//!    signatures first, then ones nobody scanned out lately, nearest first
//!    from where it is, never farther than [`ROAM_JUMPS`] from home, a
//!    whole loop of at most [`TOUR_JUMPS`] gates, never into a system it
//!    has reason to avoid.
//! 3. **Looks before it stays** ([`look`], [`assess`], [`avoid`]): what a
//!    player sees on arrival -- the pilots on the gate's grid (their
//!    corporation and security status are on every ship's slim) and a
//!    360 degree directional scan, which the server serves
//!    (`scan::cone_scan`). Local's member list is the chat client's, which
//!    the bot's connection does not read; see *Not done* in
//!    `docs/lanes/y-roam.md`. What it saw is kept for [`DANGER_TTL`] on the
//!    shared [`DANGER`] ledger, so the next explorer knows too.
//! 4. **Works what it can** ([`job_for`]): data and relic signatures are
//!    hacked; gas, combat, ore and the rest are *booked* on the ledger
//!    ([`crate::probe::Ledger::note`]) for the careers that work them and
//!    left.
//! 5. **Comes home to sell** -- or to the place within reach that pays most
//!    ([`rank_outlets`], pure) -- when the hold is full or valuable
//!    ([`should_return`]).
//! 6. **Keeps its place across a reconnect** ([`TripState`], [`put`],
//!    [`get`]): the state file's pattern (`crate::state`): a std-only text
//!    file written by temp-and-rename, whole or ignored.
//!
//! **Every rule and number here is [A] policy**, with its falsifier on the
//! constant, except the client's calls, which the verbs cite.

use crate::probe::{Kind, Pitch};
use crate::travel::{ANY_SECURITY, Map, SECURITY_FLOOR};
use crate::verbs::Book;
use crate::{Conn, Error, ItemRow, Result};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---- nerve and security bands ------------------------------------------------------

/// How much of the nerve is the risk trait; the aggression trait is the
/// rest **[A]**: a timid pilot is timid whatever his temper, a warlike one
/// only a little bolder. Falsified by explorers' losses tracking aggression
/// rather than risk.
pub const NERVE_RISK_WEIGHT: f64 = 0.65;

/// Below this nerve a bot flies high-sec only **[A]**. Traits are drawn as
/// an average of three dice (mean 0.5, deviation about 0.14 for the
/// weighted nerve), so about a third of the explorers stay in high-sec.
/// Falsified by `[econ]` showing explorers that never find enough sites
/// (too high) or are lost in low-sec in their first hour (too low).
pub const LOW_NERVE: f64 = 0.45;

/// At or above this nerve a bot flies null-sec too **[A]**: about one in
/// ten. Falsified as [`LOW_NERVE`].
pub const NULL_NERVE: f64 = 0.65;

/// At or above this nerve a bot is not put off by company in low and null
/// (other ships on the directional scan) **[A]**.
pub const BOLD_NERVE: f64 = 0.60;

/// At or above this nerve a bot is not put off even by pilots on its
/// grid who are not its corporation's **[A]**: a handful of the 50.
pub const FEARLESS_NERVE: f64 = 0.85;

/// The aggression a bot needs to call itself a fighter **[A]**: only
/// fighters work combat signatures, and nothing in this lane flies one
/// (Y-COMBAT), so it only decides whether they are worth booking.
pub const FIGHTER_AGGRESSION: f64 = 0.6;

/// The floor a high-sec-only explorer routes at: the travel lane's own.
pub const HIGH_FLOOR: f64 = SECURITY_FLOOR;
/// A floor that lets low-sec through (any security above zero) **[A]**.
pub const LOW_FLOOR: f64 = 0.05;
/// A floor that lets null-sec through but not a system whose security the
/// map does not know **[A]**: unknown is not a risk anyone chose.
pub const NULL_FLOOR: f64 = -1.0;

/// How the bot weighs the two traits it flies by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Nerve {
    pub risk: f64,
    pub aggression: f64,
}

impl Nerve {
    /// A bot with no mind: the middle of both.
    pub const NEUTRAL: Nerve = Nerve {
        risk: 0.5,
        aggression: 0.5,
    };

    /// The single number the bands are cut on.
    pub fn value(self) -> f64 {
        NERVE_RISK_WEIGHT * self.risk + (1.0 - NERVE_RISK_WEIGHT) * self.aggression
    }

    pub fn fighter(self) -> bool {
        self.aggression >= FIGHTER_AGGRESSION
    }
}

/// **The floor an explorer of this nerve routes at** (pure).
pub fn floor_for(n: Nerve) -> f64 {
    let v = n.value();
    if v < LOW_NERVE {
        HIGH_FLOOR
    } else if v < NULL_NERVE {
        LOW_FLOOR
    } else {
        NULL_FLOOR
    }
}

/// Which kind of space a system is, as the map knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Space {
    High,
    Low,
    Null,
    /// The map says nothing of its security.
    Unknown,
}

/// `system`'s [`Space`] (pure).
pub fn space_of(map: &Map, system: i64) -> Space {
    if !map.allowed(system, NULL_FLOOR) {
        Space::Unknown
    } else if map.allowed(system, HIGH_FLOOR) {
        Space::High
    } else if map.allowed(system, LOW_FLOOR) {
        Space::Low
    } else {
        Space::Null
    }
}

/// **What the driver tells a roaming career** about its bot (coordinator
/// diff, `driver.rs`): the map, the mind's nerve, the home station, the
/// state directory and the corporation's builder. `Ctx::default()` is the
/// plain home-system bot of HK4.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ctx<'a> {
    /// The gate graph. `None`: no roaming.
    pub trips: Option<crate::travel::Trips<'a>>,
    /// The mind's risk and aggression. `None`: [`Nerve::NEUTRAL`].
    pub nerve: Option<Nerve>,
    /// The home station when the driver knows it.
    pub home: Option<i64>,
    /// `eve-bots --state DIR`: where the trip file goes. `None`: memory
    /// only.
    pub state: Option<&'a Path>,
    /// Is there a builder in the bot's own corporation
    /// ([`builder_in_corp`])?
    pub builder_in_corp: bool,
}

impl Ctx<'_> {
    /// May this bot leave its home system.
    pub fn roams(&self) -> bool {
        self.trips.is_some()
    }

    pub fn nerve(&self) -> Nerve {
        self.nerve.unwrap_or(Nerve::NEUTRAL)
    }
}

// ---- what the bot does with a signature --------------------------------------------

/// What an explorer does with a signature of a kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    /// Scan down, warp to, hack.
    Work,
    /// Book it on the ledger for whoever works it, and move on.
    Leave,
}

/// **The rule** (pure): data and relic signatures are worked when the bot
/// can open a can (`can_hack`: an analyzer is fitted); a signature whose
/// kind is not disclosed yet is scanned on (the kind comes with the scan);
/// everything else -- gas (the huffer's), combat (a fighter's: Y-COMBAT's),
/// ore, wormholes -- is left, and booked ([`crate::probe::Ledger::note`])
/// for the careers that work it. Nothing here flies a fight, so a
/// non-fighter and a fighter leave combat alike; [`Nerve::fighter`] is for
/// the careers that will.
pub fn job_for(kind: Option<Kind>, can_hack: bool) -> Job {
    match kind {
        None => Job::Work,
        Some(Kind::Data | Kind::Relic) if can_hack => Job::Work,
        Some(_) => Job::Leave,
    }
}

// ---- what the bot saw: threat --------------------------------------------------------

/// `ConeScan`'s full circle, radians (`scan::in_cone`: half the angle at
/// or past pi is a sphere).
pub const DSCAN_ANGLE: f64 = std::f64::consts::TAU;
/// The longest directional range, 14.3 AU in metres **[A]**: the
/// client's slider maximum as remembered; the server does not clamp it
/// (`scan::cone_scan`), so a smaller true maximum only costs the bot a
/// result the client could not have asked for.
pub const DSCAN_RANGE: f64 = 14.3 * 149_597_870_700.0;

/// A pilot's ship group on the directional scan **[A]**: the public
/// inventory group ids of the hulls players fly (frigate 25, cruiser 26,
/// battleship 27, industrial 28, titan 30, shuttle 31, rookie ship 237,
/// assault frigate 324, heavy assault cruiser 358, deep space transport
/// 380, battlecruiser 419, destroyer 420, mining barge 463, dreadnought
/// 485, freighter 513, command ship 540, interdictor 541, exhumer 543,
/// carrier 547, supercarrier 659, covert ops 830, interceptor 831, logistics
/// 832, force recon 833, stealth bomber 834, electronic attack frigate 893,
/// heavy interdictor 894, black ops 898, marauder 900, combat recon 906,
/// strategic cruiser 963). The scan names a ball's group, not its pilot, so
/// this is how a bot tells a player's ship from a rat's. Falsified by a
/// rat's group on the list (the bot sees company in an empty belt) or a
/// player hull off it (the bot flies into a gang).
pub const PILOT_SHIP_GROUPS: &[i64] = &[
    25, 26, 27, 28, 30, 31, 237, 324, 358, 380, 419, 420, 463, 485, 513, 540, 541, 543, 547, 659,
    830, 831, 832, 833, 834, 893, 894, 898, 900, 906, 963,
];

/// A pilot with a security status at or under this is flagged by the
/// client (the Dangerous mark, `sec_status`) **[V]**, and counted hostile
/// even in high-sec.
pub const DANGEROUS_STATUS: f64 = -0.1;

/// How long what the bot saw in a system is believed **[A]**: a gang moves
/// on within the hour. Falsified by explorers that keep flying into a
/// system a minute after another saw hostiles there (too short) or never
/// return to one that cleared (too long).
pub const DANGER_TTL: Duration = Duration::from_secs(20 * 60);

/// A pilot on the grid, as the ship's slim says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pilot {
    pub char_id: i64,
    pub corp_id: Option<i64>,
    pub security_status: Option<f64>,
}

/// Who the bot is, for telling its own from the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Me {
    pub char_id: i64,
    pub corp_id: Option<i64>,
}

/// What a system looked like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Threat {
    #[default]
    Clear,
    /// Other pilots' ships on the directional scan (low and null only).
    Company(u32),
    /// Pilots on the grid who are not the bot's corporation's and who may
    /// shoot: any in low and null, flagged ones in high-sec.
    Hostile(u32),
}

/// **What the bot saw** (pure): `pilots` are the grid's, `scan_ships` the
/// directional scan's pilot-hull count.
pub fn assess(me: Me, space: Space, pilots: &[Pilot], scan_ships: u32) -> Threat {
    let hostile = pilots
        .iter()
        .filter(|p| p.char_id != me.char_id)
        .filter(|p| me.corp_id.is_none() || p.corp_id != me.corp_id)
        .filter(|p| match space {
            Space::High => p.security_status.is_some_and(|s| s <= DANGEROUS_STATUS),
            Space::Low | Space::Null | Space::Unknown => true,
        })
        .count() as u32;
    if hostile > 0 {
        return Threat::Hostile(hostile);
    }
    match space {
        Space::High => Threat::Clear,
        _ if scan_ships > 0 => Threat::Company(scan_ships),
        _ => Threat::Clear,
    }
}

/// **Does this nerve stay out of a system that looked like this** (pure).
pub fn avoid(threat: Threat, nerve: Nerve) -> bool {
    let v = nerve.value();
    match threat {
        Threat::Clear => false,
        Threat::Company(_) => v < BOLD_NERVE,
        Threat::Hostile(_) => v < FEARLESS_NERVE,
    }
}

/// The shared memory of what explorers saw: per server and system, when
/// and what. Bot-side, like [`crate::probe::LEDGER`].
#[derive(Debug)]
pub struct Dangers {
    inner: Mutex<BTreeMap<Pitch, (Instant, Threat)>>,
}

impl Default for Dangers {
    fn default() -> Dangers {
        Dangers::new()
    }
}

impl Dangers {
    pub const fn new() -> Dangers {
        Dangers {
            inner: Mutex::new(BTreeMap::new()),
        }
    }

    fn with<T>(&self, f: impl FnOnce(&mut BTreeMap<Pitch, (Instant, Threat)>) -> T) -> T {
        f(&mut self.inner.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// A look at `pitch`: a clear one forgets, anything else is kept.
    pub fn record(&self, pitch: Pitch, threat: Threat, at: Instant) {
        self.with(|m| {
            if threat == Threat::Clear {
                m.remove(&pitch);
            } else {
                m.insert(pitch, (at, threat));
            }
        });
    }

    /// What was last seen at `pitch`, if less than [`DANGER_TTL`] before
    /// `now`; else [`Threat::Clear`].
    pub fn recent(&self, pitch: Pitch, now: Instant) -> Threat {
        self.with(|m| match m.get(&pitch) {
            Some((at, t)) if now.saturating_duration_since(*at) < DANGER_TTL => *t,
            _ => Threat::Clear,
        })
    }

    /// Forget everything (tests).
    pub fn clear(&self) {
        self.with(BTreeMap::clear);
    }
}

/// The process's memory of danger.
pub static DANGER: Dangers = Dangers::new();

/// **Look around** (the player's way): the grid's pilots from the park,
/// and the 360 degree directional scan when the server serves it. Records
/// the look on [`DANGER`]. In space, bound.
pub fn look(c: &mut Conn, map: Option<&Map>) -> Result<Threat> {
    let server = c.socket()?.peer_addr()?;
    let system = c
        .session()
        .solar_system_id()
        .ok_or_else(|| Error::NotSent("look: not in space".to_owned()))?;
    let space = map.map_or(Space::Unknown, |m| space_of(m, system));
    let me = Me {
        char_id: c.session().char_id().unwrap_or_default(),
        corp_id: c.session().get_i64("corpid"),
    };
    let pilots: Vec<Pilot> = c
        .world_now()
        .balls()
        .filter_map(|b| {
            let s = b.slim?;
            Some(Pilot {
                char_id: s.char_id?,
                corp_id: s.corp_id,
                security_status: s.security_status,
            })
        })
        .collect();
    let scan = match dscan(c) {
        Ok(rows) => rows
            .iter()
            .filter(|r| PILOT_SHIP_GROUPS.contains(&r.group_id))
            .count() as u32,
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        // Not served, or docked: the grid is all there is.
        Err(_) => 0,
    };
    let threat = assess(me, space, &pilots, scan);
    DANGER.record((server, system), threat, Instant::now());
    Ok(threat)
}

/// One row of a directional scan: `.id`, `.typeID`, `.groupID`
/// (`scan::cone_scan` [V]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanRow {
    pub id: i64,
    pub type_id: i64,
    pub group_id: i64,
}

/// **The directional scan**: `ConeScan(angle, range, x, y, z)` on the
/// system scan manager, a full circle at the client's longest range.
pub fn dscan(c: &mut Conn) -> Result<Vec<ScanRow>> {
    let manager = c.system_scan_manager()?;
    let outcome = c.call_on(
        &manager,
        "ConeScan",
        vec![
            PyValue::Real(DSCAN_ANGLE),
            PyValue::Real(DSCAN_RANGE),
            PyValue::Real(0.0),
            PyValue::Real(0.0),
            PyValue::Real(1.0),
        ],
        vec![],
    )?;
    match outcome.value() {
        Some(PyValue::List(rows)) => Ok(rows.iter().filter_map(scan_row).collect()),
        other => Err(Error::Refused(format!("ConeScan answered {other:?}"))),
    }
}

/// One row (pure).
pub fn scan_row(v: &PyValue) -> Option<ScanRow> {
    Some(ScanRow {
        id: crate::probe::int(crate::probe::get(v, "id"))?,
        type_id: crate::probe::int(crate::probe::get(v, "typeID"))?,
        group_id: crate::probe::int(crate::probe::get(v, "groupID"))?,
    })
}

// ---- planning the loop ----------------------------------------------------------------

/// How far from home an explorer roams, in jumps **[A]**: three, one more
/// than the prober's old two. Falsified by explorers that are out of their
/// session window for most of a trip (too high) or that never leave the
/// first ring (too low).
pub const ROAM_JUMPS: u32 = 3;

/// Systems in one loop, besides the home system **[A]**.
pub const STOPS_MAX: usize = 3;

/// The most gates one loop may cost, out and back **[A]**.
pub const TOUR_JUMPS: u32 = 8;

/// What an unworked data or relic signature on the ledger adds to a
/// system's value, against a system nobody scanned lately (one) **[A]**.
pub const BOOKED_WEIGHT: f64 = 2.0;

/// Sites hacked in one system before the bot moves on **[A]**: a player
/// works a couple and goes. Falsified by explorers that leave a system with
/// sites unworked and an empty hold (too low) or sit in one for an hour
/// (too high). A dry system (nothing left to scan) is left at once.
pub const SITES_PER_SYSTEM: usize = 2;

/// Sites hacked in one trip before it turns for home **[A]**.
pub const SITES_PER_TRIP: usize = 6;

/// The longest a roaming trip lasts **[A]**: a player's evening.
pub const TRIP_CEILING: Duration = Duration::from_secs(90 * 60);

/// What the bot knows of a system it might visit.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Intel {
    /// Unworked data and relic signatures the ledger holds there.
    pub booked: usize,
    /// Not scanned out within [`crate::probe::REVISIT`].
    pub fresh: bool,
    /// What was last seen there.
    pub threat: Threat,
}

impl Intel {
    /// What the system is worth a visit (zero: not worth one).
    pub fn value(&self) -> f64 {
        self.booked as f64 * BOOKED_WEIGHT + if self.fresh { 1.0 } else { 0.0 }
    }
}

/// Every system within [`ROAM_JUMPS`] of `home` reachable from `start` at
/// `floor`, with its jumps from `home`; ascending by id.
pub fn ring(map: &Map, floor: f64, home: i64, start: i64) -> BTreeMap<i64, u32> {
    let mut seen: BTreeMap<i64, u32> = BTreeMap::from([(start, 0)]);
    let mut queue = VecDeque::from([start]);
    while let Some(s) = queue.pop_front() {
        for n in map.neighbours(s) {
            if seen.contains_key(&n) || !map.allowed(n, floor) {
                continue;
            }
            if map.jumps(home, n, floor).is_none_or(|j| j > ROAM_JUMPS) {
                continue;
            }
            seen.insert(n, 0);
            queue.push_back(n);
        }
    }
    seen.remove(&start);
    seen.keys()
        .map(|&s| (s, map.jumps(home, s, floor).unwrap_or(u32::MAX)))
        .collect()
}

/// **The loop** (pure): up to [`STOPS_MAX`] systems, each the best value
/// per jump from the last, none the bot avoids ([`avoid`]), none worth
/// nothing, the whole loop (out and back to `home`) within
/// [`TOUR_JUMPS`]. `start` is where the loop begins (the home system on a
/// fresh trip). Ties go to the lower system id.
pub fn plan_tour(
    map: &Map,
    nerve: Nerve,
    home: i64,
    start: i64,
    intel: impl Fn(i64) -> Intel,
) -> Vec<i64> {
    let floor = floor_for(nerve);
    let ring = ring(map, floor, home, start);
    let mut stops: Vec<i64> = Vec::new();
    let mut at = start;
    let mut spent = 0u32;
    while stops.len() < STOPS_MAX {
        let mut best: Option<(f64, i64, u32)> = None;
        for &cand in ring.keys() {
            if stops.contains(&cand) || cand == home && start == home {
                continue;
            }
            let i = intel(cand);
            if i.value() <= 0.0 || avoid(i.threat, nerve) {
                continue;
            }
            let Some(leg) = map.jumps(at, cand, floor) else {
                continue;
            };
            let Some(back) = map.jumps(cand, home, floor) else {
                continue;
            };
            if spent + leg + back > TOUR_JUMPS {
                continue;
            }
            let score = i.value() / (1.0 + f64::from(leg));
            if best.is_none_or(|(s, _, _)| score > s) {
                best = Some((score, cand, leg));
            }
        }
        let Some((_, cand, leg)) = best else {
            break;
        };
        stops.push(cand);
        spent += leg;
        at = cand;
    }
    stops
}

/// **The next system for the prober** (pure): the best of [`plan_tour`]'s
/// first stop, so a prober and a hacker choose by one rule.
pub fn next_stop(
    map: &Map,
    nerve: Nerve,
    home: i64,
    here: i64,
    intel: impl Fn(i64) -> Intel,
) -> Option<i64> {
    plan_tour(map, nerve, home, here, intel).first().copied()
}

// ---- when to turn for home ---------------------------------------------------------------

/// The value of the hold, in ISK at the server's reference prices, at
/// which a trip turns for home **[A]**: a few hundred thousand a stack
/// is a good evening for a Heron. Falsified by hackers that come home
/// with a near-empty hold every time (too low) or lose a full one (too
/// high).
pub const RETURN_VALUE: f64 = 2_000_000.0;

/// Why a trip turns for home, if it does (pure).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    Full,
    Valuable,
    Sites,
    Late,
}

/// **Should the trip turn for home now** (pure).
pub fn should_return(full: bool, hold_value: f64, sites: usize, elapsed: Duration) -> Option<Turn> {
    if full {
        Some(Turn::Full)
    } else if hold_value >= RETURN_VALUE {
        Some(Turn::Valuable)
    } else if sites >= SITES_PER_TRIP {
        Some(Turn::Sites)
    } else if elapsed >= TRIP_CEILING {
        Some(Turn::Late)
    } else {
        None
    }
}

// ---- where to sell ------------------------------------------------------------------------

/// How much a listing is worth against ISK in hand, because it sells
/// slowly or not at all **[A]**. Falsified by listings that sell within
/// the hour (too low) or sit for the week (too high).
pub const LIST_WORTH: f64 = 0.5;

/// What each extra jump costs the proceeds, as a share **[A]**: the risk
/// and the hour. Falsified by hackers that cross three systems for a few
/// percent (too low) or never use a hub that pays twice as much (too high).
pub const JUMP_COST_SHARE: f64 = 0.04;

/// A hub must beat home by this share, after the jumps **[A]**.
pub const HUB_MARGIN: f64 = 0.10;

/// The farthest hub the hackers consider, in jumps from where they are
/// **[A]**.
pub const OUTLET_JUMPS: u32 = 3;

/// One place the hold could be sold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outlet {
    pub station: i64,
    pub system: i64,
    /// Jumps from where the bot is now.
    pub jumps: u32,
    /// ISK in hand from bids here, after the sales tax.
    pub cash: f64,
    /// The proceeds that count: cash, the rest listed at home at
    /// [`LIST_WORTH`], less the jumps' cost.
    pub score: f64,
}

/// What bids at one station pay for one stack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fill {
    /// Units the bids take (at most the stack).
    pub units: i64,
    /// The lowest price among the bids used: the stack's sell price.
    pub floor: f64,
    /// ISK in hand, after the sales tax.
    pub cash: f64,
}

/// **The bids at `station` against `qty` units** (pure), the dearest
/// first. `None`: no bid takes a unit.
pub fn bid_fill(book: &Book, station: i64, qty: i64) -> Option<Fill> {
    let mut bids: Vec<_> = book
        .bids
        .iter()
        .filter(|b| b.station_id == station && b.price >= crate::pricing::TICK)
        .collect();
    bids.sort_by(|a, b| {
        b.price
            .total_cmp(&a.price)
            .then(a.order_id.cmp(&b.order_id))
    });
    let mut left = qty;
    let mut cash = 0.0;
    let mut floor = f64::MAX;
    for b in bids {
        if left <= 0 {
            break;
        }
        let take = left.min(b.vol_remaining.max(0));
        if take == 0 {
            continue;
        }
        cash += take as f64 * b.price;
        floor = floor.min(b.price);
        left -= take;
    }
    (left < qty).then_some(Fill {
        units: qty - left,
        floor,
        cash: cash * (1.0 - crate::pricing::SALES_TAX),
    })
}

/// **Where the hold fetches most** (pure): `stacks` (type, units) sold at
/// home -- bids there, the rest listed at `P_ref` -- or at any station with
/// a bid for one of them, within [`OUTLET_JUMPS`] of `from` at `floor`.
/// Best first; home is always on the list. A hub's score is its cash, plus
/// the unfilled remainder listed at home at [`LIST_WORTH`], less
/// [`JUMP_COST_SHARE`] per jump beyond the way home. It leads only when it
/// beats home by [`HUB_MARGIN`].
pub fn rank_outlets(
    stacks: &[(i64, i64)],
    books: &BTreeMap<i64, Book>,
    p_refs: &BTreeMap<i64, f64>,
    map: &Map,
    floor: f64,
    from: i64,
    home_station: i64,
) -> Vec<Outlet> {
    let home_system = map.system_of(home_station);
    let list_value = |type_id: i64, units: i64| -> f64 {
        p_refs
            .get(&type_id)
            .and_then(|&p| crate::pricing::ask(&crate::pricing::Quote::at_ref(p)))
            .map_or(0.0, |ask| ask * units as f64 * LIST_WORTH)
    };
    // (cash, listed remainder) selling `stacks` with the bids at `station`.
    let sell_at = |station: i64| -> (f64, f64) {
        let mut cash = 0.0;
        let mut rest = 0.0;
        for &(t, q) in stacks {
            let fill = books.get(&t).and_then(|b| bid_fill(b, station, q));
            cash += fill.map_or(0.0, |f| f.cash);
            rest += list_value(t, q - fill.map_or(0, |f| f.units));
        }
        (cash, rest)
    };
    let way = |a: i64, b: i64| {
        map.jumps(a, b, floor)
            .or_else(|| map.jumps(a, b, ANY_SECURITY))
    };
    let home_jumps = home_system.and_then(|h| way(from, h)).unwrap_or(0);
    let (home_cash, home_rest) = sell_at(home_station);
    let mut out = vec![Outlet {
        station: home_station,
        system: home_system.unwrap_or(from),
        jumps: home_jumps,
        cash: home_cash,
        score: home_cash + home_rest,
    }];
    let mut candidates: BTreeSet<i64> = BTreeSet::new();
    for &(t, _) in stacks {
        for b in books.get(&t).into_iter().flat_map(|b| &b.bids) {
            if b.station_id != home_station && b.price >= crate::pricing::TICK {
                candidates.insert(b.station_id);
            }
        }
    }
    for station in candidates {
        let Some(system) = map.system_of(station) else {
            continue;
        };
        let Some(jumps) = way(from, system) else {
            continue;
        };
        if jumps > OUTLET_JUMPS {
            continue;
        }
        let extra = (jumps + home_system.and_then(|h| way(system, h)).unwrap_or(0))
            .saturating_sub(home_jumps);
        // What this station does not buy is carried home and listed there.
        let (cash, rest) = sell_at(station);
        let score = (cash + rest) * (1.0 - JUMP_COST_SHARE * f64::from(extra));
        out.push(Outlet {
            station,
            system,
            jumps,
            cash,
            score,
        });
    }
    // A hub leads only by the margin; home leads otherwise.
    let home_score = out[0].score;
    out.sort_by(|a, b| {
        let beats =
            |o: &Outlet| o.station == home_station || o.score >= home_score * (1.0 + HUB_MARGIN);
        beats(b)
            .cmp(&beats(a))
            .then(b.score.total_cmp(&a.score))
            .then(a.station.cmp(&b.station))
    });
    out
}

// ---- the loot's fate -----------------------------------------------------------------------

/// `groupDatacores` **[V-data]**: what inventors consume.
pub const GROUP_DATACORE: i64 = 333;
/// `categoryDecryptors` **[A]**: the public inventory category of
/// decryptors. Falsified by a decryptor stack that the hacker sells into a
/// bid while its corp has a builder.
pub const CATEGORY_DECRYPTOR: i64 = 35;

/// Is `row` something a builder uses in invention: a datacore or a
/// decryptor.
pub fn is_builder_input(row: &ItemRow) -> bool {
    row.group_id == GROUP_DATACORE || row.category_id == CATEGORY_DECRYPTOR
}

/// **Is there a builder in the bot's corporation** (pure, from the hall's
/// state): another member, in a corporation a bot runs (never the NPC
/// corporation every corpless bot shares), whose career is the builder's.
pub fn builder_in_corp(hall: &crate::corpmind::HallState, me: &str) -> bool {
    let Some(mine) = hall.members.values().find(|m| m.name == me) else {
        return false;
    };
    hall.bot_corps().contains(&mine.corp)
        && hall
            .members
            .values()
            .any(|m| m.corp == mine.corp && m.name != me && m.career == "builder")
}

// ---- the trip's state, across a reconnect ------------------------------------------------------

/// Where the trip is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Visiting the stops.
    Out,
    /// Turned for home: sell, then dock.
    Back,
}

/// **A roaming trip's state**: enough to carry on after the connection
/// dropped. Written to `<state dir>/<character>.roam.tsv`
/// ([`file_name`]) and kept in memory ([`put`]).
#[derive(Debug, Clone, PartialEq)]
pub struct TripState {
    pub name: String,
    pub home_station: i64,
    pub home_system: i64,
    /// Unix seconds.
    pub started: u64,
    pub phase: Phase,
    /// The systems still to visit, in order.
    pub stops: Vec<i64>,
    /// The systems worked or looked at already.
    pub visited: Vec<i64>,
    /// `(system, target id)` of the sites hacked.
    pub sites: Vec<(i64, String)>,
    /// The types looted so far.
    pub looted: BTreeSet<i64>,
    pub units: i64,
    /// The wallet when the trip began.
    pub before: f64,
}

/// The state file's version.
pub const STATE_VERSION: u32 = 1;

/// A trip state older than this is dropped, not resumed **[A]**: the
/// ceiling and a reconnect's slack.
pub const STALE: Duration = Duration::from_secs(2 * 60 * 60);

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\t' => o.push_str("\\t"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            c => o.push(c),
        }
    }
    o
}

fn unesc(s: &str) -> std::result::Result<String, String> {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            o.push(c);
            continue;
        }
        match it.next() {
            Some('\\') => o.push('\\'),
            Some('t') => o.push('\t'),
            Some('n') => o.push('\n'),
            Some('r') => o.push('\r'),
            other => return Err(format!("bad escape {other:?}")),
        }
    }
    Ok(o)
}

/// The state file's name for `name`: the mind file's, `.roam.tsv`.
pub fn file_name(name: &str) -> String {
    let base = crate::state::file_name(name);
    format!("{}.roam.tsv", base.trim_end_matches(".tsv"))
}

impl TripState {
    /// A new trip leaving `home_station` in `home_system`.
    pub fn new(
        name: &str,
        home_station: i64,
        home_system: i64,
        now: u64,
        before: f64,
    ) -> TripState {
        TripState {
            name: name.to_owned(),
            home_station,
            home_system,
            started: now,
            phase: Phase::Out,
            stops: Vec::new(),
            visited: Vec::new(),
            sites: Vec::new(),
            looted: BTreeSet::new(),
            units: 0,
            before,
        }
    }

    /// Too old to carry on (pure).
    pub fn stale(&self, now: u64) -> bool {
        now.saturating_sub(self.started) >= STALE.as_secs()
    }

    /// The file's text: a header, one row per fact, and an `end` row that
    /// counts them, so a torn write is told from a whole one.
    pub fn render(&self) -> String {
        let mut rows = vec![
            format!("name\t{}", esc(&self.name)),
            format!("home\t{}\t{}", self.home_station, self.home_system),
            format!("started\t{}", self.started),
            format!(
                "phase\t{}",
                match self.phase {
                    Phase::Out => "out",
                    Phase::Back => "back",
                }
            ),
            format!("units\t{}", self.units),
            format!("before\t{:?}", self.before),
        ];
        rows.extend(self.stops.iter().map(|s| format!("stop\t{s}")));
        rows.extend(self.visited.iter().map(|s| format!("visited\t{s}")));
        rows.extend(
            self.sites
                .iter()
                .map(|(s, t)| format!("site\t{s}\t{}", esc(t))),
        );
        rows.extend(self.looted.iter().map(|t| format!("loot\t{t}")));
        let mut o = format!("roam\t{STATE_VERSION}\n");
        for r in &rows {
            o.push_str(r);
            o.push('\n');
        }
        let _ = writeln!(o, "end\t{}", rows.len());
        o
    }

    /// Parse [`Self::render`]'s text; anything torn is an error.
    pub fn parse(src: &str) -> std::result::Result<TripState, String> {
        let lines: Vec<&str> = src.lines().collect();
        if lines.first() != Some(&format!("roam\t{STATE_VERSION}").as_str()) {
            return Err("no roam header of this version".into());
        }
        let n: usize = lines
            .last()
            .and_then(|l| l.strip_prefix("end\t"))
            .and_then(|n| n.parse().ok())
            .ok_or("no end row")?;
        if n + 2 != lines.len() {
            return Err(format!(
                "end says {n} rows, the file has {}",
                lines.len().saturating_sub(2)
            ));
        }
        let num = |f: Option<&&str>, what: &str| -> std::result::Result<i64, String> {
            f.and_then(|s| s.parse().ok())
                .ok_or_else(|| format!("bad {what}"))
        };
        let mut name = None;
        let mut home = None;
        let mut started = None;
        let mut phase = None;
        let mut t = TripState::new("", 0, 0, 0, 0.0);
        for line in &lines[1..lines.len() - 1] {
            let f: Vec<&str> = line.split('\t').collect();
            match f[0] {
                "name" => name = Some(unesc(f.get(1).ok_or("no name")?)?),
                "home" => {
                    home = Some((
                        num(f.get(1), "home station")?,
                        num(f.get(2), "home system")?,
                    ))
                }
                "started" => started = Some(num(f.get(1), "started")?.max(0) as u64),
                "phase" => {
                    phase = Some(match f.get(1).copied() {
                        Some("out") => Phase::Out,
                        Some("back") => Phase::Back,
                        other => return Err(format!("unknown phase {other:?}")),
                    })
                }
                "units" => t.units = num(f.get(1), "units")?,
                "before" => t.before = f.get(1).and_then(|s| s.parse().ok()).ok_or("bad wallet")?,
                "stop" => t.stops.push(num(f.get(1), "stop")?),
                "visited" => t.visited.push(num(f.get(1), "visited")?),
                "site" => t.sites.push((
                    num(f.get(1), "site system")?,
                    unesc(f.get(2).ok_or("no site target")?)?,
                )),
                "loot" => {
                    t.looted.insert(num(f.get(1), "loot")?);
                }
                other => return Err(format!("unknown row {other:?}")),
            }
        }
        t.name = name.ok_or("no name row")?;
        (t.home_station, t.home_system) = home.ok_or("no home row")?;
        t.started = started.ok_or("no started row")?;
        t.phase = phase.ok_or("no phase row")?;
        Ok(t)
    }
}

/// The trips in memory, by character: a reconnect in this process finds
/// its trip without the disk.
static TRIPS: Mutex<BTreeMap<String, TripState>> = Mutex::new(BTreeMap::new());

/// The state directories the driver attached ([`attach`]).
static DIRS: Mutex<BTreeMap<String, PathBuf>> = Mutex::new(BTreeMap::new());

fn path_of(dir: &Path, name: &str) -> PathBuf {
    dir.join(file_name(name))
}

/// Unix seconds now.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// **Keep the trip** in memory and, with a directory, in its file
/// (temp-and-rename, so a crash leaves the old file or the new one). A
/// failed write is not a failed trip: the memory copy holds.
pub fn put(dir: Option<&Path>, st: &TripState) {
    if let Ok(mut m) = TRIPS.lock() {
        m.insert(st.name.clone(), st.clone());
    }
    let Some(dir) = dir else {
        return;
    };
    let path = path_of(dir, &st.name);
    let tmp = path.with_extension("tsv.tmp");
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(st.render().as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)
    };
    let _ = write();
}

/// **The trip of `name`**: the memory copy, else the file. A torn or
/// unreadable file is removed and reads as no trip. Stale trips are
/// dropped ([`TripState::stale`]).
pub fn get(dir: Option<&Path>, name: &str, now: u64) -> Option<TripState> {
    let mem = TRIPS.lock().ok().and_then(|m| m.get(name).cloned());
    let found = mem.or_else(|| {
        let path = path_of(dir?, name);
        let text = std::fs::read_to_string(&path).ok()?;
        match TripState::parse(&text) {
            Ok(t) if t.name == name => Some(t),
            _ => {
                let _ = std::fs::remove_file(&path);
                None
            }
        }
    });
    match found {
        Some(t) if t.stale(now) => {
            clear(dir, name);
            None
        }
        other => other,
    }
}

/// **The trip is over** (docked, or given up): memory and file go.
pub fn clear(dir: Option<&Path>, name: &str) {
    if let Ok(mut m) = TRIPS.lock() {
        m.remove(name);
    }
    if let Some(dir) = dir {
        let _ = std::fs::remove_file(path_of(dir, name));
    }
}

/// **The driver, at logon**: remember where `name`'s state directory is,
/// and say whether a trip is waiting to be carried on. A bot with one
/// carries on instead of being sent home.
pub fn attach(name: &str, dir: Option<&Path>) -> bool {
    if let (Some(d), Ok(mut m)) = (dir, DIRS.lock()) {
        m.insert(name.to_owned(), d.to_path_buf());
    }
    resumable(name)
}

/// Is a fresh trip waiting for `name` (memory, or the file in the
/// attached directory)?
pub fn resumable(name: &str) -> bool {
    let dir = DIRS.lock().ok().and_then(|m| m.get(name).cloned());
    get(dir.as_deref(), name, unix_now()).is_some()
}

/// **Give the trip up** (the bot is sent home the plain way).
pub fn abandon(name: &str) {
    let dir = DIRS.lock().ok().and_then(|m| m.get(name).cloned());
    clear(dir.as_deref(), name);
}

/// The directory attached for `name`, if any.
pub fn attached(name: &str) -> Option<PathBuf> {
    DIRS.lock().ok().and_then(|m| m.get(name).cloned())
}

// ---- tests --------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verbs::BookOrder;

    // Invented ids throughout.
    const HOME: i64 = 7_000_001;
    const A: i64 = 7_000_002;
    const B: i64 = 7_000_003;
    const C: i64 = 7_000_004;
    const FAR: i64 = 7_000_009;
    const STATION: i64 = 7_600_001;

    fn nerve(v: f64) -> Nerve {
        Nerve {
            risk: v,
            aggression: v,
        }
    }

    /// A line `HOME - A - B - C - FAR` with the given securities.
    fn line(sec: [f64; 5]) -> Map {
        let ids = [HOME, A, B, C, FAR];
        let mut m = Map::new().with_station(STATION, HOME);
        for (i, &s) in ids.iter().enumerate() {
            m = m.with_security(s, sec[i]);
            if i > 0 {
                let (p, q) = (ids[i - 1], s);
                m = m.with_gate(p, 100 + i as i64, q, 200 + i as i64).with_gate(
                    q,
                    200 + i as i64,
                    p,
                    100 + i as i64,
                );
            }
        }
        m
    }

    fn fresh(booked: usize) -> Intel {
        Intel {
            booked,
            fresh: true,
            threat: Threat::Clear,
        }
    }

    #[test]
    fn nerve_cuts_the_security_bands() {
        let timid = nerve(LOW_NERVE - 0.01);
        let middling = nerve((LOW_NERVE + NULL_NERVE) / 2.0);
        let reckless = nerve(NULL_NERVE + 0.01);
        assert_eq!(floor_for(timid), HIGH_FLOOR);
        assert_eq!(floor_for(middling), LOW_FLOOR);
        assert_eq!(floor_for(reckless), NULL_FLOOR);
        // The risk trait outweighs the temper.
        let n = Nerve {
            risk: 0.0,
            aggression: 1.0,
        };
        assert!(n.value() < 0.5);
        assert!(Nerve::NEUTRAL.value() > 0.0);
        assert!(nerve(FIGHTER_AGGRESSION).fighter());
        assert!(!nerve(FIGHTER_AGGRESSION - 0.01).fighter());
    }

    #[test]
    fn the_map_sorts_systems_into_bands() {
        let m = line([0.9, 0.3, -0.2, 0.9, 0.9]).with_security(FAR, 0.9);
        assert_eq!(space_of(&m, HOME), Space::High);
        assert_eq!(space_of(&m, A), Space::Low);
        assert_eq!(space_of(&m, B), Space::Null);
        assert_eq!(space_of(&m, 1), Space::Unknown);
    }

    #[test]
    fn data_and_relic_are_worked_and_the_rest_booked() {
        assert_eq!(job_for(Some(Kind::Data), true), Job::Work);
        assert_eq!(job_for(Some(Kind::Relic), true), Job::Work);
        assert_eq!(job_for(None, true), Job::Work, "scan on to learn the kind");
        assert_eq!(job_for(Some(Kind::Data), false), Job::Leave, "no analyzer");
        for k in [
            Kind::Gas,
            Kind::Combat,
            Kind::Ore,
            Kind::Wormhole,
            Kind::Unknown,
        ] {
            assert_eq!(job_for(Some(k), true), Job::Leave, "{k:?}");
        }
    }

    #[test]
    fn a_timid_loop_stays_in_high_sec_and_a_bold_one_goes_on() {
        // HOME high, A low, B low, C high.
        let m = line([0.9, 0.3, 0.2, 0.9, 0.9]);
        let all = |_: i64| fresh(0);
        let timid = plan_tour(&m, nerve(LOW_NERVE - 0.01), HOME, HOME, all);
        assert!(
            timid.is_empty(),
            "A is low-sec, the only way out: {timid:?}"
        );
        let mid = nerve((LOW_NERVE + NULL_NERVE) / 2.0);
        let tour = plan_tour(&m, mid, HOME, HOME, all);
        assert_eq!(tour.first(), Some(&A), "nearest first: {tour:?}");
        assert!(tour.len() <= STOPS_MAX);
        // Null in the way: only the reckless pass.
        let m = line([0.9, 0.3, -0.5, 0.3, 0.3]);
        let mid_tour = plan_tour(&m, mid, HOME, HOME, all);
        assert_eq!(mid_tour, vec![A], "B is null: {mid_tour:?}");
        let wild = plan_tour(&m, nerve(NULL_NERVE + 0.01), HOME, HOME, all);
        assert!(wild.contains(&B), "{wild:?}");
    }

    #[test]
    fn booked_signatures_pull_the_loop_and_empty_systems_are_skipped() {
        let m = line([0.9, 0.9, 0.9, 0.9, 0.9]);
        let n = nerve(0.5);
        // A is stale and empty; C has two booked signatures.
        let intel = |s: i64| match s {
            A => Intel {
                booked: 0,
                fresh: false,
                threat: Threat::Clear,
            },
            C => fresh(2),
            _ => fresh(0),
        };
        let tour = plan_tour(&m, n, HOME, HOME, intel);
        assert!(!tour.contains(&A), "{tour:?}");
        assert_eq!(
            tour.first(),
            Some(&C),
            "two booked sites beat the nearer blank: {tour:?}"
        );
        assert!(tour.contains(&B), "{tour:?}");
        let c_first = plan_tour(
            &m,
            n,
            HOME,
            HOME,
            |s| if s == C { fresh(9) } else { fresh(0) },
        );
        assert_eq!(c_first.first(), Some(&C), "value per jump: {c_first:?}");
    }

    #[test]
    fn a_loop_stays_inside_the_ring_and_the_gate_budget() {
        let m = line([0.9; 5]);
        let tour = plan_tour(&m, nerve(0.5), HOME, HOME, |_| fresh(1));
        assert!(!tour.contains(&FAR), "FAR is {} jumps out", ROAM_JUMPS + 1);
        let mut spent = 0;
        let mut at = HOME;
        for &s in &tour {
            spent += m.jumps(at, s, ANY_SECURITY).unwrap();
            at = s;
        }
        spent += m.jumps(at, HOME, ANY_SECURITY).unwrap();
        assert!(spent <= TOUR_JUMPS, "{tour:?} costs {spent}");
        // The ring itself.
        let r = ring(&m, ANY_SECURITY, HOME, HOME);
        assert_eq!(r.get(&A), Some(&1));
        assert_eq!(r.get(&C), Some(&3));
        assert!(!r.contains_key(&FAR));
    }

    #[test]
    fn systems_with_hostiles_are_avoided_by_the_nervous_only() {
        let m = line([0.3; 5]);
        let hot = |s: i64| {
            if s == A {
                Intel {
                    booked: 5,
                    fresh: true,
                    threat: Threat::Hostile(2),
                }
            } else {
                fresh(1)
            }
        };
        let mid = nerve((BOLD_NERVE + FEARLESS_NERVE) / 2.0);
        assert!(!plan_tour(&m, mid, HOME, HOME, hot).contains(&A));
        let brave = nerve(FEARLESS_NERVE + 0.01);
        assert_eq!(plan_tour(&m, brave, HOME, HOME, hot).first(), Some(&A));
        assert_eq!(next_stop(&m, mid, HOME, HOME, hot), Some(B));
    }

    fn pilot(char_id: i64, corp: i64, sec: f64) -> Pilot {
        Pilot {
            char_id,
            corp_id: Some(corp),
            security_status: Some(sec),
        }
    }

    #[test]
    fn assessing_a_system_the_players_way() {
        let me = Me {
            char_id: 1,
            corp_id: Some(10),
        };
        let mate = pilot(2, 10, 0.0);
        let stranger = pilot(3, 11, 0.0);
        let outlaw = pilot(4, 11, DANGEROUS_STATUS - 0.5);
        let myself = pilot(1, 10, 0.0);
        assert_eq!(assess(me, Space::Null, &[myself, mate], 0), Threat::Clear);
        assert_eq!(
            assess(me, Space::Null, &[myself, mate, stranger], 0),
            Threat::Hostile(1)
        );
        // In high-sec a stranger is nothing; a flagged one is a threat.
        assert_eq!(assess(me, Space::High, &[stranger], 4), Threat::Clear);
        assert_eq!(
            assess(me, Space::High, &[stranger, outlaw], 0),
            Threat::Hostile(1)
        );
        // Company on the scan counts in low and null only.
        assert_eq!(assess(me, Space::Low, &[], 3), Threat::Company(3));
        assert_eq!(assess(me, Space::High, &[], 3), Threat::Clear);
        // No corporation known: nobody is a mate.
        let lone = Me {
            char_id: 1,
            corp_id: None,
        };
        assert_eq!(assess(lone, Space::Low, &[mate], 0), Threat::Hostile(1));
    }

    #[test]
    fn nerve_decides_what_it_avoids() {
        assert!(!avoid(Threat::Clear, nerve(0.0)));
        assert!(avoid(Threat::Company(1), nerve(BOLD_NERVE - 0.01)));
        assert!(!avoid(Threat::Company(1), nerve(BOLD_NERVE)));
        assert!(avoid(Threat::Hostile(1), nerve(BOLD_NERVE)));
        assert!(!avoid(Threat::Hostile(1), nerve(FEARLESS_NERVE)));
    }

    #[test]
    fn what_was_seen_is_believed_for_a_while() {
        let d = Dangers::new();
        let server: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
        let t0 = Instant::now();
        d.record((server, A), Threat::Hostile(2), t0);
        assert_eq!(d.recent((server, A), t0), Threat::Hostile(2));
        assert_eq!(d.recent((server, B), t0), Threat::Clear);
        assert_eq!(d.recent((server, A), t0 + DANGER_TTL), Threat::Clear);
        d.record((server, A), Threat::Company(1), t0);
        d.record((server, A), Threat::Clear, t0);
        assert_eq!(
            d.recent((server, A), t0),
            Threat::Clear,
            "a clear look forgets"
        );
    }

    #[test]
    fn a_trip_turns_for_home_when_full_valuable_busy_or_late() {
        let none = Duration::ZERO;
        assert_eq!(should_return(false, 0.0, 0, none), None);
        assert_eq!(should_return(true, 0.0, 0, none), Some(Turn::Full));
        assert_eq!(
            should_return(false, RETURN_VALUE, 0, none),
            Some(Turn::Valuable)
        );
        assert_eq!(
            should_return(false, 0.0, SITES_PER_TRIP, none),
            Some(Turn::Sites)
        );
        assert_eq!(should_return(false, 0.0, 0, TRIP_CEILING), Some(Turn::Late));
    }

    fn bid(order: i64, type_id: i64, station: i64, price: f64, vol: i64) -> BookOrder {
        BookOrder {
            order_id: order,
            type_id,
            station_id: station,
            price,
            vol_remaining: vol,
            bid: true,
        }
    }

    const CORE: i64 = 990_001;
    const HUB_STATION: i64 = 7_600_002;
    const NEAR_STATION: i64 = 7_600_003;

    /// HOME - A (hub) and HOME - B (near).
    fn star() -> Map {
        Map::new()
            .with_station(STATION, HOME)
            .with_station(HUB_STATION, A)
            .with_station(NEAR_STATION, B)
            .with_security(HOME, 0.9)
            .with_security(A, 0.9)
            .with_security(B, 0.9)
            .with_gate(HOME, 1, A, 2)
            .with_gate(A, 2, HOME, 1)
            .with_gate(HOME, 3, B, 4)
            .with_gate(B, 4, HOME, 3)
    }

    #[test]
    fn bids_are_taken_dearest_first_net_of_tax() {
        let book = Book {
            asks: vec![],
            bids: vec![
                bid(1, CORE, STATION, 10.0, 5),
                bid(2, CORE, STATION, 20.0, 3),
                bid(3, CORE, HUB_STATION, 99.0, 100),
            ],
        };
        let fill = bid_fill(&book, STATION, 6).expect("bids take it");
        assert_eq!(fill.units, 6);
        assert_eq!(fill.floor, 10.0, "the dearest bid first, then the next");
        let expect = (3.0 * 20.0 + 3.0 * 10.0) * (1.0 - crate::pricing::SALES_TAX);
        assert!(
            (fill.cash - expect).abs() < 1e-9,
            "{} vs {expect}",
            fill.cash
        );
        let all = bid_fill(&book, STATION, 100).expect("some of it");
        assert_eq!(all.units, 8, "only what the bids hold");
        assert_eq!(bid_fill(&book, 1, 5), None, "no bid there");
    }

    #[test]
    fn a_hub_that_pays_more_leads_and_one_that_barely_does_not() {
        let m = star();
        let refs: BTreeMap<i64, f64> = BTreeMap::from([(CORE, 100.0)]);
        let books = |home_bid: f64, hub_bid: f64| {
            BTreeMap::from([(
                CORE,
                Book {
                    asks: vec![],
                    bids: vec![
                        bid(1, CORE, STATION, home_bid, 1_000),
                        bid(2, CORE, HUB_STATION, hub_bid, 1_000),
                    ],
                },
            )])
        };
        let stacks = [(CORE, 100)];
        // Twice the price one jump away: the hub leads, home is still listed.
        let ranked = rank_outlets(
            &stacks,
            &books(50.0, 100.0),
            &refs,
            &m,
            ANY_SECURITY,
            HOME,
            STATION,
        );
        assert_eq!(ranked[0].station, HUB_STATION, "{ranked:?}");
        assert!(ranked.iter().any(|o| o.station == STATION));
        assert_eq!(ranked[0].jumps, 1);
        // A cent more is not worth the gate.
        let ranked = rank_outlets(
            &stacks,
            &books(100.0, 101.0),
            &refs,
            &m,
            ANY_SECURITY,
            HOME,
            STATION,
        );
        assert_eq!(ranked[0].station, STATION, "{ranked:?}");
        // Nobody bids: home, where the rest is listed.
        let ranked = rank_outlets(
            &stacks,
            &BTreeMap::new(),
            &refs,
            &m,
            ANY_SECURITY,
            HOME,
            STATION,
        );
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].station, STATION);
        assert!(ranked[0].score > 0.0, "listed at P_ref: {ranked:?}");
        // A hub out of reach is not a candidate.
        let far = Map::new()
            .with_station(STATION, HOME)
            .with_station(HUB_STATION, FAR)
            .with_security(HOME, 0.9)
            .with_security(FAR, 0.9);
        let ranked = rank_outlets(
            &stacks,
            &books(1.0, 100.0),
            &refs,
            &far,
            ANY_SECURITY,
            HOME,
            STATION,
        );
        assert_eq!(ranked.len(), 1, "no route to the hub: {ranked:?}");
    }

    #[test]
    fn datacores_and_decryptors_are_a_builders_inputs() {
        let row = |group_id, category_id| ItemRow {
            item_id: 1,
            type_id: 2,
            owner_id: 3,
            location_id: 4,
            flag_id: 5,
            quantity: 6,
            group_id,
            category_id,
            singleton: false,
        };
        assert!(is_builder_input(&row(GROUP_DATACORE, 0)));
        assert!(is_builder_input(&row(0, CATEGORY_DECRYPTOR)));
        assert!(!is_builder_input(&row(0, 0)));
    }

    fn sample() -> TripState {
        let mut t = TripState::new("Invented\tPilot \\ Ünï", 5, 6, 1_000, 12_345.678);
        t.phase = Phase::Back;
        t.stops = vec![A, B];
        t.visited = vec![HOME, A];
        t.sites = vec![(A, "ABC-123".into()), (HOME, "Q\tX-9\n".into())];
        t.looted = BTreeSet::from([30, 20]);
        t.units = 77;
        t
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("eve-bot-roam-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_trip_state_round_trips_and_a_torn_one_is_refused() {
        let t = sample();
        assert_eq!(TripState::parse(&t.render()), Ok(t.clone()));
        let whole = t.render();
        let torn = [
            whole[..whole.len() / 2].to_owned(),
            whole.replace("end\t", "end\t9"),
            whole.replacen("roam\t1", "roam\t0", 1),
            whole.replace("units\t77", "units\tseventy"),
            whole.replace("phase\tback", "phase\tsideways"),
            String::new(),
        ];
        for s in torn {
            assert!(TripState::parse(&s).is_err(), "{s:?} read");
        }
    }

    #[test]
    fn the_state_file_name_is_safe_and_not_the_minds() {
        let f = file_name("Ab-9 x");
        assert_eq!(f, "Ab-9_x.roam.tsv");
        assert_ne!(f, crate::state::file_name("Ab-9 x"));
        let f = file_name("../up");
        assert!(!f.contains('/') && !f.starts_with('.'), "{f}");
    }

    #[test]
    fn a_trip_is_kept_in_memory_and_on_disk_and_dropped_when_stale_or_torn() {
        let dir = scratch("store");
        let t = sample();
        let name = t.name.clone();
        put(Some(&dir), &t);
        assert!(path_of(&dir, &name).exists());
        assert!(!path_of(&dir, &name).with_extension("tsv.tmp").exists());
        assert_eq!(get(Some(&dir), &name, t.started + 1), Some(t.clone()));
        // The disk alone (a fresh process).
        TRIPS.lock().unwrap().remove(&name);
        assert_eq!(get(Some(&dir), &name, t.started + 1), Some(t.clone()));
        // Stale: gone from both.
        let late = t.started + STALE.as_secs();
        assert_eq!(get(Some(&dir), &name, late), None);
        assert!(!path_of(&dir, &name).exists());
        // Torn: removed.
        put(Some(&dir), &t);
        TRIPS.lock().unwrap().remove(&name);
        let text = std::fs::read_to_string(path_of(&dir, &name)).unwrap();
        std::fs::write(path_of(&dir, &name), &text[..text.len() / 2]).unwrap();
        assert_eq!(get(Some(&dir), &name, t.started + 1), None);
        assert!(!path_of(&dir, &name).exists());
        // Without a directory the memory is all there is.
        put(None, &t);
        assert_eq!(get(None, &name, t.started + 1), Some(t.clone()));
        clear(None, &name);
        assert_eq!(get(None, &name, t.started + 1), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_driver_attaches_and_learns_whether_a_trip_waits() {
        let dir = scratch("attach");
        let mut t = sample();
        t.name = "Attached Pilot".into();
        t.started = unix_now();
        assert!(!attach(&t.name, Some(&dir)));
        put(Some(&dir), &t);
        assert!(resumable(&t.name));
        TRIPS.lock().unwrap().remove(&t.name);
        assert!(attach(&t.name, Some(&dir)), "from the file");
        abandon(&t.name);
        assert!(!resumable(&t.name));
        assert_eq!(attached(&t.name), Some(dir.clone()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_scan_row_reads_its_three_ids() {
        let row = eve_server::pyutil::keyval(vec![
            ("id", PyValue::Int(1)),
            ("typeID", PyValue::Int(2)),
            ("groupID", PyValue::Int(3)),
        ]);
        assert_eq!(
            scan_row(&row),
            Some(ScanRow {
                id: 1,
                type_id: 2,
                group_id: 3
            })
        );
        assert_eq!(scan_row(&PyValue::Int(1)), None);
    }
}
