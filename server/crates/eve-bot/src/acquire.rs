//! **Acquire, not GM** (R-ECON §2.4 and §3 row E8): how a bot in the closed
//! economy (`eve-bots --closed`, [`crate::brain::Policy::closed`]) gets what
//! GM setup used to give it, the way a player gets it.
//!
//! One [`run`], docked, when the brain says the hangar lacks something
//! ([`crate::brain::Act::Acquire`], at most once per [`ACQUIRE_EVERY`]):
//!
//! 1. **The role's skill** ([`Kit::skills`]): a skill the pilot does not have
//!    is bought as a book from an ask at this station (under O5 the NPC
//!    sells skillbooks at school stations only, E5), injected
//!    (`dogmaIM.InjectSkillIntoBrain`), and queued up to its level on the
//!    skill handler (`skillMgr2.GetMySkillHandler()`, then `SaveNewQueue`,
//!    the queue panel's own commit). A skill already known below its level is
//!    queued only.
//! 2. **The hull** ([`Kit::hull`]), when lacking: the cheapest asks at this
//!    station, each at most the consumer's cap, [`RESERVATION`] x the
//!    type's `P_mkt` inside the bot price clamp ([`crate::pricing`], E10; no
//!    cap where the server quotes no price), inside the wallet.
//! 3. **The rookie ship**: a pilot in a capsule whose hull could not be
//!    bought boards its corvette from the hangar, else asks
//!    `dogmaIM.CreateNewbieShip(shipID, stationID)` (E7) for one. Free, and
//!    no GM.
//! 4. **Modules and rounds** ([`Kit::modules`], [`Kit::ammo`]), when lacking,
//!    as the hull.
//!
//! Whatever is not there is one `MISSING` line each ([`Acquired::missing`]),
//! which the driver logs; the bot keeps what it has (a rookie ship) and plays
//! on. Only a broken connection is an error: a refused buy or injection is a
//! missing line.
//!
//! Every rule here is **[A]** policy except the call shapes, which the verbs
//! cite.

use crate::brain::{self, RoleRef};
use crate::pricing::Prices;
use crate::travel::{Haul, Trips};
use crate::verbs::{Book, BookOrder};
use crate::{Conn, Error, ObjectRef, Result};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::Duration;

/// How often a bot in the closed economy tries again for what it lacks
/// **[A]**: asks come and go, and a player checks the market now and then.
/// Falsified by bots that sit in rookie ships for hours while a hull ask
/// rests at their station (too long), or by `[turns]` showing the market
/// reads of idle bots crowding human turns (too short).
pub const ACQUIRE_EVERY: Duration = Duration::from_secs(30 * 60);
/// The least wait before a repeatedly `MISSING` module or round is bought
/// again ([`crate::brain::acquire_if_due`]) **[A]**; falsified by a market
/// that has none and a log full of empty acquires.
pub const MISSING_RETRY: Duration = Duration::from_secs(2 * 60);

/// The most a bot pays for a ship, module or round, as a multiple of the
/// type's `P_mkt`: [`crate::pricing::RESERVATION`] (E10 owns the rule).
pub const RESERVATION: f64 = crate::pricing::RESERVATION;

/// `expectedBrokersFee` the bot sends. The server computes its own fee
/// (E5) and an immediate buy rests nothing, so this is never charged.
const BROKER_FEE: f64 = crate::builder::BROKER_FEE;

/// `categoryShip` **[V]**.
pub const CATEGORY_SHIP: i64 = 6;

/// What a role flies: the hull, the module (type and count), the rounds
/// (type and count) and the skills `(typeID, level)` it needs. The same
/// things each role's GM setup gives (`roles.rs`), bought instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Kit {
    pub hull: Option<i64>,
    pub modules: Option<(i64, i64)>,
    pub ammo: Option<(i64, i64)>,
    pub skills: &'static [(i64, i64)],
    /// Further modules and charges `(typeID, count)` bought whenever the
    /// module or the rounds are lacking, each up to its count less what
    /// the pilot already holds (hangar, hold and fitted): the huffer's probe
    /// launcher and probes (SOC-B1), beside its scoop.
    pub more: &'static [(i64, i64)],
}

/// **An order** (DOC-2): what a pilot buys for one fit, as owned data, every
/// part once. [`Kit`] is the static row a role carries; an `Order` is built at
/// run time (from a [`Kit`] with [`Order::from_kit`], or from a doctrine's
/// [`crate::doctrine::RoleFit`] with [`Order::from_fit`]) and bought by
/// [`run_order`] or [`buy_order`].
///
/// A type is listed **once**, whatever put it there (a module and a charge
/// that share a type, a fit and a kit's `more`): [`Order::want`] keeps the
/// larger count. `modules` are bought when [`Lacking::module`] and `charges`
/// when [`Lacking::ammo`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Order {
    pub hull: Option<i64>,
    /// `(type, count)` modules (guns, tank, tackle, drones and the like).
    pub modules: Vec<(i64, i64)>,
    /// `(type, units)` rounds.
    pub charges: Vec<(i64, i64)>,
    /// `(skill, level)`.
    pub skills: Vec<(i64, i64)>,
}

impl Order {
    /// An order for a hull alone.
    pub fn hull(type_id: i64) -> Order {
        Order {
            hull: Some(type_id),
            ..Order::default()
        }
    }

    fn listed(&self, type_id: i64) -> bool {
        self.hull == Some(type_id)
            || self
                .modules
                .iter()
                .chain(&self.charges)
                .any(|p| p.0 == type_id)
    }

    fn put(list: &mut Vec<(i64, i64)>, type_id: i64, n: i64) {
        match list.iter_mut().find(|p| p.0 == type_id) {
            Some(p) => p.1 = p.1.max(n),
            None => list.push((type_id, n)),
        }
    }

    /// Add `n` of a module, once per type (the larger count stands). A type
    /// already ordered as a charge stays a charge.
    pub fn want_module(&mut self, type_id: i64, n: i64) -> &mut Order {
        if n > 0 {
            if self.charges.iter().any(|p| p.0 == type_id) {
                Order::put(&mut self.charges, type_id, n);
            } else {
                Order::put(&mut self.modules, type_id, n);
            }
        }
        self
    }

    /// Add `n` rounds, once per type.
    pub fn want_charge(&mut self, type_id: i64, n: i64) -> &mut Order {
        if n > 0 {
            if self.modules.iter().any(|p| p.0 == type_id) {
                Order::put(&mut self.modules, type_id, n);
            } else {
                Order::put(&mut self.charges, type_id, n);
            }
        }
        self
    }

    /// Add a skill, once, at the higher level.
    pub fn want_skill(&mut self, skill: i64, level: i64) -> &mut Order {
        Order::put(&mut self.skills, skill, level);
        self
    }

    /// A [`Kit`] as an order: its module, its `more`, its rounds and its
    /// skills. The miner's laser and hull ladders stay [`run_with_rounds`]'s.
    pub fn from_kit(kit: &Kit) -> Order {
        let mut o = Order {
            hull: kit.hull,
            ..Order::default()
        };
        if let Some((t, n)) = kit.modules {
            o.want_module(t, n);
        }
        for &(t, n) in kit.more {
            o.want_module(t, n);
        }
        if let Some((t, n)) = kit.ammo {
            o.want_charge(t, n);
        }
        for &(s, l) in kit.skills {
            o.want_skill(s, l);
        }
        o
    }

    /// A doctrine's role fit as an order: its hull, every module at its
    /// count, every round (the primary and the alternate), every skill.
    pub fn from_fit(fit: &crate::doctrine::RoleFit) -> Order {
        let mut o = Order::hull(fit.hull);
        for &(t, n) in &fit.modules {
            o.want_module(t, i64::from(n));
        }
        for &(t, n) in &fit.charges {
            o.want_charge(t, i64::from(n));
        }
        for &(s, l) in &fit.skills {
            o.want_skill(s, l);
        }
        o
    }

    /// Every part (module and round) still short, `(type, units)`, given
    /// how many of a type the pilot `holds`. The hull is not a part here.
    pub fn short(&self, held: &dyn Fn(i64) -> i64) -> Vec<(i64, i64)> {
        self.modules
            .iter()
            .chain(&self.charges)
            .filter_map(|&(t, n)| {
                let short = n - held(t);
                (short > 0).then_some((t, short))
            })
            .collect()
    }

    /// Whether the order names `type_id` (the hull included).
    pub fn names(&self, type_id: i64) -> bool {
        self.listed(type_id)
    }
}

/// The miner's: a Merlin (or, best, a Venture: [`hulls_of`], GAPS-B), a
/// Miner I and Mining -- and (SOC-B10) one
/// Mining Drone I with Drones I, for a hull with a drone bay (a rookie
/// ship's takes one; the Merlin has none, `crate::ore`'s header). **[A]**
/// one drone: falsified by miners in a bigger bay flying with it half
/// empty.
pub const MINER_KIT: Kit = Kit {
    hull: Some(brain::MERLIN),
    modules: Some((brain::MINER_I, 1)),
    ammo: None,
    skills: &[
        (crate::roles::SKILL_MINING, 1),
        (crate::ore::SKILL_DRONES, 1),
    ],
    more: &[(crate::ore::MINING_DRONE_I, 1)],
};

/// The escort's (and the site runner's and the hostile's): a Rifter, its
/// guns, a setup load of rounds and the gun's skill.
pub const FIGHTER_KIT: Kit = Kit {
    hull: Some(crate::escort::RIFTER),
    modules: Some((crate::escort::GUN, crate::escort::WANT_GUNS as i64)),
    ammo: Some((crate::escort::AMMO, crate::escort::SETUP_ROUNDS)),
    skills: &[(crate::escort::SKILL_GUNNERY, 1)],
    more: &[],
};

/// The courier's: a hull with a hold, nothing else (`courier.rs` setup).
pub const HAULER_KIT: Kit = Kit {
    hull: Some(brain::MERLIN),
    modules: None,
    ammo: None,
    skills: &[],
    more: &[],
};

/// A role's [`Kit`]: by its row, the miner's, the courier's, the builder's
/// (nothing: it stays docked), and the fighters' for every other row.
pub fn kit_of(role: RoleRef) -> Kit {
    if std::ptr::eq(role, &crate::roles::MINE) {
        MINER_KIT
    } else if std::ptr::eq(role, &crate::courier::ROLE) || std::ptr::eq(role, &crate::trader::ROLE)
    {
        HAULER_KIT
    } else if std::ptr::eq(role, &crate::builder::ROLE)
        || std::ptr::eq(role, &crate::reactor::ROLE)
        || std::ptr::eq(role, &crate::capital::ROLE)
        || std::ptr::eq(role, &crate::capital::ALT_ROLE)
        || std::ptr::eq(role, &crate::hauler::ROLE)
    {
        // CAP-L: nothing from nowhere -- a capital, its alt and a freighter
        // are owned or bought by the role itself, never a standard kit.
        Kit::default()
    } else if std::ptr::eq(role, &crate::foreman::ROLE) {
        crate::foreman::KIT
    } else if std::ptr::eq(role, &crate::huffer::ROLE) {
        crate::huffer::HUFFER_KIT
    } else if std::ptr::eq(role, &crate::probe::ROLE) {
        crate::probe::KIT
    } else if std::ptr::eq(role, &crate::ice::ROLE) {
        crate::ice::ICE_KIT
    } else if std::ptr::eq(role, &crate::pi::ROLE) {
        crate::pi::PI_KIT
    } else if std::ptr::eq(role, &crate::hacker::ROLE) {
        crate::hacker::KIT
    } else if crate::logi::is_logi(role) {
        crate::logi::KIT
    } else if std::ptr::eq(role, &crate::wh_raid::ROLE) {
        crate::wh_raid::RAID_KIT
    } else if std::ptr::eq(role, &crate::roles::ESCORT)
        || std::ptr::eq(role, &crate::wing::ROLE)
        || std::ptr::eq(role, &crate::site_runner::ROLE)
        || std::ptr::eq(role, &crate::missioner::ROLE)
    {
        crate::salvage::RATTER_KIT
    } else {
        FIGHTER_KIT
    }
}

/// What the brain says is lacking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lacking {
    pub ship: bool,
    pub module: bool,
    pub ammo: bool,
    /// The pilot sits in a capsule (or its ship's type is not known yet).
    pub in_capsule: bool,
}

/// How the rookie ship came.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rookie {
    /// `CreateNewbieShip` minted and boarded one.
    Created,
    /// One was in the hangar, and was boarded.
    Boarded(i64),
}

/// What one [`run`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Acquired {
    /// Skills queued (and injected first, when the book was bought).
    pub learned: Vec<i64>,
    /// `(typeID, units, ISK)` per fill.
    pub bought: Vec<(i64, i64, f64)>,
    pub rookie: Option<Rookie>,
    /// One line per thing not found: the driver's `MISSING` lines.
    pub missing: Vec<String>,
    /// One line per trip made for what this station does not sell.
    pub trips: Vec<String>,
    /// The wallet before and after.
    pub before: f64,
    pub after: f64,
}

