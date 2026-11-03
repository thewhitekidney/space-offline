//! **Rare ore from the deep belts** (PROG-NULL): miners go where the
//! minerals a high-sec belt never yields are cut, when the market pays for
//! them over the danger, and carry them home.
//!
//! **Why.** A barge (and a Strip Miner I) takes Megacyte; Zydrine and
//! Morphite feed the next rungs. The bundle's belts yield Megacyte only in
//! null-sec (461 systems), Zydrine only there (741), Morphite only there
//! (368); no high-sec or low-sec belt yields any of the three. A closed
//! world with only home-belt miners never has them, so the builders'
//! sourcing ([`crate::builder::Sourcing`], [`crate::supply::Supply`])
//! rightly holds the rung back. Players solve it by living out there; so
//! does a bot, here.
//!
//! - **What is rare** ([`rare_minerals`]): a mineral that some belt on the
//!   map yields and the home system's belts do not -- no type id is named.
//! - **Where to go** ([`choose`], pure): for each station whose system's
//!   belts yield a rare mineral and that a route reaches
//!   ([`crate::supply_run::FLOORS`], safest first), the value an hour of
//!   mining there pays ([`value_per_hour`], at the prices the market shows)
//!   less the chance of losing the hull, against the home belt's. It goes
//!   when the margin is [`MARGIN`] better.
//! - **The deployment** ([`turn`], docked, one act, [`crate::brain::Act::Rare`]):
//!   *out* -- fly the route, dock at the far station, tell the
//!   [`RareBoard`] ("a miner works there; the minerals are obtainable");
//!   the ordinary miner rules then undock and mine the belts there and sell
//!   what the local bids take; *home* -- once rare ore or mineral stock sits
//!   in the far hangar, load it, fly home and unload it; the home sale
//!   (`brain::docked`) refines and sells it into the builders' bids.
//! - **The builders' side** ([`RareBoard`]): registered in the content
//!   tables beside [`crate::supply::Supply`]; `builder::Local::missing`
//!   counts a mineral a reporting miner works as obtainable, so a rung
//!   needing it is offered only while such a miner exists. Nothing is
//!   conjured: with no miner out there, Megacyte stays unobtainable.
//!
//! Compression of the haul (the PROG-MINE foreman) works unchanged where a
//! foreman is in the fleet; the deployment does not depend on it.
//!
//! Every rule is **[A] policy**; see the constants.