impl Acquired {
    /// The act line's note.
    pub fn summary(&self) -> String {
        let spent: f64 = self.bought.iter().map(|&(_, _, isk)| isk).sum();
        let rookie = match self.rookie {
            None => String::new(),
            Some(Rookie::Created) => "; rookie ship created".to_owned(),
            Some(Rookie::Boarded(id)) => format!("; rookie ship {id} boarded"),
        };
        let trips: String = self.trips.iter().map(|t| format!("; {t}")).collect();
        format!(
            "learned {:?}, bought {} lots for {spent:.2} ISK{rookie}{trips}; {} missing; wallet {:.2} -> {:.2} ISK",
            self.learned,
            self.bought.len(),
            self.missing.len(),
            self.before,
            self.after
        )
    }
}

/// **The buy plan** (pure): the asks at `station` cheapest first, each at
/// most `cap` per unit (`None`: any price), up to `units` and inside `isk`;
/// `(ask, units)` per fill.
pub fn plan(
    book: &Book,
    station: i64,
    cap: Option<f64>,
    isk: f64,
    units: i64,
) -> Vec<(BookOrder, i64)> {
    let mut asks: Vec<&BookOrder> = book
        .asks
        .iter()
        .filter(|a| !a.bid && a.station_id == station && a.vol_remaining > 0 && a.price > 0.0)
        .filter(|a| {
            // The dev seller's flat ask (`--dev`) is the dev world's own
            // price, not the market's: the closed economy's reservation
            // cap does not apply to it (a 100 ISK round against a 66 ISK
            // cap left every dev roamer unarmed) **[A]**, falsified by a dev
            // bot that spends its billion on a thing it has no use for.
            cap.is_none_or(|c| a.price <= c) || eve_server::econ_exchange::is_dev(a.order_id)
        })
        .collect();
    asks.sort_by(|a, b| a.price.total_cmp(&b.price));
    let (mut left, mut isk) = (units, isk);
    let mut out = Vec::new();
    for a in asks {
        if left <= 0 {
            break;
        }
        let afford = (isk / a.price).floor() as i64;
        let n = left.min(a.vol_remaining).min(afford);
        if n <= 0 {
            break;
        }
        isk -= n as f64 * a.price;
        left -= n;
        out.push((*a, n));
    }
    out
}

/// **One acquisition, docked** (module header). `prices` are the server's
/// `P_ref`s ([`Conn::adjusted_prices`]) and each type's history
/// ([`Prices`]); a failed read is no cap. No trips: what this station does
/// not sell is missing.
pub fn run(conn: &mut Conn, kit: &Kit, lacking: Lacking) -> Result<Acquired> {
    run_with(conn, kit, lacking, None)
}

/// [`run`], and with `trips` (the travel lane) what this station does not
/// sell is fetched from the nearest station within reach that does
/// ([`crate::travel::find`], [`crate::travel::fetch`]): a book or a module
/// in the hold, a hull flown home.
pub fn run_with(
    conn: &mut Conn,
    kit: &Kit,
    lacking: Lacking,
    trips: Option<Trips<'_>>,
) -> Result<Acquired> {
    run_with_rounds(conn, kit, lacking, trips, &Rounds::default())
}

/// **The rounds a buyer will take** (BOT-TUNE): `any` are the types the gun
/// fires ([`crate::ammo::rounds_for`]), `local` the ones local builders make
/// ([`crate::ammo::local_products`]). Empty `any`: the kit's own round only.
#[derive(Debug, Clone, Copy, Default)]
pub struct Rounds<'a> {
    pub any: &'a [i64],
    pub local: &'a [i64],
}

/// The round to buy for a kit that names `default`: the best one with an
/// ask at `station` ([`crate::ammo::choose`]); with none there, the most
/// preferred one the gun takes (a trip fetches it); without candidates,
/// `default`.
fn pick_round(conn: &mut Conn, station: i64, default: i64, rounds: &Rounds<'_>) -> Result<i64> {
    if rounds.any.is_empty() {
        return Ok(default);
    }
    let mut cheapest: BTreeMap<i64, f64> = BTreeMap::new();
    for &t in rounds.any {
        match conn.market_orders(t) {
            Ok(book) => {
                if let Some(p) = book
                    .asks
                    .iter()
                    .filter(|a| !a.bid && a.station_id == station && a.vol_remaining > 0)
                    .map(|a| a.price)
                    .min_by(f64::total_cmp)
                {
                    cheapest.insert(t, p);
                }
            }
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => {}
        }
    }
    if let Some(t) = crate::ammo::choose(rounds.any, rounds.local, &|t| cheapest.get(&t).copied()) {
        return Ok(t);
    }
    Ok(rounds
        .local
        .iter()
        .copied()
        .find(|t| rounds.any.contains(t))
        .unwrap_or(rounds.any[0]))
}

/// [`run_with`], taking any of `rounds` for the kit's ammo.
pub fn run_with_rounds(
    conn: &mut Conn,
    kit: &Kit,
    lacking: Lacking,
    trips: Option<Trips<'_>>,
    rounds: &Rounds<'_>,
) -> Result<Acquired> {
    let (station, mut got, mut prices) = begin(conn)?;
    let mut shop = Shop {
        station,
        prices: &mut prices,
        trips,
    };
    learn(
        conn,
        kit.skills,
        &requirements(&Order::from_kit(kit), lacking),
        &mut shop,
        &mut got,
    )?;

    // The hulls best first (GAPS-B: the miner's Venture, else its Merlin);
    // the first one held or bought is assembled.
    let mut hull = false;
    // PROG-MINE: the laser the hull flown takes (a barge a Strip Miner I).
    let mut laser = kit.modules;
    if *kit == MINER_KIT
        && let Some(t) = active_hull(conn)?
    {
        laser = Some((crate::mine_ladder::laser_of(t), 1));
    }
    if lacking.ship {
        for h in hulls_in(conn, kit, shop.prices)? {
            let held = conn.hangar_items()?.iter().any(|r| r.type_id == h);
            if held || buy(conn, &mut shop, h, 1, Haul::Fly, &mut got)? > 0 {
                hull = assemble(conn, station, h, &mut got)?;
                if *kit == MINER_KIT {
                    laser = Some((crate::mine_ladder::laser_of(h), 1));
                }
                break;
            }
        }
    }
    // A trip for a book may have boarded the rookie ship already.
    // (ship-pool: nor when the chooser's hull already waits in the hangar.)
    if lacking.in_capsule
        && !hull
        && flying_capsule(conn)?
        && !crate::hull_choice::hangar_holds_hull(conn, None)?
    {
        rookie(conn, station, &mut got)?;
    }
    if lacking.module
        && let Some((t, n)) = laser
    {
        buy(conn, &mut shop, t, n, Haul::Cargo, &mut got)?;
    }
    if lacking.ammo
        && let Some((t, n)) = kit.ammo
    {
        let t = pick_round(conn, station, t, rounds)?;
        buy(conn, &mut shop, t, n, Haul::Cargo, &mut got)?;
    }
    if lacking.module || lacking.ammo {
        for &(t, n) in kit.more {
            let short = n - held(conn, t)?;
            if short > 0 {
                buy(conn, &mut shop, t, short, Haul::Cargo, &mut got)?;
            }
        }
    }
    got.after = conn.cash_balance()?;
    Ok(got)
}

/// **One acquisition of an [`Order`], docked** (DOC-2): the skills, the hull
/// (when `lacking.ship`), the rookie ship for a pilot left in a capsule, then
/// each module (`lacking.module`) and round (`lacking.ammo`) up to its count
/// less what the pilot already holds. A round that `rounds.any` lists is
/// bought as the best round of that set at this station ([`pick_round`]).
/// No ladders: a hull is the one the order names.
pub fn run_order(
    conn: &mut Conn,
    order: &Order,
    lacking: Lacking,
    trips: Option<Trips<'_>>,
    rounds: &Rounds<'_>,
) -> Result<Acquired> {
    let (station, mut got, mut prices) = begin(conn)?;
    let mut shop = Shop {
        station,
        prices: &mut prices,
        trips,
    };
    learn(
        conn,
        &order.skills,
        &requirements(order, lacking),
        &mut shop,
        &mut got,
    )?;
    let mut hull = false;
    if lacking.ship
        && let Some(h) = order.hull
    {
        let held = conn.hangar_items()?.iter().any(|r| r.type_id == h);
        if held || buy(conn, &mut shop, h, 1, Haul::Fly, &mut got)? > 0 {
            hull = assemble(conn, station, h, &mut got)?;
        }
    }
    if lacking.in_capsule
        && !hull
        && flying_capsule(conn)?
        && !crate::hull_choice::hangar_holds_hull(conn, order.hull)?
    {
        rookie(conn, station, &mut got)?;
    }
    if lacking.module {
        for &(t, n) in &order.modules {
            let short = n - held(conn, t)?;
            if short > 0 {
                buy(conn, &mut shop, t, short, Haul::Cargo, &mut got)?;
            }
        }
    }
    if lacking.ammo {
        for &(t, n) in &order.charges {
            let short = n - held(conn, t)?;
            if short > 0 {
                let t = if rounds.any.contains(&t) {
                    pick_round(conn, station, t, rounds)?
                } else {
                    t
                };
                buy(conn, &mut shop, t, short, Haul::Cargo, &mut got)?;
            }
        }
    }
    got.after = conn.cash_balance()?;
    Ok(got)
}

/// **Skills, modules and rounds of an [`Order`], bought** without the hull:
/// [`buy_more`] for an order built at run time.
pub fn buy_order(conn: &mut Conn, order: &Order, trips: Option<Trips<'_>>) -> Result<Acquired> {
    let items: Vec<(i64, i64)> = order
        .modules
        .iter()
        .chain(&order.charges)
        .copied()
        .collect();
    buy_more(conn, &order.skills, &items, trips)
}

/// What the pilot holds of each type an order names (hangar, hold, fitted,
/// drone bay): the `held` that [`Order::short`] takes, read once.
pub fn holdings(conn: &mut Conn, order: &Order) -> Result<BTreeMap<i64, i64>> {
    let mut out = BTreeMap::new();
    for &(t, _) in order.modules.iter().chain(&order.charges) {
        out.insert(t, held(conn, t)?);
    }
    Ok(out)
}

/// **Only skills** (GAPS-B): each of `skills` the pilot lacks, its book
/// bought here (and with `trips` within reach), injected and queued up to
/// its level -- [`run_with`]'s first step for a list known only at run
/// time: the builder's invention skills, read from the recipe, whose
/// encryption method a school station sells
/// (`econ_exchange::SCHOOL_INVENTION_BOOKS`).
pub fn learn_skills(
    conn: &mut Conn,
    skills: &[(i64, i64)],
    trips: Option<Trips<'_>>,
) -> Result<Acquired> {
    let (station, mut got, mut prices) = begin(conn)?;
    let mut shop = Shop {
        station,
        prices: &mut prices,
        trips,
    };
    learn(conn, skills, &[], &mut shop, &mut got)?;
    got.after = conn.cash_balance()?;
    Ok(got)
}

/// **Skills a boarding or a fit waits on** (SKILLGATE): [`learn_skills`],
/// but the levels go to the **front** of the queue.
pub fn learn_urgent(
    conn: &mut Conn,
    skills: &[(i64, i64)],
    trips: Option<Trips<'_>>,
) -> Result<Acquired> {
    let (station, mut got, mut prices) = begin(conn)?;
    let mut shop = Shop {
        station,
        prices: &mut prices,
        trips,
    };
    learn(conn, &[], skills, &mut shop, &mut got)?;
    got.after = conn.cash_balance()?;
    Ok(got)
}

/// **The trained levels, for a hull check** (SKILLGATE): `None` when the
/// thread has no content (nothing to check, no call made) or the read was
/// refused (nothing filtered); only a dead connection is the error.
pub fn gate_levels(conn: &mut Conn) -> Result<Option<BTreeMap<i64, i64>>> {
    if CONTENT.with(|c| c.borrow().is_none()) {
        return Ok(None);
    }
    match skill_levels(conn) {
        Ok(l) => Ok(Some(l)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(_) => Ok(None),
    }
}

/// [`flyable`] over [`gate_levels`]: `None` flies everything.
pub fn flyable_in(levels: &Option<BTreeMap<i64, i64>>, type_id: i64) -> bool {
    levels.as_ref().is_none_or(|l| flyable(l, type_id))
}

/// Whether a refusal is the fit's skill gate (SKILLGATE): the key the
/// server raises for a module the pilot cannot use
/// (`FittingHasSkillPrerequisites`, [A] until the gate is seen on a client).
pub fn is_skill_refusal(e: &Error) -> bool {
    matches!(e, Error::Refused(w) if w.contains("FittingHasSkillPrerequisites"))
}

/// Whether a refusal is the online gate (SKILLGATE): the key the server
/// raises when a module's skills are not (any more) trained
/// (`OnlineHasSkillPrerequisites`, [A] until seen on a client).
pub fn is_online_skill_refusal(e: &Error) -> bool {
    matches!(e, Error::Refused(w) if w.contains("OnlineHasSkillPrerequisites"))
}

/// **`set_online`, soft on the skill gate** (SKILLGATE): a refusal for the
/// pilot's skills (a module the server put offline because the skill is not
/// trained) queues what the module asks, ahead of the rest, and is answered
/// `Ok(Err(note))`; every other error passes.
pub fn set_online_gated(conn: &mut Conn, module: i64) -> Result<std::result::Result<(), String>> {
    match conn.set_online(module) {
        Ok(()) => Ok(Ok(())),
        Err(e) if is_online_skill_refusal(&e) => {
            let ty = conn
                .fitted()?
                .iter()
                .find(|r| r.item_id == module)
                .map(|r| r.type_id);
            let lack = match (ty, gate_levels(conn)?) {
                (Some(t), Some(l)) => lacking_of(&l, t),
                (Some(t), None) => direct_requirements(t),
                _ => Vec::new(),
            };
            let missing = learn_urgent_soft(conn, &lack)?;
            Ok(Err(format!(
                "online waits on skills {lack:?}{}",
                missing_note(&missing)
            )))
        }
        Err(e) => Err(e),
    }
}

/// **`fit_auto`, soft on the skill gate** (SKILLGATE B7): a refusal for the
/// pilot's skills queues what the items ask (ahead of the rest) and is
/// answered `Ok(Err(note))` -- not a failure to count, the caller waits like
/// for any missing thing. Every other error passes.
pub fn fit_auto_gated(
    conn: &mut Conn,
    item_ids: &[i64],
) -> Result<std::result::Result<Vec<crate::ItemRow>, String>> {
    match conn.fit_auto(item_ids) {
        Ok(rows) => Ok(Ok(rows)),
        Err(e) if is_skill_refusal(&e) => {
            let types: Vec<i64> = conn
                .hangar_items()?
                .iter()
                .filter(|r| item_ids.contains(&r.item_id))
                .map(|r| r.type_id)
                .collect();
            let lack = match gate_levels(conn)? {
                Some(l) => types
                    .iter()
                    .flat_map(|t| lacking_of(&l, *t))
                    .collect::<Vec<_>>(),
                None => requirements_of(types.iter().copied()),
            };
            let missing = learn_urgent_soft(conn, &lack)?;
            Ok(Err(format!(
                "fit waits on skills {lack:?}{}",
                missing_note(&missing)
            )))
        }
        Err(e) => Err(e),
    }
}

/// [`learn_urgent`] as a side effect: a refusal (not docked, a book not sold
/// here) is dropped, and only a dead connection is the error. What could not
/// be bought or queued comes back as the `missing` lines, for the caller to
/// log (a wait on a book that never comes must not be silent).
pub fn learn_urgent_soft(conn: &mut Conn, skills: &[(i64, i64)]) -> Result<Vec<String>> {
    if skills.is_empty() {
        return Ok(Vec::new());
    }
    match learn_urgent(conn, skills, None) {
        Ok(got) => Ok(got.missing),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => Ok(vec![format!("queueing {skills:?}: {e}")]),
    }
}

/// The `missing` lines of [`learn_urgent_soft`] as a suffix for a note.
pub fn missing_note(missing: &[String]) -> String {
    if missing.is_empty() {
        String::new()
    } else {
        format!(" ({})", missing.join("; "))
    }
}

/// **The pilot's trained skills** (X-TRADE): `{typeID: trained level}` from
/// `GetSkills()` on its skill handler. A skill that is injected but not yet
/// trained is present at level 0, so a key means "the book is in the
/// pilot's brain".
pub fn skill_levels(conn: &mut Conn) -> Result<BTreeMap<i64, i64>> {
    let handler = skill_handler(conn)?;
    known_skills(conn, &handler)
}

/// **The pilot's skill queue** (X-SKILL): `(skill, to-level)` rows, in order.
pub fn skill_queue(conn: &mut Conn) -> Result<Vec<(i64, i64)>> {
    let handler = skill_handler(conn)?;
    queue_of(conn, &handler)
}

/// **The pilot's unallocated skill points** (WS-3): the second item of
/// `GetSkillQueueAndFreePoints()`.
pub fn free_points(conn: &mut Conn) -> Result<i64> {
    let handler = skill_handler(conn)?;
    Ok(queue_and_free(conn, &handler)?.1)
}

/// **Free points onto the queue** (WS-3), the human's "apply" button: the
/// preview (`GetFreeSkillPointsAppliedToQueue`), then, when it is not
/// empty, `ApplyFreeSkillPointsToQueue`. Returns the points spent. The
/// points come from a world rule equal for every pilot; they land only on
/// skills whose book the bot already bought, so nothing comes from nowhere.
pub fn apply_free_to_queue(conn: &mut Conn) -> Result<i64> {
    let handler = skill_handler(conn)?;
    let outcome = conn.call_on(&handler, "GetFreeSkillPointsAppliedToQueue", vec![], vec![])?;
    // `{typeID: points}`, on the wire `(value, key)`.
    let spent: i64 = match outcome.value() {
        Some(PyValue::Dict(entries)) => entries
            .iter()
            .filter_map(|(v, _)| match v {
                PyValue::Int(p) => Some(*p),
                _ => None,
            })
            .sum(),
        other => {
            return Err(Error::Refused(format!(
                "GetFreeSkillPointsAppliedToQueue answered {other:?}"
            )));
        }
    };
    if spent <= 0 {
        return Ok(0);
    }
    conn.call_on(&handler, "ApplyFreeSkillPointsToQueue", vec![], vec![])?;
    Ok(spent)
}

/// **Skills and goods, bought** (K-DRONE): each of `skills` the pilot lacks
/// learned, then each `(type, count)` of `items` bought up to its count less
/// what the pilot already holds ([`held`]) -- [`run_with`]'s `more` for a
/// caller that decides at run time (the fighters' combat drones).
pub fn buy_more(
    conn: &mut Conn,
    skills: &[(i64, i64)],
    items: &[(i64, i64)],
    trips: Option<Trips<'_>>,
) -> Result<Acquired> {
    let (station, mut got, mut prices) = begin(conn)?;
    let mut shop = Shop {
        station,
        prices: &mut prices,
        trips,
    };
    let front = requirements_of(items.iter().map(|p| p.0));
    learn(conn, skills, &front, &mut shop, &mut got)?;
    for &(t, n) in items {
        let short = n - held(conn, t)?;
        if short > 0 {
            buy(conn, &mut shop, t, short, Haul::Cargo, &mut got)?;
        }
    }
    got.after = conn.cash_balance()?;
    Ok(got)
}

/// One acquisition's start, docked: the station, the record with the
/// wallet before, and the prices (none when the read is refused).
fn begin(conn: &mut Conn) -> Result<(i64, Acquired, Prices)> {
    let station = conn
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("acquire: not docked".to_owned()))?;
    let got = Acquired {
        before: conn.cash_balance()?,
        ..Acquired::default()
    };
    let prices = match Prices::read(conn) {
        Ok(p) => p,
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(_) => Prices::new(BTreeMap::new(), eve_server::pyutil::filetime_now()),
    };
    Ok((station, got, prices))
}

/// The hulls a kit flies, best first: the miner's
/// [`brain::MINER_HULLS`] (GAPS-B: a Venture, with its mining hold and
/// drone bay, else the Merlin), every other kit's one hull.
pub fn hulls_of(kit: &Kit) -> Vec<i64> {
    if *kit == MINER_KIT {
        crate::mine_ladder::LEGACY.to_vec()
    } else if *kit == crate::ice::ICE_KIT {
        // ICE-HULL: the ladder's barges and exhumers, nothing else fits a
        // harvester.
        crate::mine_ladder::ice_hulls().map(|h| h.type_id).collect()
    } else {
        kit.hull.into_iter().collect()
    }
}

/// The hull type the pilot flies, from the hangar's row for the active ship.
fn active_hull(conn: &mut Conn) -> Result<Option<i64>> {
    let Some(ship) = conn.session().ship_id() else {
        return Ok(None);
    };
    Ok(conn
        .hangar_items()?
        .iter()
        .find(|r| r.item_id == ship)
        .map(|r| r.type_id))
}

/// **The hulls to try** (PROG-MINE): for the miner's kit the ladder's
/// candidates first ([`crate::mine_ladder::candidates_in`]: flyable,
/// affordable, a quarter better than the hull flown), then [`hulls_of`]; for
/// every other kit [`hulls_of`] alone. A refused read of skills or prices
/// leaves the ladder out, not the legacy tail.
fn hulls_in(conn: &mut Conn, kit: &Kit, prices: &Prices) -> Result<Vec<i64>> {
    let mut out = Vec::new();
    if *kit == MINER_KIT {
        match crate::mine_ladder::candidates_in(conn, prices) {
            Ok((c, flown)) => {
                out = c;
                // A Venture or better has nothing to fall back to: with no
                // better hull to be had, no purchase is tried at all.
                if flown.is_some_and(crate::mine_ladder::venture_or_better) {
                    return Ok(out);
                }
            }
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => {}
        }
    } else if *kit == crate::ice::ICE_KIT {
        // ICE-HULL: the ladder's ice hulls the pilot can fly and pay for.
        match crate::mine_ladder::ice_candidates_in(conn, prices) {
            Ok((c, _)) => out = c,
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => {}
        }
    }
    // SKILLGATE: only a hull the pilot's skills board; a refused read of
    // them filters nothing.
    let levels = match skill_levels(conn) {
        Ok(l) => Some(l),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(_) => None,
    };
    for h in hulls_of(kit) {
        if !out.contains(&h) && levels.as_ref().is_none_or(|l| flyable(l, h)) {
            out.push(h);
        }
    }
    Ok(out)
}

/// Units of `type_id` the pilot holds: in the station hangar, in the
/// active ship's hold, fitted, and (SOC-B10) in its drone bay.
pub(crate) fn held(conn: &mut Conn, type_id: i64) -> Result<i64> {
    let count = |rows: Vec<crate::ItemRow>| -> i64 {
        rows.iter()
            .filter(|r| r.type_id == type_id)
            .map(|r| r.quantity.max(1))
            .sum()
    };
    let bay = match conn.session().ship_id() {
        Some(ship) => count(conn.ship_items(ship, Some(crate::ore::FLAG_DRONE_BAY))?),
        None => 0,
    };
    Ok(count(conn.hangar_items()?) + count(conn.cargo()?) + count(conn.fitted()?) + bay)
}

/// Where one [`run_with`] buys: its station, the prices, and the trips it
/// may make.
struct Shop<'a, 'b> {
    station: i64,
    prices: &'a mut Prices,
    trips: Option<Trips<'b>>,
}

/// Buy up to `units` of `type_id` at the shop's station by [`plan`], and
/// with trips the rest from the nearest seller within reach; the units got.
/// Short of `units`, one missing line.
fn buy(
    conn: &mut Conn,
    shop: &mut Shop<'_, '_>,
    type_id: i64,
    units: i64,
    haul: Haul,
    got: &mut Acquired,
) -> Result<i64> {
    let station = shop.station;
    let cap = shop.prices.reservation(conn, type_id)?;
    let book = match conn.market_orders(type_id) {
        Ok(b) => b,
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(e) => {
            got.missing.push(format!("type {type_id}: no book ({e})"));
            return Ok(0);
        }
    };
    let isk = conn.cash_balance()?;
    let mut n = 0;
    for (ask, k) in plan(&book, station, cap, isk, units) {
        match conn.buy(station, type_id, ask.price, k, 0, BROKER_FEE) {
            Ok(row) => {
                let filled = k - row.vol_remaining.clamp(0, k);
                if filled > 0 {
                    n += filled;
                    got.bought
                        .push((type_id, filled, filled as f64 * ask.price));
                }
            }
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(e) => {
                got.missing
                    .push(format!("type {type_id}: buy refused ({e})"));
                return Ok(n);
            }
        }
    }
    if n < units
        && let Some(trips) = shop.trips
    {
        n += errand(conn, trips, type_id, units - n, cap, haul, got)?;
    }
    if n < units {
        let cap = cap.map_or_else(|| "no cap".to_owned(), |c| format!("cap {c:.2}"));
        got.missing.push(format!(
            "{} of type {type_id}: asks at station {station} ({cap}, wallet {isk:.2} ISK)",
            units - n
        ));
    }
    Ok(n)
}

/// **The errand** (the travel lane): the nearest seller of `type_id`
/// within reach that the wallet covers ([`crate::travel::find`]), there
/// and back ([`crate::travel::fetch`]); the units brought home. Its
/// summary is a trip line, its refusals missing lines.
fn errand(
    conn: &mut Conn,
    trips: Trips<'_>,
    type_id: i64,
    units: i64,
    cap: Option<f64>,
    haul: Haul,
    got: &mut Acquired,
) -> Result<i64> {
    let source = match crate::travel::find(conn, trips, type_id, units, cap) {
        Ok(Some(s)) => s,
        Ok(None) => return Ok(0),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(e) => {
            got.missing
                .push(format!("type {type_id}: no seller found ({e})"));
            return Ok(0);
        }
    };
    let fetched = match crate::travel::fetch(conn, trips, &source, type_id, units, cap, haul) {
        Ok(f) => f,
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(e) => {
            got.missing.push(format!(
                "type {type_id}: trip to station {} failed ({e})",
                source.station
            ));
            return Ok(0);
        }
    };
    got.bought.extend(fetched.bought.iter().copied());
    got.trips.push(fetched.summary());
    Ok(fetched.home)
}