use crate::brain::{Act, Memory, Observation};
use crate::supply::Supply;
use crate::supply_run::{self, Hazards};
use crate::travel::{Map, Trips};
use crate::verbs::Book;
use crate::{Conn, Error, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One miner in `seed.is_multiple_of(RARE_SHARE_MOD)` considers the deployment
/// **[A]**: not every miner leaves safety. Falsified by a closed soak where
/// too few (no Megacyte ever) or too many (empty home belts) go.
pub const RARE_SHARE_MOD: u64 = 4;

/// How often a docked miner reconsiders **[A]**.
pub const RECHECK: Duration = Duration::from_secs(900);

/// Mineral units an m3 of ore yields where the table has no rock sizes
/// (a table built by [`Supply::from_rocks`]), spread over the minerals
/// equally **[A]**: about Veldspar's 40. Falsified by a bundle belt whose
/// real units per m3 differ by an order of magnitude.
pub const FALLBACK_UNITS_PER_M3: f64 = 40.0;

/// Hours one deployment is judged over (mine, haul, return) **[A]**.
pub const CYCLE_H: f64 = 8.0;

/// A belt must beat home's by this share **[A]**.
pub const MARGIN: f64 = 0.25;

/// What a loss costs besides the cargo: the hull flown, ISK **[A]**.
pub const EXPOSURE: f64 = 10_000_000.0;

/// The chance of losing the hull in one hour at a belt of each band
/// **[A]**, by the same bands as a jump.
pub const STAY_DEATH_HIGH: f64 = 0.0002;
pub const STAY_DEATH_LOW: f64 = 0.004;
pub const STAY_DEATH_NULL: f64 = 0.012;

/// Rare units in the far hangar that call a haul home **[A]**.
pub const HAUL_MIN_UNITS: i64 = 500;

/// A wallet below this keeps a miner home **[A]**: a hull lost out there
/// must be replaceable.
pub const MIN_WALLET: f64 = 2_000_000.0;

// ---- the board ---------------------------------------------------------------------

/// `miner name -> (system, rare minerals cut there)`.
type Reports = BTreeMap<String, (i64, Vec<i64>)>;

/// **Who mines what, out there**: shared by every bot through the content
/// tables. A report is a miner's name, the system and the rare minerals its
/// belts yield.
#[derive(Debug, Clone, Default)]
pub struct RareBoard {
    miners: Arc<Mutex<Reports>>,
}

impl RareBoard {
    pub fn new() -> RareBoard {
        RareBoard::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Reports> {
        self.miners.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `name` mines in `system`, where `minerals` are cut.
    pub fn report(&self, name: &str, system: i64, minerals: Vec<i64>) {
        self.lock().insert(name.to_owned(), (system, minerals));
    }

    /// `name` is back home, or lost.
    pub fn withdraw(&self, name: &str) {
        self.lock().remove(name);
    }

    /// Every mineral some reporting miner cuts.
    pub fn offered(&self) -> BTreeSet<i64> {
        self.lock()
            .values()
            .flat_map(|(_, m)| m.iter().copied())
            .collect()
    }

    /// The miners out there.
    pub fn count(&self) -> usize {
        self.lock().len()
    }
}

// ---- the choice, pure --------------------------------------------------------------

/// The minerals some belt on the map yields and `home`'s belts do not.
pub fn rare_minerals(supply: &Supply, map: &Map, home: i64) -> BTreeSet<i64> {
    let mut all = BTreeSet::new();
    for s in map.systems() {
        all.extend(supply.minable_in(s));
    }
    all.into_iter()
        .filter(|&m| !supply.supplies(home, m))
        .collect()
}

/// The chance of losing the hull in an hour at a belt of `security`.
pub fn stay_death(security: Option<f64>) -> f64 {
    let jump = supply_run::jump_death(security);
    if jump == supply_run::DEATH_HIGH {
        STAY_DEATH_HIGH
    } else if jump == supply_run::DEATH_LOW {
        STAY_DEATH_LOW
    } else {
        STAY_DEATH_NULL
    }
}

/// What an hour of mining `system`'s belts pays, ISK, at `price_of` (a
/// mineral the market has no price for counts for nothing): the minerals
/// an m3 of its rock yields ([`Supply::units_per_m3`]) times their prices,
/// times `m3_h`, what the hull flown mines an hour
/// ([`crate::mine_ladder::effective_m3_h`]). A system the table has no rock
/// sizes for counts [`FALLBACK_UNITS_PER_M3`] spread over its minerals.
pub fn value_per_hour(
    supply: &Supply,
    system: i64,
    price_of: &dyn Fn(i64) -> Option<f64>,
    m3_h: f64,
) -> f64 {
    let price = |m: i64| price_of(m).unwrap_or(0.0);
    let per_m3: f64 = if supply.sized(system) {
        supply
            .minable_in(system)
            .map(|m| supply.units_per_m3(system, m).unwrap_or(0.0) * price(m))
            .sum()
    } else {
        let minerals: Vec<i64> = supply.minable_in(system).collect();
        if minerals.is_empty() {
            return 0.0;
        }
        minerals.iter().map(|&m| price(m)).sum::<f64>() / minerals.len() as f64
            * FALLBACK_UNITS_PER_M3
    };
    per_m3 * m3_h
}

/// What the hull flown mines an hour, m3, trips included: the ladder row for
/// `hull` (the Merlin's when it is none or off the ladder **[A]**: the
/// starter, the conservative rate), two lasers' worth, no burst, no
/// compression.
pub fn m3_per_hour(hull: Option<i64>) -> f64 {
    use crate::mine_ladder::{MERLIN, effective_m3_h, hull_of};
    hull.and_then(hull_of)
        .or_else(|| hull_of(MERLIN))
        .map_or(0.0, |h| effective_m3_h(h, h.lasers, 0.0, false))
}

/// A belt worth the trip.
#[derive(Debug, Clone, PartialEq)]
pub struct Belt {
    pub system: i64,
    pub station: i64,
    pub floor: f64,
    /// Round-trip chance of losing the hull.
    pub risk: f64,
    /// The rare minerals its rocks yield.
    pub rare: Vec<i64>,
    /// Expected ISK over [`CYCLE_H`], less the risk.
    pub expected: f64,
}

/// What [`choose`] reads.
pub struct World<'a> {
    pub map: &'a Map,
    pub supply: &'a Supply,
    pub hazards: &'a Hazards,
    pub price_of: &'a dyn Fn(i64) -> Option<f64>,
    /// What the flown hull mines an hour, m3 ([`m3_per_hour`]).
    pub m3_h: f64,
}

/// **The belt to deploy to** from `here`, or `None` to stay: the best
/// expected ISK over a cycle among systems with a station that yield a rare
/// mineral, beating home's by [`MARGIN`]. Pure.
pub fn choose(w: &World<'_>, here: i64) -> Option<Belt> {
    let rare = rare_minerals(w.supply, w.map, here);
    if rare.is_empty() {
        return None;
    }
    let home_ev = value_per_hour(w.supply, here, w.price_of, w.m3_h)
        * CYCLE_H
        * (1.0 - stay_death(w.map.security(here)) * CYCLE_H);
    let mut best: Option<Belt> = None;
    for system in w.map.systems() {
        if system == here {
            continue;
        }
        let Some(station) = w.map.stations_in(system).into_iter().min() else {
            continue;
        };
        let minerals: Vec<i64> = w.supply.minable_in(system).collect();
        let yields: Vec<i64> = minerals
            .iter()
            .copied()
            .filter(|m| rare.contains(m))
            .collect();
        if yields.is_empty() {
            continue;
        }
        let Some((floor, route)) = supply_run::FLOORS.iter().find_map(|&f| {
            w.map
                .route(here, system, f)
                .filter(|r| r.len() - 1 <= supply_run::RUN_MAX_JUMPS as usize)
                .map(|r| (f, r))
        }) else {
            continue;
        };
        let out = supply_run::route_risk(w.map, &route, w.hazards);
        let back: Vec<i64> = route.iter().rev().copied().collect();
        let home_leg = supply_run::route_risk(w.map, &back, w.hazards);
        let stay = (stay_death(w.map.security(system)) + w.hazards.extra(system)).min(1.0);
        let stay_risk = 1.0 - (1.0 - stay).powf(CYCLE_H);
        let risk = 1.0 - (1.0 - out) * (1.0 - home_leg) * (1.0 - stay_risk);
        if risk > supply_run::MAX_RISK {
            continue;
        }
        let gross = value_per_hour(w.supply, system, w.price_of, w.m3_h) * CYCLE_H;
        let expected = gross * (1.0 - risk) - risk * EXPOSURE;
        if expected <= home_ev * (1.0 + MARGIN) {
            continue;
        }
        let better = best.as_ref().is_none_or(|b| {
            expected > b.expected || (expected == b.expected && station < b.station)
        });
        if better {
            best = Some(Belt {
                system,
                station,
                floor,
                risk,
                rare: yields,
                expected,
            });
        }
    }
    best
}

// ---- the leg, pure -----------------------------------------------------------------

/// What a docked miner knows of its deployment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// The station it deployed from: `Some` while it works away.
    pub home: Option<i64>,
    /// When it last looked.
    pub checked: Option<std::time::Instant>,
}

/// What to do this turn.
#[derive(Debug, Clone, PartialEq)]
pub enum Leg {
    /// Nothing: mine where it is.
    Stay,
    /// Fly out to this belt.
    Out(Belt),
    /// Carry the stock home to this station.
    Home(i64),
}

/// **The decision** (pure): away with enough rare stock -> home; at home
/// with a belt worth the trip -> out; else stay.
pub fn decide(state: &State, at: i64, stock: i64, belt: Option<Belt>) -> Leg {
    match state.home {
        Some(home) if home != at => {
            if stock >= HAUL_MIN_UNITS {
                Leg::Home(home)
            } else {
                Leg::Stay
            }
        }
        _ => belt.map_or(Leg::Stay, Leg::Out),
    }
}

/// Whether a bot with this seed considers the deployment at all.
pub fn eligible(seed: u64) -> bool {
    seed.is_multiple_of(RARE_SHARE_MOD)
}

/// The miner's docked hook (`brain::docked`): every [`RECHECK`] an eligible
/// miner spends a turn on [`turn`]. `None`: carry on mining.
pub fn due(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    if !eligible(m.seed) || m.policy.travel.is_none() {
        return None;
    }
    if m.rare.checked.is_some_and(|t| o.now < t + RECHECK) {
        return None;
    }
    m.rare.checked = Some(o.now);
    Some(vec![Act::Rare])
}

// ---- the turn ----------------------------------------------------------------------

/// The mineral's price: the best standing bid, else the cheapest ask.
fn book_price(book: &Book) -> Option<f64> {
    let bid = book
        .bids
        .iter()
        .chain(book.asks.iter().filter(|a| a.bid))
        .map(|b| b.price)
        .fold(None, |a: Option<f64>, p| Some(a.map_or(p, |a| a.max(p))));
    bid.or_else(|| {
        book.asks
            .iter()
            .filter(|a| !a.bid)
            .map(|a| a.price)
            .fold(None, |a: Option<f64>, p| Some(a.map_or(p, |a| a.min(p))))
    })
}

/// Units of rare minerals (or ore that refines into them) in the hangar.
fn stock_of(
    c: &mut Conn,
    content: &eve_server::content::Content,
    rare: &BTreeSet<i64>,
) -> Result<(i64, Vec<i64>)> {
    let refine = content.try_get::<eve_server::reprocess::Table>();
    let mut units = 0;
    let mut items = Vec::new();
    for row in c.hangar_items()? {
        let yields_rare = rare.contains(&row.type_id)
            || (row.category_id == crate::brain::CATEGORY_ASTEROID
                && refine.is_some_and(|r| {
                    r.materials(row.type_id)
                        .iter()
                        .any(|(m, _)| rare.contains(m))
                }));
        if yields_rare && !row.is_fitted() {
            units += row.quantity;
            items.push(row.item_id);
        }
    }
    Ok((units, items))
}

/// What [`turn`] reads from the world.
pub struct Env<'a> {
    pub content: &'a eve_server::content::Content,
    pub map: &'a Map,
    pub hazards: &'a Hazards,
    /// The ship type flown, if known.
    pub hull: Option<i64>,
}