/// An assembled `hull` in the hangar, assembling a packaged one (a hull
/// bought off the market comes packaged: the seller had to repackage it)
/// with `ship.AssembleShip(itemIDs)` on the station's `ship` moniker
/// (`invItemFunctions.py:201`, `service/inventory.rs`) [V]. `false`, with a
/// missing line, when there is none or the server refuses.
fn assemble(conn: &mut Conn, station: i64, hull: i64, got: &mut Acquired) -> Result<bool> {
    match assemble_hull(conn, station, hull)? {
        Ok(present) => Ok(present),
        Err(why) => {
            got.missing.push(why);
            Ok(false)
        }
    }
}

/// [`assemble`]'s work: `Ok(true)` with an assembled `hull` in the hangar,
/// `Ok(false)` with none at all, `Err(why)` when the server refused.
pub(crate) fn assemble_hull(
    conn: &mut Conn,
    station: i64,
    hull: i64,
) -> Result<std::result::Result<bool, String>> {
    let rows: Vec<crate::ItemRow> = conn
        .hangar_items()?
        .into_iter()
        .filter(|r| r.type_id == hull)
        .collect();
    if rows.iter().any(|r| r.singleton) {
        return Ok(Ok(true));
    }
    let Some(packaged) = rows.first() else {
        return Ok(Ok(false));
    };
    let answer = conn.moniker_call(
        "ship",
        crate::station_moniker(station),
        "AssembleShip",
        vec![PyValue::List(vec![PyValue::Int(packaged.item_id)])],
        vec![],
    )?;
    Ok(match answer.outcome.refusal() {
        None => Ok(true),
        Some(key) => Err(format!("assembling hull {}: {key}", packaged.item_id)),
    })
}

/// **The pilot sits in a capsule**: the active ship's hangar row is the
/// capsule's type, or the hangar does not list it.
pub(crate) fn flying_capsule(conn: &mut Conn) -> Result<bool> {
    let ship = conn.session().ship_id();
    Ok(conn
        .hangar_items()?
        .iter()
        .find(|r| Some(r.item_id) == ship)
        .is_none_or(|r| r.type_id == brain::CAPSULE))
}

/// Board the corvette in the hangar, else ask for one (module header, 3).
fn rookie(conn: &mut Conn, station: i64, got: &mut Acquired) -> Result<()> {
    match rookie_ship(conn, station)? {
        Ok(r) => got.rookie = Some(r),
        Err(why) => got.missing.push(why),
    }
    Ok(())
}

/// [`rookie`]'s work: how the rookie ship came, or why it did not.
pub(crate) fn rookie_ship(
    conn: &mut Conn,
    station: i64,
) -> Result<std::result::Result<Rookie, String>> {
    let me = conn.session().char_id();
    let corvette = conn.hangar_items()?.into_iter().find(|r| {
        r.category_id == CATEGORY_SHIP
            && r.group_id == eve_server::starter_ship::GROUP_CORVETTE
            && me.is_none_or(|m| r.owner_id == m)
    });
    if let Some(c) = corvette {
        return match conn.board(c.item_id) {
            Ok(_) => Ok(Ok(Rookie::Boarded(c.item_id))),
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
            Err(e) => Ok(Err(format!("boarding rookie ship {}: {e}", c.item_id))),
        };
    }
    let ship = conn.session().ship_id().map_or(PyValue::None, PyValue::Int);
    let outcome = conn.call(
        "dogmaIM",
        "CreateNewbieShip",
        vec![ship, PyValue::Int(station)],
    )?;
    Ok(match outcome.refusal() {
        None => Ok(Rookie::Created),
        Some(key) => Err(format!("a rookie ship: {key}")),
    })
}

thread_local! {
    /// The client's static data, for [`learn`]'s prerequisites: the driver
    /// hands it over on the bot's own thread before it acquires
    /// ([`use_content`]); without it a kit's skills are queued as given.
    static CONTENT: std::cell::RefCell<Option<std::sync::Arc<eve_server::content::Content>>> =
        const { std::cell::RefCell::new(None) };
}

/// Let [`learn`] read skill prerequisites from `content` (this thread's).
pub fn use_content(content: &std::sync::Arc<eve_server::content::Content>) {
    CONTENT.with(|c| *c.borrow_mut() = Some(std::sync::Arc::clone(content)));
}

/// **Whether `hull` takes `module`** (FIT-REFUSE): the server's own rule
/// ([`eve_server::fitting::hull_allows`]) over the content's rows. True
/// when the answer is unknown (no [`use_content`], no typedata, a type the
/// table lacks), so a bot with no bundle behaves as before; the server
/// still refuses what it should.
pub fn hull_takes(module: i64, hull: i64) -> bool {
    CONTENT.with(|c| {
        let c = c.borrow();
        let Some(t) = c
            .as_ref()
            .and_then(|c| c.try_get::<eve_server::typedata::Installed>())
        else {
            return true;
        };
        let (Some(h), Some(m)) = (t.item(hull), t.table().get(module)) else {
            return true;
        };
        eve_server::fitting::hull_allows(|a| m.attribute(a), h.group_id, hull)
    })
}

/// A type's id from what `/createitem` takes (SKILLGATE): a number, or a
/// name the content's typedata knows. `None` without either.
pub(crate) fn type_named(needle: &str) -> Option<i64> {
    if let Ok(id) = needle.parse::<i64>() {
        return Some(id);
    }
    CONTENT.with(|c| {
        let c = c.borrow();
        let c = c.as_ref()?;
        c.try_get::<eve_server::typedata::Installed>()?;
        eve_server::typedata::grantable_in(c, needle).map(|t| t.type_id)
    })
}

/// **What a type asks of its pilot, directly** (SKILLGATE): every
/// `(skill, level)` of the six requirement pairs on the type's dogma sheet
/// (`drone_skills::required_skills`, the gate's own reader), not the
/// prerequisites' closure -- the client's `GetMissingSkills` is as direct.
/// Empty without [`use_content`] or a typedata table, so a bot with no
/// bundle behaves as before the gate.
pub fn direct_requirements(type_id: i64) -> Vec<(i64, i64)> {
    CONTENT.with(|c| {
        let c = c.borrow();
        let Some(t) = c
            .as_ref()
            .and_then(|c| c.try_get::<eve_server::typedata::Installed>())
        else {
            return Vec::new();
        };
        eve_server::drone_skills::required_skills(t.table(), type_id)
    })
}

/// The `(skill, level)` of `type_id`'s direct requirements `levels` (trained
/// levels, `{skill: level}`) does not reach.
pub fn lacking_of(levels: &BTreeMap<i64, i64>, type_id: i64) -> Vec<(i64, i64)> {
    direct_requirements(type_id)
        .into_iter()
        .filter(|(s, l)| levels.get(s).copied().unwrap_or(0) < *l)
        .collect()
}

/// **Whether the pilot may board or use `type_id`** (SKILLGATE, pure over
/// the trained levels): every direct requirement met. True when nothing is
/// known about the type (no content).
pub fn flyable(levels: &BTreeMap<i64, i64>, type_id: i64) -> bool {
    lacking_of(levels, type_id).is_empty()
}

/// The direct requirements of each of `types`, once per skill at the larger
/// level, in the order the types are given.
pub fn requirements_of(types: impl IntoIterator<Item = i64>) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = Vec::new();
    for t in types {
        for (s, l) in direct_requirements(t) {
            match out.iter_mut().find(|p| p.0 == s) {
                Some(p) => p.1 = p.1.max(l),
                None => out.push((s, l)),
            }
        }
    }
    out
}

/// What the parts of an order that `lacking` asks to buy need of their pilot
/// ([`requirements_of`]): the hull only when `lacking.ship`, the modules only
/// when `lacking.module`, the rounds only when `lacking.ammo`. A part that is
/// not bought must not put its skill ahead of the queue (a fighter's kit
/// names a placeholder Rifter its chooser never buys).
pub fn requirements(order: &Order, lacking: Lacking) -> Vec<(i64, i64)> {
    requirements_of(
        order
            .hull
            .filter(|_| lacking.ship)
            .into_iter()
            .chain(order.modules.iter().filter(|_| lacking.module).map(|p| p.0))
            .chain(order.charges.iter().filter(|_| lacking.ammo).map(|p| p.0)),
    )
}

/// The role's skills (module header, 1). `front` are the skills the order's
/// parts need (SKILLGATE): their levels, prerequisites first, go at the
/// **front** of the queue (ahead of what is queued already), so a full queue
/// does not delay the first hull by days.
fn learn(
    conn: &mut Conn,
    skills: &[(i64, i64)],
    front: &[(i64, i64)],
    shop: &mut Shop<'_, '_>,
    got: &mut Acquired,
) -> Result<()> {
    if skills.is_empty() && front.is_empty() {
        return Ok(());
    }
    let handler = skill_handler(conn)?;
    let known = known_skills(conn, &handler)?;
    let mut queue = queue_of(conn, &handler)?;
    let original = queue.clone();
    let mut learned = Vec::new();
    // Prerequisites in front, at the table's levels: the server refuses a
    // level queued before the skills it needs (`QueueCannotPlaceSkillBefore
    // Requirements`), and a kit's gun skill needs Gunnery.
    let expand = |list: &[(i64, i64)]| match CONTENT.with(|c| c.borrow().clone()) {
        Some(c) => {
            crate::skill_plan::expand(list, |s| eve_server::skill::required_in(&c, s).to_vec())
        }
        None => list.to_vec(),
    };
    let front = expand(front);
    let wanted = expand(skills);
    // The levels queued at the front, in order; what the queue holds of them
    // already moves up (a prerequisite must still come before its skill).
    let mut head: Vec<(i64, i64)> = Vec::new();
    for (list, urgent) in [(&front, true), (&wanted, false)] {
        for &(skill, level) in list {
            let have = known.get(&skill).copied();
            if have.is_some_and(|l| l >= level) {
                continue;
            }
            if have.is_none() && !inject_bought(conn, shop, skill, got)? {
                continue;
            }
            let mut queued = false;
            for l in have.unwrap_or(0) + 1..=level {
                if urgent {
                    queue.retain(|e| *e != (skill, l));
                    if !head.contains(&(skill, l)) {
                        head.push((skill, l));
                        queued = true;
                    }
                } else if !queue.contains(&(skill, l)) && !head.contains(&(skill, l)) {
                    queue.push((skill, l));
                    queued = true;
                }
            }
            if queued {
                learned.push(skill);
            }
        }
    }
    if !head.is_empty() {
        queue.splice(0..0, head);
    }
    if queue == original {
        return Ok(());
    }
    let info = PyValue::Dict(
        queue
            .iter()
            .enumerate()
            .map(|(i, &(t, l))| {
                (
                    PyValue::Tuple(vec![PyValue::Int(t), PyValue::Int(l)]),
                    PyValue::Int(i as i64),
                )
            })
            .collect(),
    );
    // `CommitTransaction`: one positional dict, `activate` a keyword; kwargs
    // are `(value, key)` on the wire (`skill_queue.rs::save_new_queue`) [V].
    let outcome = conn.call_on(
        &handler,
        "SaveNewQueue",
        vec![info],
        vec![(PyValue::Bool(true), crate::bstr("activate"))],
    )?;
    match outcome.refusal() {
        None => got.learned.extend(learned),
        Some(key) => got.missing.push(format!("queueing {learned:?}: {key}")),
    }
    Ok(())
}

/// Buy `skill`'s book here and inject it; `false` (with a missing line) when
/// either did not happen.
fn inject_bought(
    conn: &mut Conn,
    shop: &mut Shop<'_, '_>,
    skill: i64,
    got: &mut Acquired,
) -> Result<bool> {
    let in_hangar = |conn: &mut Conn| -> Result<Option<i64>> {
        Ok(conn
            .hangar_items()?
            .into_iter()
            .find(|r| r.type_id == skill)
            .map(|r| r.item_id))
    };
    let book = match in_hangar(conn)? {
        Some(b) => b,
        None => {
            if buy(conn, shop, skill, 1, Haul::Cargo, got)? == 0 {
                return Ok(false);
            }
            let Some(b) = in_hangar(conn)? else {
                got.missing
                    .push(format!("skillbook {skill}: bought, not in the hangar"));
                return Ok(false);
            };
            b
        }
    };
    // `skillsvc.InjectSkillIntoBrain`: one list of item ids (`character.rs`,
    // `Skill training`) [V].
    let outcome = conn.call(
        "dogmaIM",
        "InjectSkillIntoBrain",
        vec![PyValue::List(vec![PyValue::Int(book)])],
    )?;
    if let Some(key) = outcome.refusal() {
        got.missing
            .push(format!("injecting skillbook {skill}: {key}"));
        return Ok(false);
    }
    Ok(true)
}

/// `skillMgr2.GetMySkillHandler()`: an object reference, the client's
/// `skillsvc.GetSkillHandler` [V].
pub(crate) fn skill_handler(conn: &mut Conn) -> Result<ObjectRef> {
    let outcome = conn.call("skillMgr2", "GetMySkillHandler", vec![])?;
    outcome
        .value()
        .as_ref()
        .and_then(|v| crate::object_refs(v).into_iter().next())
        .ok_or_else(|| Error::Protocol("GetMySkillHandler: no object reference".to_owned()))
}

/// `GetSkills()` on the handler: `{typeID: CharacterSkillEntry}` -> levels.
fn known_skills(conn: &mut Conn, handler: &ObjectRef) -> Result<BTreeMap<i64, i64>> {
    let outcome = conn.call_on(handler, "GetSkills", vec![], vec![])?;
    let Some(PyValue::Dict(entries)) = outcome.value() else {
        return Err(Error::Refused(format!(
            "GetSkills answered {:?}",
            outcome.value()
        )));
    };
    Ok(entries
        .iter()
        .filter_map(|(entry, key)| {
            let PyValue::Int(t) = key else { return None };
            let level = match field(entry, "_trainedSkillLevel", 0) {
                Some(PyValue::Int(l)) => *l,
                _ => 0,
            };
            Some((*t, level))
        })
        .collect())
}

/// `GetSkillQueueAndFreePoints()` on the handler: the queue as
/// `(trainingTypeID, trainingToLevel)` rows, in order.
fn queue_of(conn: &mut Conn, handler: &ObjectRef) -> Result<Vec<(i64, i64)>> {
    Ok(queue_and_free(conn, handler)?.0)
}

/// [`queue_of`] and the pilot's unallocated skill points (the reply's second
/// item).
fn queue_and_free(conn: &mut Conn, handler: &ObjectRef) -> Result<(Vec<(i64, i64)>, i64)> {
    let outcome = conn.call_on(handler, "GetSkillQueueAndFreePoints", vec![], vec![])?;
    let (rows, free) = match outcome.value() {
        Some(PyValue::Tuple(pair)) => {
            let mut it = pair.into_iter();
            let rows = match it.next() {
                Some(PyValue::List(rows)) => rows,
                _ => Vec::new(),
            };
            let free = match it.next() {
                Some(PyValue::Int(n)) => n,
                _ => 0,
            };
            (rows, free)
        }
        other => {
            return Err(Error::Refused(format!(
                "GetSkillQueueAndFreePoints answered {other:?}"
            )));
        }
    };
    let queue = rows
        .iter()
        .filter_map(|r| {
            match (
                field(r, "trainingTypeID", 0),
                field(r, "trainingToLevel", 0),
            ) {
                (Some(PyValue::Int(t)), Some(PyValue::Int(l))) => Some((*t, *l)),
                _ => None,
            }
        })
        .collect();
    Ok((queue, free))
}