/// **One deployment turn**, docked. Returns the act line's note.
pub fn turn(c: &mut Conn, env: &Env<'_>, name: &str, state: &mut State) -> Result<String> {
    let at = c
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("rare ore: not docked".to_owned()))?;
    let here = c
        .session()
        .solar_system_id2()
        .ok_or_else(|| Error::NotSent("rare ore: no system".to_owned()))?;
    let Some(supply) = env.content.try_get::<Supply>() else {
        return Ok("no ore supply table: stays".to_owned());
    };
    let board = env.content.try_get::<RareBoard>();
    // The home system decides what is rare; away, the home station's.
    let home_system = state
        .home
        .and_then(|h| env.map.system_of(h))
        .unwrap_or(here);
    let rare = rare_minerals(supply, env.map, home_system);
    let (stock, items) = if state.home.is_some_and(|h| h != at) {
        stock_of(c, env.content, &rare)?
    } else {
        (0, Vec::new())
    };
    let belt = if state.home.is_none() && c.cash_balance()? >= MIN_WALLET {
        let mut prices = BTreeMap::new();
        for m in &rare {
            if let Ok(b) = c.market_orders(*m) {
                prices.insert(*m, book_price(&b));
            }
        }
        for m in supply.minable_in(here) {
            if let std::collections::btree_map::Entry::Vacant(e) = prices.entry(m)
                && let Ok(b) = c.market_orders(m)
            {
                e.insert(book_price(&b));
            }
        }
        let price_of = |m: i64| prices.get(&m).copied().flatten();
        choose(
            &World {
                map: env.map,
                supply,
                hazards: env.hazards,
                price_of: &price_of,
                m3_h: m3_per_hour(env.hull),
            },
            here,
        )
    } else {
        None
    };
    match decide(state, at, stock, belt) {
        Leg::Stay => {
            if state.home.is_none()
                && let Some(board) = board
            {
                board.withdraw(name);
            }
            Ok(format!("stays at {at}: {stock} rare units in the hangar"))
        }
        Leg::Out(b) => {
            crate::travel::go(
                c,
                Trips {
                    map: env.map,
                    floor: b.floor,
                },
                b.station,
            )?;
            state.home = Some(at);
            if let Some(board) = board {
                board.report(name, b.system, b.rare.clone());
            }
            Ok(format!(
                "deployed to {} (system {}, risk {:.3}, rare {:?})",
                b.station, b.system, b.risk, b.rare
            ))
        }
        Leg::Home(home) => {
            let mut notes = Vec::new();
            for item in &items {
                if let Err(e) = c.load_cargo(*item, None) {
                    notes.push(format!("load: {e}"));
                }
            }
            let floor = supply_run::FLOORS
                .iter()
                .copied()
                .find(|&f| {
                    env.map
                        .system_of(home)
                        .is_some_and(|s| env.map.route(here, s, f).is_some())
                })
                .unwrap_or(crate::travel::ANY_SECURITY);
            crate::travel::go(
                c,
                Trips {
                    map: env.map,
                    floor,
                },
                home,
            )?;
            let mut home_units = 0;
            for row in c.cargo()? {
                if c.unload(row.item_id, None).is_ok() {
                    home_units += row.quantity;
                }
            }
            // Home again: the next check may send it out once more, so the
            // board keeps its report until it decides to stay.
            state.home = None;
            Ok(format!(
                "carried {home_units} units home to {home}{}",
                if notes.is_empty() {
                    String::new()
                } else {
                    format!("; {}", notes.join("; "))
                }
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supply::OreFacts;

    // Invented ids only.
    const HOME: i64 = 32_000_001;
    const LOW: i64 = 32_000_002;
    const DEEP: i64 = 32_000_003;
    const DEEP2: i64 = 32_000_004;
    const ORE_H: i64 = 33_000_001;
    const ORE_R: i64 = 33_000_002;
    const MIN_COMMON: i64 = 34_000_001;
    const MIN_RARE: i64 = 34_000_002;
    const ST_H: i64 = 62_000_001;
    const ST_D: i64 = 62_000_003;
    const ST_D2: i64 = 62_000_004;

    fn yields(ore: i64) -> Vec<i64> {
        match ore {
            ORE_H => vec![MIN_COMMON],
            ORE_R => vec![MIN_COMMON, MIN_RARE],
            _ => vec![],
        }
    }

    fn supply() -> Supply {
        Supply::from_rocks([(HOME, ORE_H), (DEEP, ORE_R), (DEEP2, ORE_R)], yields)
    }

    fn map() -> Map {
        let l = |m: Map, a: i64, b: i64| {
            m.with_gate(a, a * 10 + b, b, b * 10 + a)
                .with_gate(b, b * 10 + a, a, a * 10 + b)
        };
        let m = l(l(l(Map::new(), HOME, LOW), LOW, DEEP), DEEP, DEEP2);
        m.with_security(HOME, 0.9)
            .with_security(LOW, 0.3)
            .with_security(DEEP, -0.4)
            .with_security(DEEP2, -0.5)
            .with_station(ST_H, HOME)
            .with_station(ST_D, DEEP)
            .with_station(ST_D2, DEEP2)
    }

    const M3_H: f64 = 1_000.0;

    fn prices(rare: f64) -> impl Fn(i64) -> Option<f64> {
        move |m| match m {
            MIN_COMMON => Some(10.0),
            MIN_RARE => Some(rare),
            _ => None,
        }
    }

    #[test]
    fn rare_is_what_a_belt_yields_and_home_does_not() {
        let r = rare_minerals(&supply(), &map(), HOME);
        assert_eq!(r.into_iter().collect::<Vec<_>>(), vec![MIN_RARE]);
        assert!(rare_minerals(&supply(), &map(), DEEP).is_empty());
    }

    #[test]
    fn the_belt_must_pay_over_the_danger() {
        let (s, m, h) = (supply(), map(), Hazards::none());
        let cheap = prices(11.0);
        let w = World {
            map: &m,
            supply: &s,
            hazards: &h,
            price_of: &cheap,
            m3_h: M3_H,
        };
        assert!(
            choose(&w, HOME).is_none(),
            "an unpaid rare mineral stays home"
        );
        let dear = prices(50_000.0);
        let w = World {
            price_of: &dear,
            ..w
        };
        let b = choose(&w, HOME).expect("a rich belt");
        assert_eq!(b.station, ST_D, "the nearer of two equal belts");
        assert_eq!(b.rare, vec![MIN_RARE]);
        assert_eq!(b.floor, crate::travel::ANY_SECURITY);
        assert!(b.risk > 0.0 && b.risk < supply_run::MAX_RISK);
    }

    #[test]
    fn a_deep_belt_mostly_common_ore_does_not_beat_home() {
        // Home: 20 big common rocks. Deep: the same 20 and one small rock of
        // the rare ore. Priced high, the rare mineral is a sliver of an m3.
        let facts = |ore: i64| match ore {
            ORE_H => OreFacts {
                minerals: vec![(MIN_COMMON, 4.0)],
                volume: 0.1,
                rock_units: 1_000.0,
            },
            ORE_R => OreFacts {
                minerals: vec![(MIN_COMMON, 3.0), (MIN_RARE, 0.5)],
                volume: 0.1,
                rock_units: 100.0,
            },
            _ => OreFacts::default(),
        };
        let s = Supply::from_sized(
            [(HOME, ORE_H, 20), (DEEP, ORE_H, 20), (DEEP, ORE_R, 1)],
            facts,
        );
        let (m, h) = (map(), Hazards::none());
        let dear = prices(1_000.0);
        let w = World {
            map: &m,
            supply: &s,
            hazards: &h,
            price_of: &dear,
            m3_h: M3_H,
        };
        assert!(
            choose(&w, HOME).is_none(),
            "one small rare rock does not pay the trip"
        );
        // The same belt by the old mean price would have: 505 against 10.
        assert!(
            value_per_hour(&s, DEEP, &dear, M3_H) < 1.25 * value_per_hour(&s, HOME, &dear, M3_H)
        );
    }

    #[test]
    fn a_hunter_in_the_way_turns_the_miner_back() {
        let (s, m) = (supply(), map());
        let dear = prices(50_000.0);
        let h = Hazards([(DEEP, 0.9), (DEEP2, 0.9)].into());
        let w = World {
            map: &m,
            supply: &s,
            hazards: &h,
            price_of: &dear,
            m3_h: M3_H,
        };
        assert!(choose(&w, HOME).is_none());
    }

    #[test]
    fn the_leg_goes_out_from_home_and_carries_the_stock_back() {
        let b = Belt {
            system: DEEP,
            station: ST_D,
            floor: 0.0,
            risk: 0.1,
            rare: vec![MIN_RARE],
            expected: 1.0,
        };
        let home = State::default();
        assert_eq!(decide(&home, ST_H, 0, Some(b.clone())), Leg::Out(b.clone()));
        assert_eq!(decide(&home, ST_H, 0, None), Leg::Stay);
        let away = State {
            home: Some(ST_H),
            checked: None,
        };
        assert_eq!(decide(&away, ST_D, HAUL_MIN_UNITS - 1, Some(b)), Leg::Stay);
        assert_eq!(decide(&away, ST_D, HAUL_MIN_UNITS, None), Leg::Home(ST_H));
    }

    #[test]
    fn the_board_offers_what_miners_report_and_forgets_those_who_leave() {
        let b = RareBoard::new();
        assert!(b.offered().is_empty());
        b.report("Mina", DEEP, vec![MIN_RARE]);
        b.report("Minb", DEEP2, vec![MIN_RARE, MIN_COMMON]);
        assert_eq!(b.count(), 2);
        assert!(b.offered().contains(&MIN_RARE));
        b.withdraw("Mina");
        b.withdraw("Minb");
        assert!(b.offered().is_empty());
    }

    #[test]
    fn only_a_share_of_the_miners_considers_it() {
        assert!(eligible(RARE_SHARE_MOD * 7));
        assert!(!eligible(RARE_SHARE_MOD * 7 + 1));
    }
}