/// The value under `name` in any dict inside `v` (an instance's state, an
/// `ObjectEx`'s header): `(value, key)` entries, the key as bytes.
pub(crate) fn field<'a>(v: &'a PyValue, name: &str, depth: usize) -> Option<&'a PyValue> {
    if depth > 6 {
        return None;
    }
    match v {
        PyValue::Dict(entries) => entries
            .iter()
            .find(|(_, k)| match k {
                PyValue::Bytes(b) => b == name.as_bytes(),
                PyValue::Unicode(s) => s == name,
                _ => false,
            })
            .map(|(value, _)| value)
            .or_else(|| {
                entries
                    .iter()
                    .find_map(|(value, _)| field(value, name, depth + 1))
            }),
        PyValue::Tuple(items) | PyValue::List(items) => {
            items.iter().find_map(|i| field(i, name, depth + 1))
        }
        PyValue::Instance { state, .. } => field(state, name, depth + 1),
        PyValue::ObjectEx { header, .. } => field(header, name, depth + 1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    //! The harness: one in-process server per test over invented furniture
    //! -- an invented race, its invented corvette (`typedata::FIXTURE`'s
    //! frigate numbers in `groupCorvette`), a fixture school at the home
    //! station whose corporation lists the roles' skillbooks, and asks a
    //! human stand-in posts. No GM slash from a bot: each test checks its
    //! log for none.
    use super::*;
    use crate::brain::{ActKind, Policy};
    use crate::driver::Verdict;
    use crate::driver::{BotConfig, Handle, Pace};
    use crate::verbs::SellLine;
    use eve_server::econ_exchange::{self as exchange, CATEGORY_SKILL};
    use eve_server::model::Character;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Instant;

    // Invented ids only, never a client value.
    const RACE: i64 = 91;
    const HULL: i64 = 990_870_001;
    /// `typedata::FIXTURE`'s autocannon: the rookie fit's one module.
    const FIXTURE_GUN: i64 = 990_000_002;
    const SCHOOL_CORP: i64 = 1_999_501;
    const SKILL_GROUP: i64 = 99_016;
    const BOOK_REF: f64 = 30_000.0;
    const WALLET: f64 = 5_000_000.0;
    /// PROG-PVE2: the Rifter's invented reference price, and the other
    /// weapon systems' types (the table's own ids, invented numbers).
    const HULL_REF: f64 = 400_000.0;
    const PUNISHER: i64 = 597;
    const ATRON: i64 = 608;
    const LASER_GUN: i64 = 453;
    const HYBRID_GUN: i64 = 561;
    const CRYSTAL: i64 = 246;
    const CHARGE: i64 = 222;
    const PLATE: i64 = 11_293;
    const MEMBRANE: i64 = 11_267;
    const MINER: i64 = 90_000_101;
    const ESCORT: i64 = 90_000_102;
    const ALICE: i64 = 90_000_103;
    /// Invented asks: low enough for any `P_ref` cap.
    const HULL_ASK: f64 = 1_000.0;
    const GUN_ASK: f64 = 100.0;
    const ROUND_ASK: f64 = 1.0;
    /// How long a bot may take to get there.
    const WITHIN: Duration = Duration::from_secs(30);

    fn home() -> Character {
        Character::default()
    }

    /// `Content::fixture()` plus the corvette, the race's rookie fit, the
    /// books' prices and the school.
    fn content() -> eve_server::content::Content {
        content_gated(None)
    }

    /// [`content`], the corvette asking for `gate` at level 1 (SKILLGATE).
    fn content_gated(gate: Option<i64>) -> eve_server::content::Content {
        let ask = gate.map_or(String::new(), |s| format!(";182={s};277=1"));
        let hull = format!(
            "type\t{HULL}\t{}\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
             9=310.0;11=45.0;12=4.0;13=3.0;14=3.0;37=355.0;48=125.0;55=150000.0;70=3.1;\
             101=0.0;102=2.0;552=40.0;600=5.0{ask}\tFixture Corvette\n",
            eve_server::starter_ship::GROUP_CORVETTE
        );
        // PROG-PVE: a cruiser, a medium gun and its rounds on the fixture's
        // invented numbers, at the table's own type ids (the bots' hull and
        // fit tables are keyed by them).
        let medium = format!(
            "type\t{RUPTURE}\t26\t6\t1\t1\t96000.0\t450.0\t12200000.0\t88.0\t0\t5000\t\
             9=1500.0;11=860.0;12=5.0;13=4.0;14=5.0;37=210.0;48=350.0;55=425000.0;70=0.5;\
             76=50000.0;101=0.0;102=4.0;192=6.0;263=1500.0;265=1800.0;271=0.0;272=0.5;273=0.6;\
             274=0.8;479=1250000.0;482=1275.0;552=125.0;564=290.0;600=4.0;1137=3.0;1281=1.0\tFixture Cruiser\n\
             type\t{MEDIUM_GUN}\t55\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t34\t12,16,34,42\t\
             6=0.0;30=2.0;50=4.0;51=2500.0;54=900.0;64=2.5;128=2.0;158=4000.0;160=400.0;604=83.0\tFixture Medium Gun\n\
             type\t{MEDIUM_ROUND}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t\
             114=7.0;117=3.0;128=2.0\tFixture Medium Rounds\n",
            RUPTURE = 629,
            MEDIUM_GUN = crate::fit_plan::MEDIUM_GUN,
            MEDIUM_ROUND = crate::fit_plan::MEDIUM_ROUND,
        );
        // PROG-PVE2: two more frigates (an Amarr and a Gallente one), a small
        // laser and hybrid gun with their rounds, and a tank module of each
        // rack, all at the table's own type ids on the fixture's invented
        // numbers. The effects are the fixture autocannon's (34, the
        // server's weapon effect), the slot effects (11 low, 13 medium) and
        // online (16).
        let frigate = |id: i64, name: &str| {
            format!(
                "type\t{id}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
                 9=310.0;11=45.0;12=5.0;13=2.0;14=4.0;37=355.0;48=125.0;55=150000.0;70=3.1;\
                 76=21000.0;101=0.0;102=4.0;192=5.0;263=420.0;265=440.0;271=0.0;272=0.5;\
                 273=0.6;274=0.8;479=600000.0;482=260.0;564=640.0;600=5.0;1137=3.0;1281=1.0\t{name}\n"
            )
        };
        let small_gun = |id: i64, group: i64, charge_group: i64, name: &str| {
            format!(
                "type\t{id}\t{group}\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t34\t12,16,34,42\t\
                 6=0.0;30=2.0;50=4.0;51=2500.0;54=900.0;64=2.5;128=1.0;158=4000.0;160=400.0;\
                 604={charge_group}.0\t{name}\n"
            )
        };
        let small_round = |id: i64, group: i64, name: &str| {
            format!(
                "type\t{id}\t{group}\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t\
                 114=7.0;117=3.0;128=1.0\t{name}\n"
            )
        };
        let tank_module = |id: i64, group: i64, slot_effect: i64, name: &str| {
            format!(
                "type\t{id}\t{group}\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t{slot_effect},16\t\
                 30=3.0;50=10.0\t{name}\n"
            )
        };
        let more = format!(
            "{}{}{}{}{}{}{}{}",
            frigate(PUNISHER, "Fixture Amarr Frigate"),
            frigate(ATRON, "Fixture Gallente Frigate"),
            small_gun(LASER_GUN, 53, 86, "Fixture Small Laser"),
            small_gun(HYBRID_GUN, 74, 85, "Fixture Small Hybrid"),
            small_round(CRYSTAL, 86, "Fixture Crystal S"),
            small_round(CHARGE, 85, "Fixture Hybrid Charge S"),
            tank_module(PLATE, 329, 11, "Fixture Plate"),
            tank_module(MEMBRANE, 326, 11, "Fixture Membrane"),
        );
        // SKILLGATE: a gated content also has a Merlin (the miner's hull) that
        // asks for the same skill.
        let merlin = gate.map_or(String::new(), |s| {
            format!(
                "type\t{}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
                 9=310.0;11=45.0;12=4.0;13=3.0;14=3.0;37=355.0;48=125.0;55=150000.0;70=3.1;\
                 101=0.0;102=2.0;552=40.0;600=5.0;182={s};277=1\tFixture Merlin\n",
                brain::MERLIN
            )
        });
        let text = format!(
            "{}{hull}{medium}{more}{merlin}",
            eve_server::typedata::FIXTURE
        );
        let td = eve_server::typedata::Table::parse(&text).expect("the test rows parse");
        let mut starter = eve_server::starter_ship::Table::default();
        starter
            .parse_races(&[format!("{RACE}\t{HULL}\t\t1")])
            .expect("parses");
        starter.add_fit(&[(HULL, FIXTURE_GUN)]);

        let books = [crate::roles::SKILL_MINING, crate::escort::SKILL_GUNNERY];
        let mut prices = eve_server::econ_prices::Table::fixture();
        let lines: Vec<String> = books
            .iter()
            .map(|b| {
                format!("{b}\t{BOOK_REF}\tbase\t{BOOK_REF}\t{CATEGORY_SKILL}\t{SKILL_GROUP}\t\t1")
            })
            .collect();
        prices.prices.extend(
            eve_server::econ_prices::Table::parse(&lines)
                .expect("parses")
                .prices,
        );
        // PROG-PVE2: the Rifter has a reference price, so a spare hull of
        // it can be listed.
        prices.prices.extend(
            eve_server::econ_prices::Table::parse(&[format!(
                "{}\t{HULL_REF}\tbase\t{HULL_REF}\t{CATEGORY_SHIP}\t25\t\t1",
                crate::escort::RIFTER
            )])
            .expect("parses")
            .prices,
        );
        let h = home();
        let station = h.station_id.expect("the default pilot is docked");
        let row = exchange::TypeRow {
            group_id: SKILL_GROUP,
            category_id: CATEGORY_SKILL,
            base_price: BOOK_REF,
            meta_group_id: None,
            tech_level: None,
        };
        let trades: Vec<(i64, i64, f64)> = books.iter().map(|&b| (SCHOOL_CORP, b, 1.0)).collect();
        let table = exchange::Table::with_schools(
            vec![exchange::Station {
                station_id: station,
                system_id: h.solar_system_id.expect("in a system"),
                region_id: h.region_id,
                owner_id: SCHOOL_CORP,
            }],
            &trades,
            books.iter().map(|&b| (b, row)).collect(),
            &[],
            &[exchange::School {
                corp_id: SCHOOL_CORP,
                stations: vec![station],
            }],
        )
        .expect("the exchange builds");

        let mut c = eve_server::content::Content::fixture();
        c.insert("typedata", eve_server::typedata::Installed::build(td));
        c.insert(eve_server::starter_ship::TABLE.name, starter);
        c.insert("econ_prices", prices);
        c.insert("econ_exchange", table);
        // PROG-PVE: the two skills the upgrade test grants (invented rows
        // at the client's ids, which the bots' tables are keyed by).
        let skills = "skill\t3333\t990020100\t1\t165\t166\t0.01\t\tFixture Cruiser Skill\n\
             skill\t3305\t990020100\t1\t165\t166\t0.01\t\tFixture Medium Gun Skill\n\
             skill\t3303\t990020100\t1\t165\t166\t0.01\t\tFixture Small Energy Skill\n\
             skill\t3301\t990020100\t1\t165\t166\t0.01\t\tFixture Small Hybrid Skill\n\
             skill\t3394\t990020100\t1\t165\t166\t0.01\t\tFixture Hull Upgrades\n";
        c.insert(
            eve_server::skill::TABLE.name,
            eve_server::skill::Table::parse(skills).expect("the skill rows parse"),
        );
        c
    }

    fn start() -> SocketAddr {
        start_at(eve_server::game_clock::GameClock::REAL)
    }

    fn start_at(clock: eve_server::game_clock::GameClock) -> SocketAddr {
        start_with(clock, content())
    }

    fn start_with(
        clock: eve_server::game_clock::GameClock,
        content: eve_server::content::Content,
    ) -> SocketAddr {
        start_with_sp(clock, content, 0)
    }

    /// [`start_with`] under the world rule `start_sp` (WS-3).
    fn start_with_sp(
        clock: eve_server::game_clock::GameClock,
        content: eve_server::content::Content,
        start_sp: i64,
    ) -> SocketAddr {
        let pilot = |id, name: &str, ship| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            balance: WALLET,
            race_id: RACE,
            ..Character::default()
        };
        let mut config = eve_server::server::Config::new("127.0.0.1:0".parse().expect("literal"));
        config.sandbox.characters = vec![
            pilot(MINER, "Miner1", 100_000_001),
            pilot(ESCORT, "Escort1", 100_000_002),
            pilot(ALICE, "Alice", 100_000_003),
        ];
        config.sandbox.rules.start_sp = start_sp;
        if start_sp > 0 {
            // Only a fresh roster pilot is funded, and so granted its points.
            config.sandbox.fresh_roster = vec![(MINER, None)];
        }
        config.content = Some(Arc::new(content));
        config.time_scale = clock;
        let server = eve_server::server::Server::bind(config).expect("bind");
        let addr = server.local_addr().expect("local addr");
        std::thread::spawn(move || {
            let _ = server.run();
        });
        addr
    }

    fn login(addr: SocketAddr, name: &str) -> Conn {
        let config = BotConfig::new(name, addr, Pace::Fast);
        let mut c = Conn::connect(addr, Duration::from_secs(10)).expect("connects");
        c.handshake_as(&config.token).expect("handshake");
        c.select_character_named(name).expect("selects by name");
        c
    }

    /// A human stand-in's setup: a GM slash, named at the call site.
    fn gm(c: &mut Conn, line: &str) {
        c.call(
            "slash",
            "SlashCmd",
            vec![PyValue::Bytes(line.as_bytes().to_vec())],
        )
        .expect("the slash is answered");
    }

    /// Alice lists `quantity` of `type_id` at the home station at `price`,
    /// a hull repackaged first, as the market wants
    /// (`RepackageBeforeSelling`).
    fn ask(c: &mut Conn, type_id: i64, quantity: i64, price: f64) {
        gm(c, &format!("/createitem {type_id} {quantity}"));
        let station = c.session().station_id().expect("docked");
        let hulls: Vec<PyValue> = c
            .hangar_items()
            .expect("the hangar lists")
            .iter()
            .filter(|r| r.type_id == type_id && r.category_id == CATEGORY_SHIP)
            .map(|r| PyValue::Tuple(vec![PyValue::Int(r.item_id), PyValue::Int(station)]))
            .collect();
        if !hulls.is_empty() {
            let batch = PyValue::Dict(vec![(PyValue::List(hulls), PyValue::Int(station))]);
            let done = c
                .call("repackagingSvc", "RepackageItems", vec![batch])
                .expect("answered");
            assert_eq!(done.refusal(), None, "repackaged");
        }
        let rows: Vec<_> = c
            .hangar_items()
            .expect("the hangar lists")
            .into_iter()
            .filter(|r| r.type_id == type_id)
            .collect();
        let lines: Vec<SellLine> = rows
            .iter()
            .map(|r| SellLine {
                station_id: station,
                type_id,
                item_id: r.item_id,
                price,
                quantity: r.quantity,
            })
            .collect();
        c.sell(&lines, crate::builder::ORDER_DAYS, BROKER_FEE)
            .expect("Alice lists");
    }

    /// A closed-economy bot (no GM setup) as `name` in `role`, run until
    /// `done` says so; its handle, stopped.
    fn drive(
        addr: SocketAddr,
        name: &str,
        role: RoleRef,
        done: impl Fn(&Handle) -> bool,
    ) -> Handle {
        drive_with(addr, name, role, None, done)
    }

    /// [`drive`], the bot reading `content` (the skill plan needs the
    /// skill table).
    fn drive_with(
        addr: SocketAddr,
        name: &str,
        role: RoleRef,
        content: Option<Arc<eve_server::content::Content>>,
        done: impl Fn(&Handle) -> bool,
    ) -> Handle {
        let mut config = BotConfig::new(name, addr, Pace::Fast).with_role(role);
        config.chat = false;
        config.content = content;
        config.policy = Policy {
            role,
            gm_setup: false,
            closed: true,
            ..Policy::fast()
        };
        let (handle, thread) = crate::driver::spawn(config);
        let deadline = Instant::now() + WITHIN;
        while !done(&handle) {
            assert!(
                Instant::now() < deadline,
                "{name} never got there:\n{}",
                dump(&handle)
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        handle.stop();
        let _ = thread.join();
        assert!(
            !handle
                .log()
                .iter()
                .any(|e| e.kind == ActKind::Slash || e.kind == ActKind::GmSetup),
            "a GM slash from a closed-economy bot:\n{}",
            dump(&handle)
        );
        handle
    }

    fn dump(h: &Handle) -> String {
        h.log()
            .iter()
            .map(|e| format!("  {:?} {} {:?}", e.kind, e.what, e.verdict))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn flying(h: &Handle) -> Option<i64> {
        h.observation().and_then(|o| o.ship).and_then(|s| s.type_id)
    }

    fn acquired(h: &Handle) -> bool {
        h.log()
            .iter()
            .any(|e| e.kind == ActKind::Acquire && e.verdict.is_ok())
    }

    /// **E8**: a bot in its capsule with no hull ask gets the race's rookie
    /// ship through `CreateNewbieShip`, not GM, and flies it.
    #[test]
    fn a_capsule_bot_gets_a_rookie_ship_through_create_newbie_ship_not_gm() {
        let addr = start();
        let h = drive(addr, "Miner1", &crate::roles::MINE, |h| {
            acquired(h) && flying(h) == Some(HULL)
        });
        let line = h
            .log()
            .into_iter()
            .find(|e| e.kind == ActKind::Acquire)
            .expect("the act");
        assert!(line.what.contains("rookie ship created"), "{line:?}");
    }

    /// **E8**: a bot with ISK buys its role's skillbook from the fixture
    /// school's ask at its station, injects it and queues it.
    #[test]
    fn a_bot_with_isk_buys_a_skillbook_at_the_school_injects_it_and_queues_it() {
        let addr = start();
        let skill = crate::roles::SKILL_MINING;
        {
            let mut c = login(addr, "Miner1");
            let handler = skill_handler(&mut c).expect("the handler");
            assert!(
                !known_skills(&mut c, &handler)
                    .expect("skills")
                    .contains_key(&skill),
                "the fixture pilot starts without it"
            );
        }
        drive(addr, "Miner1", &crate::roles::MINE, acquired);

        let mut c = login(addr, "Miner1");
        let handler = skill_handler(&mut c).expect("the handler");
        assert!(
            known_skills(&mut c, &handler)
                .expect("skills")
                .contains_key(&skill),
            "injected"
        );
        assert!(
            queue_of(&mut c, &handler)
                .expect("the queue")
                .contains(&(skill, 1)),
            "queued"
        );
        assert!(
            !c.hangar_items()
                .expect("the hangar lists")
                .iter()
                .any(|r| r.type_id == skill),
            "the book was used"
        );
        assert!(c.cash_balance().expect("a wallet") < WALLET, "it paid");
    }

    /// **A kit skill with a prerequisite** (the dev roamers' bug): the
    /// server refuses a level queued before the skills it needs, so
    /// `learn` puts the prerequisite's book, injection and levels in
    /// front. Invented rows: the gunnery book requires the mining book.
    #[test]
    fn a_kit_skill_is_queued_after_its_prerequisites() {
        let addr = start();
        let (need, skill) = (crate::roles::SKILL_MINING, crate::escort::SKILL_GUNNERY);
        let mut c = content();
        let rows = format!(
            "skill\t{need}\t990020100\t1\t165\t166\t0.01\t\tFixture Prerequisite\n\
             skill\t{skill}\t990020100\t1\t165\t166\t0.01\t{need}=1\tFixture Dependent\n"
        );
        c.insert(
            eve_server::skill::TABLE.name,
            eve_server::skill::Table::parse(&rows).expect("the skill rows parse"),
        );
        use_content(&Arc::new(c));
        let mut conn = login(addr, "Miner1");
        let got = learn_skills(&mut conn, &[(skill, 1)], None).expect("learns");
        assert!(got.missing.is_empty(), "{:?}", got.missing);
        let handler = skill_handler(&mut conn).expect("the handler");
        assert_eq!(
            queue_of(&mut conn, &handler).expect("the queue"),
            vec![(need, 1), (skill, 1)],
            "the prerequisite first"
        );
    }

    /// **WS-3**: a pilot with a pool and a bought book applies the pool to
    /// its queue -- the level rises, the pool falls, no GM call.
    #[test]
    fn free_points_raise_a_bought_skill_and_empty_the_pool() {
        let pool = 500_000;
        let addr = start_with_sp(eve_server::game_clock::GameClock::REAL, content(), pool);
        let skill = crate::roles::SKILL_MINING;
        let mut c = content();
        let rows = format!("skill\t{skill}\t990020100\t1\t165\t166\t0.01\t\tFixture Skill\n");
        c.insert(
            eve_server::skill::TABLE.name,
            eve_server::skill::Table::parse(&rows).expect("the skill rows parse"),
        );
        use_content(&Arc::new(c));
        let mut conn = login(addr, "Miner1");
        assert_eq!(free_points(&mut conn).expect("the pool"), pool);
        let got = learn_skills(&mut conn, &[(skill, 1)], None).expect("learns");
        assert!(got.missing.is_empty(), "{:?}", got.missing);
        let spent = apply_free_to_queue(&mut conn).expect("applies");
        assert!(spent > 0, "something was spent");
        assert_eq!(
            free_points(&mut conn).expect("the pool"),
            pool - spent,
            "the pool fell by what was spent"
        );
        let levels = skill_levels(&mut conn).expect("the skills");
        assert!(levels.get(&skill).is_some_and(|&l| l >= 1), "{levels:?}");
        // Nothing is left to spend on: a second apply spends nothing.
        assert_eq!(apply_free_to_queue(&mut conn).expect("applies"), 0);
    }

    /// **WS-3**: a pool and an empty queue (no book bought) stays banked.
    #[test]
    fn free_points_without_a_queue_stay_banked() {
        let pool = 500_000;
        let addr = start_with_sp(eve_server::game_clock::GameClock::REAL, content(), pool);
        let mut conn = login(addr, "Miner1");
        assert_eq!(apply_free_to_queue(&mut conn).expect("applies"), 0);
        assert_eq!(free_points(&mut conn).expect("the pool"), pool);
        assert!(skill_queue(&mut conn).expect("the queue").is_empty());
    }

    /// **X-SKILL**: the miner's plan buys the book from the school's ask,
    /// injects it, queues levels beyond the kit's one, and the game clock at
    /// scale 60 trains them -- no GM.
    #[test]
    fn the_skill_plan_queues_levels_and_the_game_clock_trains_them() {
        let clock = eve_server::game_clock::GameClock::new(60).expect("a legal scale");
        let addr = start_at(clock);
        let skill = crate::roles::SKILL_MINING;
        let content = Arc::new(content());
        {
            // A human stand-in's setup: the pilot's fitting skills are
            // already trained, so the plan's next rows are the miner's.
            let mut c = login(addr, "Miner1");
            for &(name, _) in crate::skill_plan::CORE_PLAN {
                if let Some(s) = eve_server::skill::find_any_in(&content, name) {
                    gm(&mut c, &format!("/giveskill {} 5", s.type_id));
                }
            }
        }
        let content = Some(content);
        let h = drive_with(addr, "Miner1", &crate::roles::MINE, content, |h| {
            h.log()
                .iter()
                .any(|e| e.kind == ActKind::Acquire && e.what.starts_with("skill plan: targets"))
        });
        let line = h
            .log()
            .into_iter()
            .find(|e| e.what.starts_with("skill plan: targets"))
            .expect("the plan's act");
        assert!(line.what.contains(&skill.to_string()), "{line:?}");
        let mut c = login(addr, "Miner1");
        let handler = skill_handler(&mut c).expect("the handler");
        let queue = queue_of(&mut c, &handler).expect("the queue");
        assert!(
            queue.contains(&(skill, 1)) && queue.contains(&(skill, 2)),
            "{queue:?}"
        );
        assert!(c.cash_balance().expect("a wallet") < WALLET, "it paid");
        // Two levels at scale 60 (rank 1: about a minute of wall time).
        let deadline = Instant::now() + Duration::from_secs(150);
        loop {
            let level = known_skills(&mut c, &handler)
                .expect("skills")
                .get(&skill)
                .copied()
                .unwrap_or(0);
            if level >= 2 {
                break;
            }
            assert!(Instant::now() < deadline, "level {level} after 150 s");
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    /// **E8**: with a hull, guns and rounds on the market at its station,
    /// the escort buys them, boards, fits and loads -- no GM, no rookie
    /// ship.
    #[test]
    fn a_bot_buys_a_hull_module_and_ammo_from_asks_and_fits_them() {
        use crate::escort::{AMMO, GUN, RIFTER, SETUP_ROUNDS, WANT_GUNS};
        let addr = start();
        {
            let mut alice = login(addr, "Alice");
            ask(&mut alice, RIFTER, 1, HULL_ASK);
            ask(&mut alice, GUN, WANT_GUNS as i64, GUN_ASK);
            ask(&mut alice, AMMO, SETUP_ROUNDS, ROUND_ASK);
        }
        let h = drive(addr, "Escort1", &crate::roles::ESCORT, |h| {
            h.observation().is_some_and(|o| {
                o.ship.and_then(|s| s.type_id) == Some(RIFTER)
                    && o.guns.len() == WANT_GUNS
                    && o.guns.iter().all(|g| g.online && g.rounds > Some(0))
            })
        });
        // The hull came through the chooser (ship-pool), the rest through
        // the kit; neither made a rookie ship.
        for line in h
            .log()
            .into_iter()
            .filter(|e| matches!(e.kind, ActKind::Acquire | ActKind::Drone))
        {
            assert!(!line.what.contains("rookie ship created"), "{line:?}");
        }
        assert!(
            h.log().iter().any(|e| e.kind == ActKind::Drone),
            "the chooser's act"
        );
    }

    /// **PROG-PVE, the hull upgrade over the socket**: an escort flying a
    /// Rifter with the cruiser skill and the medium gun skill trained, and a
    /// Rupture, three medium guns and medium rounds on the market, buys them
    /// (the chooser over the classes above the frigate, then the class's
    /// fit), boards the cruiser (a bigger class always is the step up), fits
    /// the medium guns and loads the medium rounds. Without the cruiser
    /// skill it stays in the Rifter. No GM from the bot; the skills are a
    /// human stand-in's.
    #[test]
    fn the_dev_sellers_ask_is_not_held_to_the_reservation_cap() {
        let at = |id, price| BookOrder {
            order_id: id,
            type_id: 1,
            station_id: 5,
            price,
            vol_remaining: 1_000,
            bid: false,
        };
        let book = Book {
            asks: vec![at(7, 100.0), at(exchange::DEV_ORDER_BASE + 7, 100.0)],
            bids: vec![],
        };
        let got = plan(&book, 5, Some(66.0), 1.0e9, 1_000);
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(exchange::is_dev(got[0].0.order_id));
        assert_eq!(got[0].1, 1_000);
    }

    #[test]
    fn a_trained_escort_with_the_isk_buys_a_cruiser_arms_it_and_boards_it() {
        use crate::escort::{AMMO, GUN, RIFTER, SETUP_ROUNDS, WANT_GUNS};
        use crate::fit_plan::{MEDIUM_GUN, MEDIUM_ROUND, SKILL_MEDIUM, fit_of};
        use crate::hull_choice::Class;
        use crate::progression::Race;
        let rupture = 629;
        let cruiser_guns = fit_of(Class::Cruiser).guns;
        let market = |addr| {
            let mut alice = login(addr, "Alice");
            ask(&mut alice, rupture, 1, HULL_ASK);
            ask(&mut alice, MEDIUM_GUN, cruiser_guns as i64, GUN_ASK);
            ask(&mut alice, MEDIUM_ROUND, SETUP_ROUNDS, ROUND_ASK);
        };
        let setup = |addr, cruiser_skill: bool| {
            let mut c = login(addr, "Escort1");
            gm(&mut c, &format!("/createitem {RIFTER}"));
            gm(&mut c, &format!("/createitem {GUN} {WANT_GUNS}"));
            gm(&mut c, &format!("/createitem {AMMO} {SETUP_ROUNDS}"));
            gm(
                &mut c,
                &format!("/giveskill {} 5", crate::escort::SKILL_GUNNERY),
            );
            gm(&mut c, &format!("/giveskill {SKILL_MEDIUM} 1"));
            if cruiser_skill {
                let skill = Race::Minmatar.hull_skill(Class::Cruiser);
                gm(&mut c, &format!("/giveskill {skill} 1"));
            }
        };

        // Untrained: the Rifter, and the Rupture stays on the book.
        let addr = start();
        market(addr);
        setup(addr, false);
        let known = Some(Arc::new(content()));
        let h = drive_with(addr, "Escort1", &crate::roles::ESCORT, known, |h| {
            h.log().iter().any(|e| e.what.starts_with("hull upgrade:"))
        });
        assert_eq!(flying(&h), Some(RIFTER), "no cruiser skill, no cruiser");
        assert!(
            dump(&h).contains("no hull above"),
            "the bot says why:\n{}",
            dump(&h)
        );

        // Trained: it buys, arms, boards, fits and loads.
        let addr = start();
        market(addr);
        setup(addr, true);
        // The bot reads the bundle's numbers (a medium gun is unknown to the
        // invented no-content table).
        let known = Some(Arc::new(content()));
        let h = drive_with(addr, "Escort1", &crate::roles::ESCORT, known, |h| {
            h.observation().is_some_and(|o| {
                o.ship.and_then(|s| s.type_id) == Some(rupture)
                    && o.guns.len() == cruiser_guns
                    && o.guns.iter().all(|g| g.type_id == MEDIUM_GUN && g.online)
                    && o.guns.iter().all(|g| g.rounds > Some(0))
            })
        });
        assert!(
            h.log()
                .iter()
                .any(|e| e.what.contains("hull upgrade: bought 629")),
            "{}",
            dump(&h)
        );
    }

    /// **PROG-PVE2, the other weapon systems over the socket**: an escort that
    /// buys an Amarr frigate fits **lasers** and loads **crystals**, one that
    /// buys a Gallente frigate fits **hybrid guns** and loads **hybrid
    /// charges** -- the race of the hull sets the weapon, the kit buys the
    /// guns and rounds of that system and no projectile ones. No GM from the
    /// bot; the gun skills are a human stand-in's.
    #[test]
    fn a_laser_hull_buys_lasers_and_crystals_and_a_hybrid_hull_hybrids() {
        use crate::escort::SETUP_ROUNDS;
        use crate::fit_plan::{Size, fit_for};
        use crate::hull_choice::{Class, Weapon};
        for (hull, weapon, skill) in [
            (PUNISHER, Weapon::Laser, 3303),
            (ATRON, Weapon::Hybrid, 3301),
        ] {
            let fit = fit_for(Class::Frigate, weapon);
            assert_eq!(fit.size, Size::Small);
            let addr = start();
            {
                let mut alice = login(addr, "Alice");
                ask(&mut alice, hull, 1, HULL_ASK);
                ask(&mut alice, fit.gun, fit.guns as i64, GUN_ASK);
                ask(&mut alice, fit.round, SETUP_ROUNDS, ROUND_ASK);
                // The projectile kit is on the market too; the bot must not
                // buy it.
                ask(&mut alice, crate::escort::GUN, 2, GUN_ASK);
                ask(&mut alice, crate::escort::AMMO, SETUP_ROUNDS, ROUND_ASK);
            }
            {
                let mut c = login(addr, "Escort1");
                gm(&mut c, &format!("/giveskill {skill} 5"));
            }
            let known = Some(Arc::new(content()));
            let h = drive_with(addr, "Escort1", &crate::roles::ESCORT, known, |h| {
                h.observation().is_some_and(|o| {
                    o.ship.and_then(|s| s.type_id) == Some(hull)
                        && o.guns.len() == fit.guns
                        && o.guns.iter().all(|g| g.type_id == fit.gun && g.online)
                        && o.guns.iter().all(|g| g.rounds > Some(0))
                })
            });
            // The projectile goods stand on the market untouched.
            let mut alice = login(addr, "Alice");
            let mine = alice.my_orders().expect("Alice's orders read");
            let left = |t: i64| {
                mine.iter()
                    .find(|o| o.type_id == t)
                    .map(|o| o.vol_remaining)
            };
            assert_eq!(
                left(crate::escort::GUN),
                Some(2),
                "{weapon:?}\n{}",
                dump(&h)
            );
            assert_eq!(
                left(crate::escort::AMMO),
                Some(SETUP_ROUNDS),
                "{weapon:?}\n{}",
                dump(&h)
            );
            // And the weapon's own goods were bought.
            assert_eq!(left(fit.gun), None, "{weapon:?}: the guns were taken");
        }
    }

    /// **PROG-PVE2, the old hull sold**: an escort that has climbed into a
    /// cruiser and holds two old Rifters keeps one as its spare and lists the
    /// other on the market -- repackaged first, then an ask at the
    /// reference price rule -- so the ISK is not left in the hangar.
    #[test]
    fn a_climbed_escort_keeps_one_spare_hull_and_lists_the_other_for_sale() {
        use crate::escort::{AMMO, GUN, RIFTER, SETUP_ROUNDS};
        use crate::fit_plan::{MEDIUM_GUN, MEDIUM_ROUND, SKILL_MEDIUM, fit_of};
        use crate::hull_choice::Class;
        let rupture = 629;
        let addr = start();
        {
            let mut c = login(addr, "Escort1");
            gm(&mut c, &format!("/createitem {rupture}"));
            gm(&mut c, &format!("/createitem {RIFTER} 2"));
            gm(
                &mut c,
                &format!("/createitem {MEDIUM_GUN} {}", fit_of(Class::Cruiser).guns),
            );
            gm(
                &mut c,
                &format!("/createitem {MEDIUM_ROUND} {SETUP_ROUNDS}"),
            );
            gm(&mut c, &format!("/createitem {GUN} 2"));
            gm(&mut c, &format!("/createitem {AMMO} {SETUP_ROUNDS}"));
            gm(&mut c, &format!("/giveskill {SKILL_MEDIUM} 5"));
            gm(
                &mut c,
                &format!("/giveskill {} 5", crate::escort::SKILL_GUNNERY),
            );
        }
        let known = Some(Arc::new(content()));
        let h = drive_with(addr, "Escort1", &crate::roles::ESCORT, known, |h| {
            h.log()
                .iter()
                .any(|e| e.kind == ActKind::Drone && e.what.contains("spare hulls: repackaged"))
        });
        assert_eq!(flying(&h), Some(rupture), "{}", dump(&h));
        let mut c = login(addr, "Escort1");
        let mine = c.my_orders().expect("my orders read");
        let ask = mine
            .iter()
            .find(|o| o.type_id == RIFTER && !o.bid)
            .unwrap_or_else(|| panic!("no Rifter ask in {mine:?}\n{}", dump(&h)));
        assert_eq!(ask.vol_remaining, 1, "one hull listed");
        // One of the two went, the other is the spare: the act says so (the
        // bot is in space again by now, its hangar out of reach).
        assert!(
            h.log()
                .iter()
                .any(|e| e.what.contains("spare hulls: repackaged 1, listed 1")),
            "{}",
            dump(&h)
        );
    }

    /// **PROG-PVE2, the tank**: an Amarr escort (an armour race) holding a
    /// plate and a membrane fits both into its low slots as it undocks, within
    /// the hull's slots and power, and onlines them.
    #[test]
    fn an_armour_race_escort_fits_the_plate_and_membrane_it_holds_as_it_undocks() {
        use crate::fit_plan::{Size, fit_for};
        use crate::hull_choice::{Class, Weapon};
        let fit = fit_for(Class::Frigate, Weapon::Laser);
        assert_eq!(fit.size, Size::Small);
        let addr = start();
        {
            let mut c = login(addr, "Escort1");
            gm(&mut c, &format!("/createitem {PUNISHER}"));
            gm(&mut c, &format!("/createitem {} {}", fit.gun, fit.guns));
            gm(&mut c, &format!("/createitem {} 1000", fit.round));
            gm(&mut c, &format!("/createitem {PLATE}"));
            gm(&mut c, &format!("/createitem {MEMBRANE}"));
            gm(&mut c, &format!("/giveskill {} 5", fit.skill));
            gm(&mut c, "/giveskill 3394 5");
        }
        let known = Some(Arc::new(content()));
        let h = drive_with(addr, "Escort1", &crate::roles::ESCORT, known, |h| {
            h.log()
                .iter()
                .any(|e| e.kind == ActKind::Undock && e.verdict.is_ok())
        });
        let mut c = login(addr, "Escort1");
        let fitted = c.fitted().expect("the ship lists");
        let types: Vec<i64> = fitted.iter().map(|r| r.type_id).collect();
        assert!(
            types.contains(&PLATE) && types.contains(&MEMBRANE),
            "{types:?}\n{}",
            dump(&h)
        );
        // In the low racks, and online.
        for r in fitted
            .iter()
            .filter(|r| [PLATE, MEMBRANE].contains(&r.type_id))
        {
            assert!(crate::tank::LOW_FLAGS.contains(&r.flag_id), "{r:?}");
            assert!(c.is_online(r.item_id).expect("online read"), "{r:?}");
        }
    }

    /// **E8**: with no hull ask, the escort keeps its rookie ship; the chooser
    /// found no hull on offer (ship-pool), and no GM made one.
    #[test]
    fn with_no_ask_it_keeps_its_rookie_ship_and_says_missing() {
        let addr = start();
        let h = drive(addr, "Escort1", &crate::roles::ESCORT, |h| {
            let log = h.log();
            log.iter().any(|e| e.kind == ActKind::Drone) && flying(h) == Some(HULL)
        });
        assert_eq!(flying(&h), Some(HULL), "still the rookie ship");
    }

    /// The plan (pure): this station's asks only, cheapest first, never
    /// above the cap, inside the wallet.
    #[test]
    fn the_plan_buys_cheapest_here_under_the_cap_inside_the_wallet() {
        const HERE: i64 = 60_000_001;
        const THERE: i64 = 60_000_002;
        let o = |id, station, price, vol| BookOrder {
            order_id: id,
            type_id: HULL,
            station_id: station,
            price,
            vol_remaining: vol,
            bid: false,
        };
        let book = Book {
            asks: vec![
                o(1, HERE, 30.0, 5),
                o(2, THERE, 1.0, 50),
                o(3, HERE, 10.0, 2),
                o(4, HERE, 99.0, 50),
            ],
            bids: vec![],
        };
        let p = plan(&book, HERE, Some(50.0), 1_000.0, 4);
        let got: Vec<(i64, i64)> = p.iter().map(|(a, n)| (a.order_id, *n)).collect();
        assert_eq!(got, [(3, 2), (1, 2)]);
        let poor = plan(&book, HERE, None, 25.0, 4);
        let got: Vec<(i64, i64)> = poor.iter().map(|(a, n)| (a.order_id, *n)).collect();
        assert_eq!(got, [(3, 2)], "the wallet stops it");
        assert!(plan(&book, HERE, Some(5.0), 1_000.0, 4).is_empty());
    }

    /// Every role row has a kit; only the builder's and the reactor's (both
    /// stay docked) and CAP-L's three (a capital pilot, its alt and a
    /// freighter hauler own their hulls or buy them themselves, never a kit)
    /// are empty.
    #[test]
    fn every_role_has_a_kit_and_the_builder_buys_nothing() {
        for r in crate::roles::ROLES {
            let k = kit_of(r);
            assert_eq!(
                k == Kit::default(),
                std::ptr::eq(*r, &crate::builder::ROLE)
                    || std::ptr::eq(*r, &crate::reactor::ROLE)
                    || std::ptr::eq(*r, &crate::capital::ROLE)
                    || std::ptr::eq(*r, &crate::capital::ALT_ROLE)
                    || std::ptr::eq(*r, &crate::hauler::ROLE),
                "{}",
                r.name
            );
        }
        assert_eq!(kit_of(&crate::roles::MINE), MINER_KIT);
    }

    // ---- DOC-2: the owned order --------------------------------------------

    /// Invented ids for the order tests (never a client value).
    const O_HULL: i64 = 990_950_001;
    const O_GUN: i64 = 990_950_002;
    const O_PLATE: i64 = 990_950_003;
    const O_ROUND: i64 = 990_950_004;
    const O_ALT: i64 = 990_950_005;
    const O_SKILL: i64 = 990_950_006;

    fn fit_of_ids() -> crate::doctrine::RoleFit {
        crate::doctrine::RoleFit {
            hull: O_HULL,
            modules: vec![(O_GUN, 3), (O_PLATE, 2)],
            charges: vec![(O_ROUND, 60), (O_ALT, 20)],
            skills: vec![(O_SKILL, 4)],
            profile: crate::doctrine::testkit::brawler(crate::doctrine::testkit::flat_tank(
                1.0, 1.0, 1.0,
            )),
            cost: 1.0,
        }
    }

    #[test]
    fn an_order_lists_every_part_once() {
        let o = Order::from_fit(&fit_of_ids());
        assert_eq!(o.hull, Some(O_HULL));
        assert_eq!(o.modules, vec![(O_GUN, 3), (O_PLATE, 2)]);
        assert_eq!(o.charges, vec![(O_ROUND, 60), (O_ALT, 20)]);
        assert_eq!(o.skills, vec![(O_SKILL, 4)]);
        // Wanting a part again keeps one row, at the larger count; a type
        // ordered as a round stays a round; a skill keeps the higher level.
        let mut o = o;
        o.want_module(O_GUN, 2)
            .want_module(O_GUN, 5)
            .want_module(O_ROUND, 100)
            .want_charge(O_PLATE, 1)
            .want_skill(O_SKILL, 2)
            .want_skill(O_SKILL, 5)
            .want_module(O_ALT, 0);
        assert_eq!(o.modules, vec![(O_GUN, 5), (O_PLATE, 2)]);
        assert_eq!(o.charges, vec![(O_ROUND, 100), (O_ALT, 20)]);
        assert_eq!(o.skills, vec![(O_SKILL, 5)]);
        let all: Vec<i64> = o.modules.iter().chain(&o.charges).map(|p| p.0).collect();
        let mut once = all.clone();
        once.sort_unstable();
        once.dedup();
        assert_eq!(all.len(), once.len(), "{all:?}");
        assert!(o.names(O_HULL) && o.names(O_ALT) && !o.names(1));
        // A kit becomes an order with its module, its `more`, its rounds and
        // its skills, a type shared by two of them once.
        let k = Order::from_kit(&FIGHTER_KIT);
        assert_eq!(k.hull, FIGHTER_KIT.hull);
        assert_eq!(k.modules.len(), 1);
        assert_eq!(k.charges.len(), 1);
        let m = Order::from_kit(&MINER_KIT);
        assert!(m.modules.contains(&(crate::ore::MINING_DRONE_I, 1)));
    }

    #[test]
    fn a_bid_has_units_for_the_missing_count() {
        let o = Order::from_fit(&fit_of_ids());
        let held = |t: i64| match t {
            O_GUN => 1,
            O_PLATE => 2,
            O_ROUND => 61,
            _ => 0,
        };
        // Three guns, one held: two missing. Both plates held. Rounds held
        // over the count. The alternate round, none held.
        assert_eq!(o.short(&held), vec![(O_GUN, 2), (O_ALT, 20)]);
        assert!(o.short(&|_| 1_000).is_empty());
    }
    // ---- SKILLGATE (invented ids; the requirement attributes are the dogma's) ----

    const GATED_HULL: i64 = 98_001;
    const GATED_GUN: i64 = 98_002;
    const LOW_SKILL: i64 = 98_101;
    const HIGH_SKILL: i64 = 98_102;

    /// A content whose hull asks for `LOW_SKILL` 2 in the first pair and
    /// `HIGH_SKILL` 3 in the sixth, and whose gun asks for `LOW_SKILL` 1.
    fn gated_content() -> Arc<eve_server::content::Content> {
        let rows = format!(
            "type\t{GATED_HULL}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
             182={LOW_SKILL};277=2;1290={HIGH_SKILL};1288=3\tGated Hull\n\
             type\t{GATED_GUN}\t55\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t34\t12\t\
             182={LOW_SKILL};277=1\tGated Gun\n"
        );
        let table =
            eve_server::typedata::Table::parse(&format!("{}{rows}", eve_server::typedata::FIXTURE))
                .expect("the gated rows parse");
        let mut c = eve_server::content::Content::fixture();
        c.insert("typedata", eve_server::typedata::Installed::build(table));
        Arc::new(c)
    }

    #[test]
    fn flyable_reads_every_requirement() {
        // No content: nothing is known, everything flies.
        assert!(flyable(&BTreeMap::new(), GATED_HULL));
        use_content(&gated_content());
        assert_eq!(
            direct_requirements(GATED_HULL),
            vec![(LOW_SKILL, 2), (HIGH_SKILL, 3)]
        );
        let mut levels = BTreeMap::from([(LOW_SKILL, 5)]);
        assert!(!flyable(&levels, GATED_HULL), "the sixth pair counts");
        levels.insert(HIGH_SKILL, 2);
        assert_eq!(lacking_of(&levels, GATED_HULL), vec![(HIGH_SKILL, 3)]);
        levels.insert(HIGH_SKILL, 3);
        assert!(flyable(&levels, GATED_HULL));
        assert!(
            flyable(&BTreeMap::new(), 98_999),
            "an unknown type has none"
        );
    }

    #[test]
    fn an_orders_requirements_merge_to_the_larger_level() {
        use_content(&gated_content());
        let mut o = Order::hull(GATED_HULL);
        o.want_module(GATED_GUN, 2);
        let all = Lacking {
            ship: true,
            module: true,
            ammo: true,
            in_capsule: false,
        };
        assert_eq!(requirements(&o, all), vec![(LOW_SKILL, 2), (HIGH_SKILL, 3)]);
        // Only what is bought counts: a fighter needing rounds alone queues
        // nothing for the hull it is not buying.
        let rounds_only = Lacking {
            ship: false,
            module: false,
            ammo: true,
            ..all
        };
        assert_eq!(requirements(&o, rounds_only), Vec::new());
        let gun_only = Lacking {
            ship: false,
            ammo: false,
            ..all
        };
        assert_eq!(requirements(&o, gun_only), vec![(LOW_SKILL, 1)]);
    }

    /// **SKILLGATE B4** (driver `Act::Board`, over a real server): the
    /// hangar corvette asks for a skill the pilot lacks. The bot does not
    /// send the Board: the lack is queued first, the note says so, and no
    /// Board failure or refusal is logged (nothing is banned).
    #[test]
    fn a_board_of_an_unskilled_hull_waits_and_queues_the_skill() {
        let skill = crate::escort::SKILL_GUNNERY;
        let addr = start_with(
            eve_server::game_clock::GameClock::REAL,
            content_gated(Some(skill)),
        );
        let known = Some(Arc::new(content_gated(Some(skill))));
        // A human stand-in's setup: a Merlin in the hangar of a pilot still in
        // its capsule; the miner boards the hull it prefers.
        gm(
            &mut login(addr, "Miner1"),
            &format!("/createitem {}", brain::MERLIN),
        );
        let h = drive_with(addr, "Miner1", &crate::roles::MINE, known, |h| {
            h.log().iter().any(|e| e.what.contains("lacks skills"))
        });
        // The Board act ends with the stop that interrupts its wait (a
        // closed connection); a refusal by the server never happened.
        assert!(
            !h.log()
                .iter()
                .any(|e| e.kind == ActKind::Board && matches!(e.verdict, Verdict::Refused(_))),
            "no Board refusal:\n{}",
            dump(&h)
        );
        let mut c = login(addr, "Miner1");
        let handler = skill_handler(&mut c).expect("the handler");
        assert!(
            queue_of(&mut c, &handler)
                .expect("the queue")
                .contains(&(skill, 1)),
            "the hull's skill is queued"
        );
    }

    #[test]
    fn only_the_online_skill_key_is_an_online_skill_refusal() {
        let key = |k: &str| Error::Refused(format!("SetModuleOnline(1, 2): still offline ({k})"));
        assert!(is_online_skill_refusal(&key("OnlineHasSkillPrerequisites")));
        assert!(!is_online_skill_refusal(&key("NotEnoughCpu")));
    }

    #[test]
    fn only_the_fit_skill_key_is_a_skill_refusal() {
        let key = |k: &str| {
            Error::Refused(format!(
                "MultiAdd([1], flag=0): nothing new is fitted ({k})"
            ))
        };
        assert!(is_skill_refusal(&key("FittingHasSkillPrerequisites")));
        assert!(!is_skill_refusal(&key("NotEnoughCpu")));
        assert!(!is_skill_refusal(&Error::NotSent(
            "FittingHasSkillPrerequisites".to_owned()
        )));
    }

    /// A kit's hull skill goes in front of what is queued already.
    #[test]
    fn a_kits_hull_skill_goes_first_in_the_queue() {
        let addr = start();
        let (filler, urgent) = (crate::roles::SKILL_MINING, crate::escort::SKILL_GUNNERY);
        let mut c = content();
        let rows = format!(
            "skill\t{filler}\t990020100\t1\t165\t166\t0.01\t\tFixture Filler\n\
             skill\t{urgent}\t990020100\t1\t165\t166\t0.01\t\tFixture Urgent\n"
        );
        c.insert(
            eve_server::skill::TABLE.name,
            eve_server::skill::Table::parse(&rows).expect("the skill rows parse"),
        );
        use_content(&Arc::new(c));
        let mut conn = login(addr, "Miner1");
        learn_skills(&mut conn, &[(filler, 1)], None).expect("learns the filler");
        let got = learn_urgent(&mut conn, &[(urgent, 1)], None).expect("learns the urgent one");
        assert!(got.missing.is_empty(), "{:?}", got.missing);
        let handler = skill_handler(&mut conn).expect("the handler");
        assert_eq!(
            queue_of(&mut conn, &handler).expect("the queue"),
            vec![(urgent, 1), (filler, 1)],
            "the hull's skill first"
        );
    }
}
