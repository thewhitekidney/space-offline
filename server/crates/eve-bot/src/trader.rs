//! **The trader and hauler** (R-SOCIETY §1 row 12, §6 SOC-B8): a bot that
//! earns by moving goods, the way a player with a hold and a map does.
//!
//! - **Regional arbitrage** ([`best_deal`], pure): for each type it probes,
//!   the asks and bids the region's book shows (`GetOrders` answers by
//!   region), grouped by station. A deal buys at station S and sells at
//!   station T where T's bid, after the sales tax, beats S's ask; or it
//!   brings the goods home and lists them at `P_mkt` when nothing at home
//!   asks less ([`Outlet::List`], after tax and the broker's fee). Each
//!   jump of the whole trip -- here to S, S to T, T home -- costs
//!   [`ISK_PER_JUMP`]; a deal must clear [`MIN_PROFIT`] and
//!   [`TRADE_MARGIN`] of what it spends. Units are bounded by the hold
//!   (the hull's capacity over the type's volume), by [`WALLET_SHARE`] of
//!   the wallet, and by the volume on offer -- so an NPC side is **capped
//!   by its stock** (`vol_remaining`, E5b's `TIER_A_STOCK`).
//! - **The NPC trade-goods loop** (E5b, O5-i): the probed types are the
//!   bundle's trade goods ([`trade_goods`], [`TRADE_GOOD_GROUPS`]), which
//!   NPC corporations ask at `TRADE_GOOD_ASK` x base and bid at
//!   `TIER_A_BID` x base at their own stations. At the client's 8 % sales
//!   tax and Accounting 0 the loop **loses** (0.92 < 0.94); it pays only
//!   with Accounting (at IV, 4.5 %: 0.955 > 0.94) or where a player ask
//!   undercuts the NPC. The rule finds both cases by the numbers; it is
//!   never told which.
//! - **The trader's skills** (X-TRADE): each cycle reads the pilot's own
//!   trained skills ([`crate::acquire::skill_levels`]), prices every deal at
//!   *its* sales tax and broker's fee ([`Costs::with_rates`], the server's
//!   `market_fees` formulas), and queues the next steps of [`SKILL_PLAN`]
//!   ([`plan_stage`], [`crate::acquire::learn_skills`]): Trade, Accounting,
//!   Broker Relations, then the order-count and remote-order skills. A
//!   trade is taken only when it is net-positive at the skills the pilot
//!   has *now*, so a fresh trader sits out the loop until Accounting trains.
//! - **Courier contracts** ([`best_courier`], pure; [`haul`]): the server
//!   serves player courier contracts (`contractProxy`, `contracts.rs` **[V]**
//!   read for this lane). A public outstanding courier whose reward, less
//!   the jumps of the trip, clears [`MIN_PROFIT`], whose package fits the
//!   hold and whose collateral is within [`COLLATERAL_SHARE`] of the wallet
//!   is accepted at its pickup station, flown, delivered
//!   (`CompleteContract(id, conStatusFinished)`) and the bot flies home.
//! - **The hold** ([`hold_of`]): the active hull's capacity from the
//!   content's type table; a pilot in a capsule boards or is given its
//!   rookie ship first (`acquire::rookie_ship`). A hauler hull is the
//!   closed economy's `acquire` (the role's kit), when it is affordable.
//!
//! **Every number here is [A] policy** -- what a trader would do -- except
//! the client's call shapes and constants, which cite **[V]**.

use crate::brain::{self, Act, Memory, Observation};
use crate::price_memory::Desk;
use crate::pricing::Rates;
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::speculate::{self, Exit};
use crate::travel::{self, Trips};
use crate::verbs::{Book, BookOrder, SellLine};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::market_fees::{
    MINIMUM_BROKER_FEE, Standings, TYPE_ACCOUNTING, TYPE_BROKER_RELATIONS, TYPE_DAYTRADING,
    TYPE_MARGIN_TRADING, TYPE_MARKETING, TYPE_PROCUREMENT, TYPE_RETAIL, TYPE_TRADE, TYPE_TYCOON,
    TYPE_VISIBILITY, TYPE_WHOLESALE, TradeSkills,
};
use std::collections::BTreeMap;
use std::time::Duration;

// ---- policy ----------------------------------------------------------------------

/// How often a trader runs one [`run`] cycle, docked **[A]**: a hauler
/// checks the market between trips. Falsified by traders whose `trade` act
/// lines crowd `[live]` (too often) or who sit docked for an hour beside a
/// paying loop (too seldom).
pub const TRADE_EVERY: (Duration, Duration) =
    (Duration::from_secs(5 * 60), Duration::from_secs(15 * 60));

/// **What a jump costs** a trader, ISK **[A]**: its time, about a minute
/// a jump, at a modest hauler's wage. Falsified by traders crossing five
/// systems for a few thousand ISK (too low) or never leaving home beside
/// a margin players take (too high).
pub const ISK_PER_JUMP: f64 = 15_000.0;

/// **The margin** a deal must clear over what it spends **[A]**: 1 %, a
/// buffer for a book that moves while the goods are in the hold. It was 5 %
/// while the rule priced the *worst* tax (Accounting 0) and a margin had to
/// cover what the tax could not; the tax and the broker's fee are now the
/// pilot's own ([`Costs::with_rates`]), so the margin is only risk.
/// Falsified by traders whose ledger is negative while they trade (too
/// low) or who never trade while the `[econ]` index shows regional spreads
/// over 5 % (too high).
pub const TRADE_MARGIN: f64 = 0.01;

/// The least ISK a trip must earn **[A]**: below it a player stays docked.
/// Falsified as [`TRADE_MARGIN`] is.
pub const MIN_PROFIT: f64 = 20_000.0;

/// **The longest trade trip**, jumps out, across and home **[A]**: two
/// [`travel::TRIP_MAX_JUMPS`]-ish legs. Falsified by traders out of their
/// session for an hour (too high) or never reaching the station that bids
/// (too low).
pub const TRADE_MAX_JUMPS: u32 = 8;

/// The most of its wallet one deal may spend **[A]**: the rest pays taxes
/// and the next trip. Falsified by traders that stall on `NotEnoughMoney`
/// (too high) or haul a quarter-empty hold with millions in the wallet
/// (too low).
pub const WALLET_SHARE: f64 = 0.6;

/// How many types a cycle probes **[A]**: one `GetOrders` and one history
/// read each, rotated so every trade good is seen within a few cycles.
/// Falsified by `[live]` showing market calls crowding human turns.
pub const PROBES_PER_CYCLE: usize = 12;

/// A listing at home is sized to this many days of the region's sales
/// **[A]**: a trader does not list a month of supply. Falsified by asks
/// that sit unsold for days (too high).
pub const LIST_DAYS_OF_VOLUME: f64 = 2.0;

/// How long a trader's listing rests, days: the builder's
/// ([`crate::builder::ORDER_DAYS`]).
pub const LIST_DAYS: i64 = crate::builder::ORDER_DAYS;

/// The most of its wallet a courier's collateral may hold **[A]**.
/// Falsified by haulers that cannot pay for a trade after a courier (too
/// high) or never take a contract that players issue (too low).
pub const COLLATERAL_SHARE: f64 = 0.5;

/// The groups of the NPC trade goods, category 17 (R-NPCTRADE §2:
/// `corporationTrades` positive entries, groups 280-284, 313 and 314)
/// **[V]**: the types NPCs both ask and bid (E5b, O5-i).
pub const TRADE_GOOD_GROUPS: [i64; 7] = [280, 281, 282, 283, 284, 313, 314];

/// `categoryCommodity` (`econ_exchange::CATEGORY_COMMODITY`) **[V]**.
pub const CATEGORY_COMMODITY: i64 = eve_server::econ_exchange::CATEGORY_COMMODITY;

/// The sales tax a trader assumes ([`crate::pricing::SALES_TAX`], the
/// client's 8 % at Accounting 0) **[V]**.
pub const SALES_TAX: f64 = crate::pricing::SALES_TAX;

/// The broker's fee on a listing ([`crate::pricing::BROKER_FEE`]) **[V]**.
pub const BROKER_FEE: f64 = crate::pricing::BROKER_FEE;

// ---- the trader's skill plan (X-TRADE) --------------------------------------------

/// One step of [`SKILL_PLAN`]: train `skill` to `level` once the wallet
/// holds `wallet` ISK **[A]** (a book and its months of training are for a
/// trader that has something to trade with; falsified by a trader that
/// never reaches Accounting IV with a million in the wallet).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanStep {
    pub skill: i64,
    pub level: i64,
    pub wallet: f64,
}

const fn step(skill: i64, level: i64, wallet: f64) -> PlanStep {
    PlanStep {
        skill,
        level,
        wallet,
    }
}

/// **What a trader trains, in order** **[A]** (the ids are the client's
/// **[V]**, `inventorycommon/const.py`; each step's prerequisites come from
/// the content's skill table at run time, never from here).
///
/// 1. *The tax and the fee* -- Trade IV (the prerequisite of Accounting and
///    Daytrading), Accounting IV and V (sales tax 8 % to 3.6 %), Broker
///    Relations IV and V (commission 3 % to 1.5 %). These decide whether the
///    NPC trade-goods loop pays at all.
/// 2. *More orders* -- Retail IV (`+8` orders a level), Margin Trading
///    (type 16597, which the content calls Advanced Broker Relations:
///    modification fee `0.5 + 0.06` discount a level), Marketing III and
///    Procurement IV (remote sell and buy orders), Visibility II (order
///    ranges), Daytrading III (remote modification).
/// 3. *The big book* -- Retail V, Wholesale V (`+16` a level), Marketing IV,
///    Tycoon I (`+32`).
///
/// Falsified by a trader whose order cap (5 + 4 Trade + 8 Retail ...) binds
/// before its skills do, or one that trains remote-order skills it never
/// uses (the server gates ranges by Visibility only; remote placement is
/// not gated, `docs/lanes/x-trade.md`).
pub const SKILL_PLAN: [PlanStep; 17] = [
    step(TYPE_TRADE, 4, 0.0),
    step(TYPE_ACCOUNTING, 4, 0.0),
    step(TYPE_BROKER_RELATIONS, 4, 0.0),
    step(TYPE_ACCOUNTING, 5, 0.0),
    step(TYPE_BROKER_RELATIONS, 5, 0.0),
    step(TYPE_RETAIL, 4, 2_000_000.0),
    step(TYPE_MARGIN_TRADING, 4, 2_000_000.0),
    step(TYPE_MARKETING, 3, 5_000_000.0),
    step(TYPE_PROCUREMENT, 4, 5_000_000.0),
    step(TYPE_VISIBILITY, 2, 5_000_000.0),
    step(TYPE_DAYTRADING, 3, 5_000_000.0),
    step(TYPE_RETAIL, 5, 20_000_000.0),
    step(TYPE_WHOLESALE, 5, 20_000_000.0),
    step(TYPE_MARKETING, 4, 20_000_000.0),
    step(TYPE_TYCOON, 1, 50_000_000.0),
    step(TYPE_MARGIN_TRADING, 5, 50_000_000.0),
    step(TYPE_DAYTRADING, 5, 50_000_000.0),
];

/// How many plan steps one cycle queues **[A]**: a book is bought for each
/// new skill, so the spend per cycle is bounded. Falsified by a skill queue
/// that sits empty while the plan has steps the wallet allows (too low).
pub const LEARN_PER_CYCLE: usize = 3;

/// **The next steps of the plan** (pure): from `levels` (`{skill: trained
/// level}`, a key meaning the book is injected) and the `wallet`, up to
/// [`LEARN_PER_CYCLE`] `(skill, level)` targets in [`SKILL_PLAN`] order.
///
/// A step is skipped when the skill is already at the level, and the walk
/// stops at the first step the wallet does not reach (later steps ask for
/// more). `requires(skill)` is its prerequisite list; a step is taken only
/// when each prerequisite is *trained*, or is staged in this very call for a
/// skill whose book is already injected -- so a purchase that fails can never
/// leave a queue the server refuses for an unmet prerequisite.
pub fn plan_stage(
    levels: &BTreeMap<i64, i64>,
    wallet: f64,
    requires: impl Fn(i64) -> Vec<(i64, i64)>,
) -> Vec<(i64, i64)> {
    // In plan order; a later step of a skill already staged raises its level
    // where the skill first appeared, so a queue trains it in one run.
    let mut staged: Vec<(i64, i64)> = Vec::new();
    let staged_level = |staged: &[(i64, i64)], skill: i64| {
        staged
            .iter()
            .find(|&&(s, _)| s == skill)
            .map(|&(_, level)| level)
    };
    for PlanStep {
        skill,
        level,
        wallet: needs,
    } in SKILL_PLAN
    {
        if wallet < needs {
            break;
        }
        let have = levels.get(&skill).copied().unwrap_or(0);
        if have >= level || staged_level(&staged, skill).is_some_and(|t| t >= level) {
            continue;
        }
        let ready = requires(skill).iter().all(|&(r, l)| {
            levels.get(&r).copied().unwrap_or(0) >= l
                || (levels.contains_key(&r) && staged_level(&staged, r).unwrap_or(0) >= l)
        });
        if !ready {
            continue;
        }
        if let Some(entry) = staged.iter_mut().find(|(s, _)| *s == skill) {
            entry.1 = level;
        } else if staged.len() < LEARN_PER_CYCLE {
            staged.push((skill, level));
        }
    }
    staged
}

/// How a cycle gets its tax and fee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillUse {
    /// The trader prices with [`Setup::costs`] exactly as given and trains
    /// nothing (tests; a fixed-rate scenario).
    Given,
    /// Read the pilot's own skills each cycle, price with them and queue the
    /// next steps of [`SKILL_PLAN`] (the driver's setting).
    Own,
}

// ---- the client's contract constants (`appConst`) [V] ----------------------------

/// `conTypeCourier`.
pub const CONTRACT_COURIER: i64 = 3;
/// `conAvailPublic`.
pub const AVAIL_PUBLIC: i64 = 0;
/// `conStatusOutstanding`.
pub const STATUS_OUTSTANDING: i64 = 0;
/// `conStatusFinished`: what the Complete button sends.
pub const STATUS_FINISHED: i64 = 4;
/// `typePlasticWrap`: the courier package.
pub const TYPE_PLASTIC_WRAP: i64 = 3468;

/// What the trader says it lacks when it has no map to travel over.
pub const MISSING_MAP: &str = "a map to trade over (eve-bots --content or --celestials)";

// ---- the row ---------------------------------------------------------------------

/// Trades and hauls, and stays out of fights.
pub static ROLE: RoleSpec = RoleSpec {
    name: "trader",
    greeting: "o7 -- hauling goods, not fighting",
    civilian: true,
    docked,
    in_space,
    banner,
    setup,
};

/// Docked: a hull first (a capsule has no hold), a map, then one
/// [`Act::Trade`] when its time has come and a wait for the next.
fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if o.failures.values().any(|&n| n >= brain::GIVE_UP_AFTER) {
        return vec![Act::Reconnect];
    }
    m.homing = None;
    let active = o.ship.map(|s| s.ship_id);
    let in_capsule = o
        .ship
        .is_none_or(|s| s.type_id.is_none_or(|t| t == brain::CAPSULE));
    if in_capsule {
        if let Some(hull) = o.hangar.iter().find(|r| {
            r.category_id == crate::acquire::CATEGORY_SHIP
                && r.type_id != brain::CAPSULE
                && r.singleton
                && Some(r.item_id) != active
                && !m.combat.untrained.contains(&r.type_id)
        }) {
            return vec![Act::Board(hull.item_id)];
        }
        return brain::gm_or_missing(m, true, false, false, "a hull with a hold");
    }
    if m.policy.travel.is_none() {
        return vec![Act::Missing(MISSING_MAP), Act::Wait(brain::MISSING_WAIT)];
    }
    if let Some(next) = m.market_next
        && o.now < next
    {
        return vec![Act::Wait(next - o.now)];
    }
    let next = o.now + m.jitter(TRADE_EVERY);
    m.market_next = Some(next);
    vec![Act::Trade, Act::Wait(next - o.now)]
}

/// In space: home, the miner's way.
fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    match &o.park {
        Some(park) => brain::go_home(m, o, park),
        None => vec![Act::Wait(brain::SHORT_WAIT)],
    }
}

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, reconnects {}",
        s.name, s.phase, s.reconnects
    )
}

/// A Merlin when the hangar has no hull, as the courier's setup: a hold.
fn setup(lacks: Lacks) -> Vec<String> {
    if lacks.ship {
        vec!["/createitem merlin".to_owned()]
    } else {
        Vec::new()
    }
}

// ---- the arbitrage choice, pure ----------------------------------------------------

/// The costs and thresholds a deal is judged by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Costs {
    /// The fraction of a sale the tax takes.
    pub sales_tax: f64,
    /// The fraction of a listing the broker takes.
    pub broker_fee: f64,
    pub isk_per_jump: f64,
    pub margin: f64,
    pub min_profit: f64,
}

impl Costs {
    /// These costs with the sales tax and the broker's fee of `rates`.
    pub fn with_rates(self, rates: Rates) -> Costs {
        Costs {
            sales_tax: rates.sales_tax,
            broker_fee: rates.broker_fee,
            ..self
        }
    }

    /// The default costs for a pilot with `skills` (no standings **[A]**,
    /// [`Rates::at`]).
    pub fn at_skills(skills: &TradeSkills) -> Costs {
        Costs::default().with_rates(Rates::at(skills, Standings::default()))
    }
}

impl Default for Costs {
    fn default() -> Costs {
        Costs {
            sales_tax: SALES_TAX,
            broker_fee: BROKER_FEE,
            isk_per_jump: ISK_PER_JUMP,
            margin: TRADE_MARGIN,
            min_profit: MIN_PROFIT,
        }
    }
}

/// One probed type: its book, its unit volume, and what the region pays.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub type_id: i64,
    /// m3 a unit.
    pub volume: f64,
    pub book: Book,
    /// `P_mkt`, if known.
    pub p_mkt: Option<f64>,
    /// The region's sales a day.
    pub daily_volume: f64,
}

/// Where a trip's goods are sold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outlet {
    /// Into the bids at the sell station, at once.
    Bid,
    /// Brought home and listed at `P_mkt`.
    List,
    /// Carried to a station in **another region** that a remembered book
    /// showed bidding (FE-TRADE), confirmed against the live book on
    /// arrival, and sold into the bid at `min_bid` or better.
    Remote,
}

/// Where the trader stands and what it carries with.
#[derive(Debug, Clone, Copy)]
pub struct Means<'a> {
    pub trips: Trips<'a>,
    /// The system it is in, docked.
    pub system: i64,
    /// Its home station, where every trip ends.
    pub home: i64,
    /// m3 of hold.
    pub hold: f64,
    pub wallet: f64,
}

/// One trade trip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Deal {
    pub type_id: i64,
    pub buy_station: i64,
    pub sell_station: i64,
    pub outlet: Outlet,
    pub units: i64,
    /// The dearest ask the plan takes: the buy cap.
    pub max_ask: f64,
    /// The cheapest bid the plan sells into, or the listing's price: the
    /// sell floor.
    pub min_bid: f64,
    /// ISK out.
    pub spend: f64,
    /// ISK in, after tax (and the broker's fee on a listing).
    pub revenue: f64,
    /// Jumps of the whole trip.
    pub jumps: u32,
    /// `revenue - spend - jumps x ISK_PER_JUMP`.
    pub profit: f64,
    /// `P_mkt` at home when the deal was planned, 0 if unknown: where an
    /// aborted [`Outlet::Remote`] leg lists what it brought back.
    pub home_value: f64,
}

impl Deal {
    /// The act line's note.
    pub fn summary(&self) -> String {
        format!(
            "type {} x{}: buy at {} <= {:.2}, {} at {} >= {:.2}, {} jumps, profit {:.0} ISK",
            self.type_id,
            self.units,
            self.buy_station,
            self.max_ask,
            match self.outlet {
                Outlet::Bid => "sell",
                Outlet::List => "list",
                Outlet::Remote => "carry to",
            },
            self.sell_station,
            self.min_bid,
            self.jumps,
            self.profit
        )
    }
}

/// One rung of a ladder: a price and the units at it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rung {
    price: f64,
    units: i64,
}

/// The fill of one buy ladder (cheapest first) against one sell ladder
/// (dearest first) at `keep` of each sale's price: units while a unit
/// earns, bounded by `max_units` and `max_spend`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Fill {
    units: i64,
    spend: f64,
    revenue: f64,
    max_ask: f64,
    min_bid: f64,
}

fn fill(asks: &[Rung], bids: &[Rung], keep: f64, max_units: i64, max_spend: f64) -> Fill {
    let mut f = Fill::default();
    let (mut i, mut j) = (0, 0);
    let mut ask_left = asks.first().map_or(0, |r| r.units);
    let mut bid_left = bids.first().map_or(0, |r| r.units);
    while i < asks.len() && j < bids.len() && f.units < max_units {
        let (a, b) = (asks[i], bids[j]);
        if b.price * keep <= a.price {
            break;
        }
        let afford = ((max_spend - f.spend) / a.price).floor();
        let afford = if afford <= 0.0 { 0 } else { afford as i64 };
        let k = ask_left.min(bid_left).min(max_units - f.units).min(afford);
        if k <= 0 {
            break;
        }
        f.units += k;
        f.spend += k as f64 * a.price;
        f.revenue += k as f64 * b.price * keep;
        f.max_ask = a.price;
        f.min_bid = b.price;
        ask_left -= k;
        bid_left -= k;
        if ask_left == 0 {
            i += 1;
            ask_left = asks.get(i).map_or(0, |r| r.units);
        }
        if bid_left == 0 {
            j += 1;
            bid_left = bids.get(j).map_or(0, |r| r.units);
        }
    }
    f
}

/// `orders` grouped by station, each station's ladder sorted cheapest first
/// (`dearest == false`) or dearest first.
fn ladders(orders: &[BookOrder], dearest: bool) -> BTreeMap<i64, Vec<Rung>> {
    let mut out: BTreeMap<i64, Vec<Rung>> = BTreeMap::new();
    for o in orders {
        if o.vol_remaining <= 0 || o.price <= 0.0 {
            continue;
        }
        out.entry(o.station_id).or_default().push(Rung {
            price: o.price,
            units: o.vol_remaining,
        });
    }
    for v in out.values_mut() {
        if dearest {
            v.sort_by(|a, b| b.price.total_cmp(&a.price));
        } else {
            v.sort_by(|a, b| a.price.total_cmp(&b.price));
        }
    }
    out
}

/// Jumps from system `from` to station `station`'s system, at the floor.
fn jumps_to(trips: Trips<'_>, from: i64, station: i64) -> Option<(i64, u32)> {
    let system = trips.map.system_of(station)?;
    Some((system, trips.map.jumps(from, system, trips.floor)?))
}

/// **The best deal** (pure) over `candidates`: every buy station S with an
/// ask and every sell station T with a bid for the same type -- plus home
/// with a listing at `P_mkt` when nothing at home asks that or less --
/// filled ladder against ladder while a unit earns after tax; the trip is
/// here, S, T, home, at most [`TRADE_MAX_JUMPS`]; units fit the hold and
/// [`WALLET_SHARE`] of the wallet. The deal with the most profit that
/// clears `costs.min_profit` and `costs.margin` of its spend wins; ties go
/// to fewer jumps, then the lower type, buy and sell station.
pub fn best_deal(candidates: &[Candidate], means: Means<'_>, costs: Costs) -> Option<Deal> {
    let Means {
        trips,
        system,
        home,
        hold,
        wallet,
    } = means;
    let home_back = trips.map.system_of(home)?;
    let budget = wallet * WALLET_SHARE;
    let mut best: Option<Deal> = None;
    for c in candidates {
        if c.volume <= 0.0 || !c.volume.is_finite() {
            continue;
        }
        let by_hold = (hold / c.volume).floor();
        if by_hold < 1.0 {
            continue;
        }
        let max_units = if by_hold >= i64::MAX as f64 {
            i64::MAX
        } else {
            by_hold as i64
        };
        let asks = ladders(
            &c.book
                .asks
                .iter()
                .filter(|o| !o.bid)
                .copied()
                .collect::<Vec<_>>(),
            false,
        );
        let mut outlets: Vec<(i64, Outlet, Vec<Rung>, f64)> = ladders(
            &c.book
                .bids
                .iter()
                .filter(|o| o.bid)
                .copied()
                .collect::<Vec<_>>(),
            true,
        )
        .into_iter()
        .map(|(t, ladder)| (t, Outlet::Bid, ladder, 1.0 - costs.sales_tax))
        .collect();
        if let Some(p) = c.p_mkt.filter(|p| *p > 0.0) {
            let undercut = asks
                .get(&home)
                .and_then(|l| l.first())
                .is_some_and(|r| r.price <= p);
            let units = (c.daily_volume * LIST_DAYS_OF_VOLUME).floor();
            if !undercut && units >= 1.0 {
                outlets.push((
                    home,
                    Outlet::List,
                    vec![Rung {
                        price: p,
                        units: units as i64,
                    }],
                    1.0 - costs.sales_tax - costs.broker_fee,
                ));
            }
        }
        for (&s, ask_ladder) in &asks {
            let Some((s_sys, out)) = jumps_to(trips, system, s) else {
                continue;
            };
            for (t, outlet, bid_ladder, keep) in &outlets {
                let Some((t_sys, across)) = jumps_to(trips, s_sys, *t) else {
                    continue;
                };
                let Some(back) = trips.map.jumps(t_sys, home_back, trips.floor) else {
                    continue;
                };
                let jumps = out + across + back;
                if jumps > TRADE_MAX_JUMPS {
                    continue;
                }
                let f = fill(ask_ladder, bid_ladder, *keep, max_units, budget);
                if f.units <= 0 {
                    continue;
                }
                // A listing pays at least `mktMinimumFee` to the broker.
                let floor = match outlet {
                    Outlet::List => {
                        let fee = f.revenue / keep * costs.broker_fee;
                        (MINIMUM_BROKER_FEE - fee).max(0.0)
                    }
                    Outlet::Bid | Outlet::Remote => 0.0,
                };
                let profit = f.revenue - floor - f.spend - f64::from(jumps) * costs.isk_per_jump;
                if profit < costs.min_profit || profit < costs.margin * f.spend {
                    continue;
                }
                let deal = Deal {
                    type_id: c.type_id,
                    buy_station: s,
                    sell_station: *t,
                    outlet: *outlet,
                    units: f.units,
                    max_ask: f.max_ask,
                    min_bid: f.min_bid,
                    spend: f.spend,
                    revenue: f.revenue,
                    jumps,
                    profit,
                    home_value: c.p_mkt.unwrap_or(0.0),
                };
                if best.is_none_or(|b| better(&deal, &b)) {
                    best = Some(deal);
                }
            }
        }
    }
    best
}

fn better(a: &Deal, b: &Deal) -> bool {
    a.profit
        .total_cmp(&b.profit)
        .then(b.jumps.cmp(&a.jumps))
        .then(b.type_id.cmp(&a.type_id))
        .then(b.buy_station.cmp(&a.buy_station))
        .then(b.sell_station.cmp(&a.sell_station))
        .is_gt()
}

/// **The best cross-region deal** (pure), FE-TRADE: buy at a station of this
/// region's book (`candidates`), carry the goods to the station in **another
/// region** that the desk's memory ([`crate::price_memory`]) saw bidding,
/// and sell into that remembered bid, aged to `day`. The trip is here,
/// buy station, sell station, home, at most `knobs.region_max_jumps`. The
/// deal must clear `costs.min_profit` and `costs.margin + knobs.region_risk`
/// of its spend, after the sales tax and every jump at `costs.isk_per_jump`:
/// the risk margin pays for a book that may have moved. Units fit the hold,
/// [`WALLET_SHARE`] of the wallet, the asks on offer and the **remembered
/// depth** of the bid. `here` is this region (its own memory is not a remote
/// outlet). The sell station is the remembered one when the map routes to it,
/// else the nearest station of that region ([`travel::find_in_region`]), where
/// the live read on arrival decides.
pub fn best_remote(
    candidates: &[Candidate],
    desk: &Desk,
    here: i64,
    day: i64,
    means: Means<'_>,
    costs: Costs,
) -> Option<Deal> {
    let knobs = desk.knobs;
    let Means {
        trips,
        system,
        home,
        hold,
        wallet,
    } = means;
    let home_back = trips.map.system_of(home)?;
    let budget = wallet * WALLET_SHARE;
    let keep = 1.0 - costs.sales_tax;
    let mut best: Option<Deal> = None;
    for c in candidates {
        if c.volume <= 0.0 || !c.volume.is_finite() {
            continue;
        }
        let by_hold = (hold / c.volume).floor();
        if by_hold < 1.0 {
            continue;
        }
        let max_units = if by_hold >= i64::MAX as f64 {
            i64::MAX
        } else {
            by_hold as i64
        };
        let asks = ladders(
            &c.book
                .asks
                .iter()
                .filter(|o| !o.bid)
                .copied()
                .collect::<Vec<_>>(),
            false,
        );
        for (region, bid) in desk.memory.bids_elsewhere(here, c.type_id, day) {
            for (&s, ask_ladder) in &asks {
                let Some((s_sys, out)) = jumps_to(trips, system, s) else {
                    continue;
                };
                let landing = trips
                    .map
                    .system_of(bid.station)
                    .filter(|&sys| trips.map.region_of(sys) == Some(region))
                    .and_then(|sys| {
                        Some(travel::Landing {
                            station: bid.station,
                            system: sys,
                            jumps: trips.map.jumps(s_sys, sys, trips.floor)?,
                        })
                    })
                    .or_else(|| {
                        travel::find_in_region(
                            trips,
                            s_sys,
                            region,
                            knobs.region_max_jumps,
                            Some(bid.station),
                        )
                    });
                let Some(land) = landing else {
                    continue;
                };
                let Some(back) = trips.map.jumps(land.system, home_back, trips.floor) else {
                    continue;
                };
                let jumps = out + land.jumps + back;
                if jumps > knobs.region_max_jumps {
                    continue;
                }
                let bids = [Rung {
                    price: bid.price,
                    units: bid.units,
                }];
                let f = fill(ask_ladder, &bids, keep, max_units, budget);
                if f.units <= 0 {
                    continue;
                }
                let profit = f.revenue - f.spend - f64::from(jumps) * costs.isk_per_jump;
                if profit < costs.min_profit
                    || profit < (costs.margin + knobs.region_risk) * f.spend
                {
                    continue;
                }
                let deal = Deal {
                    type_id: c.type_id,
                    buy_station: s,
                    sell_station: land.station,
                    outlet: Outlet::Remote,
                    units: f.units,
                    max_ask: f.max_ask,
                    min_bid: f.min_bid,
                    spend: f.spend,
                    revenue: f.revenue,
                    jumps,
                    profit,
                    home_value: c.p_mkt.unwrap_or(0.0),
                };
                if best.is_none_or(|b| better(&deal, &b)) {
                    best = Some(deal);
                }
            }
        }
    }
    best
}

// ---- the watch set ----------------------------------------------------------------

/// `categoryMaterial` and the mineral group: the hulls' and modules' inputs
/// (public game constants) **[V]**.
pub const CATEGORY_MATERIAL: i64 = 4;
pub const GROUP_MINERAL: i64 = 18;
/// Ships, modules and charges (`typedata`, `fitting`).
const CATEGORY_SHIP: i64 = eve_server::typedata::CATEGORY_SHIP;
const CATEGORY_MODULE: i64 = eve_server::typedata::CATEGORY_MODULE;
const CATEGORY_CHARGE: i64 = 8;

/// How many of the dearest hulls, modules and charges (by the server's
/// index price) a trader watches besides the trade goods and minerals
/// (`FE_WATCH_N` in the spec is 24 in all) **[A]**. Falsified by a tape
/// where hull prices diverge between regions and no trader moves one (raise),
/// or by market calls crowding human turns (lower).
pub const WATCH_HULLS: usize = 6;
pub const WATCH_MODULES: usize = 6;
pub const WATCH_CHARGES: usize = 4;

/// The dearest hull a trader watches, ISK **[A]**: a capital hull has no
/// book to read (and the freighter hauler is its own career). Falsified by
/// a tape that shows capital hulls trading.
pub const WATCH_HULL_CEILING: f64 = 200_000_000.0;

/// **What a trader watches** (pure): the trade goods and the minerals of
/// `content`, plus the dearest [`WATCH_HULLS`] hulls under
/// [`WATCH_HULL_CEILING`], [`WATCH_MODULES`] modules and [`WATCH_CHARGES`]
/// charges by `index` price (`(type, price)`); published types only; by
/// type id, each once. Empty without a type table. Deterministic: the dearest
/// first, ties to the lower id.
pub fn watch_set(content: &Content, index: &[(i64, f64)]) -> Vec<i64> {
    let Some(td) = content.try_get::<eve_server::typedata::Installed>() else {
        return Vec::new();
    };
    let table = td.table();
    let price: BTreeMap<i64, f64> = index.iter().copied().filter(|(_, p)| *p > 0.0).collect();
    let mut out: Vec<i64> = table
        .types()
        .filter(|t| {
            (t.category_id == CATEGORY_COMMODITY && TRADE_GOOD_GROUPS.contains(&t.group_id))
                || (t.category_id == CATEGORY_MATERIAL && t.group_id == GROUP_MINERAL)
        })
        .map(|t| t.type_id)
        .collect();
    for (category, n, ceiling) in [
        (CATEGORY_SHIP, WATCH_HULLS, WATCH_HULL_CEILING),
        (CATEGORY_MODULE, WATCH_MODULES, f64::INFINITY),
        (CATEGORY_CHARGE, WATCH_CHARGES, f64::INFINITY),
    ] {
        let mut ranked: Vec<(i64, f64)> = table
            .types()
            .filter(|t| t.category_id == category && t.published)
            .filter_map(|t| price.get(&t.type_id).map(|p| (t.type_id, *p)))
            .filter(|(_, p)| *p <= ceiling)
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        out.extend(ranked.into_iter().take(n).map(|(t, _)| t));
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The `n` types a cycle probes (pure): a window of `types` starting at
/// `offset` (mod the list), wrapping, each at most once.
pub fn probe_window(types: &[i64], offset: usize, n: usize) -> Vec<i64> {
    if types.is_empty() {
        return Vec::new();
    }
    let start = offset % types.len();
    types
        .iter()
        .cycle()
        .skip(start)
        .take(n.min(types.len()))
        .copied()
        .collect()
}

/// **The trade goods** in `content`'s type table: category
/// [`CATEGORY_COMMODITY`], a group in [`TRADE_GOOD_GROUPS`], by type id.
/// Empty without a type table.
pub fn trade_goods(content: &Content) -> Vec<i64> {
    let Some(td) = content.try_get::<eve_server::typedata::Installed>() else {
        return Vec::new();
    };
    td.table()
        .types()
        .filter(|t| t.category_id == CATEGORY_COMMODITY && TRADE_GOOD_GROUPS.contains(&t.group_id))
        .map(|t| t.type_id)
        .collect()
}

// ---- courier contracts, pure -------------------------------------------------------

/// One public courier contract, as `SearchContracts` shows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Offer {
    pub contract_id: i64,
    pub issuer_id: i64,
    pub start_station: i64,
    pub end_station: i64,
    pub reward: f64,
    pub collateral: f64,
    /// m3 of the package.
    pub volume: f64,
}

/// A courier the trader would take: the offer, the trip's jumps, the
/// profit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Courier {
    pub offer: Offer,
    pub jumps: u32,
    pub profit: f64,
}

/// **The best courier** (pure): an offer not issued by `me`, whose package
/// fits the hold, whose collateral is within [`COLLATERAL_SHARE`] of the
/// wallet, and whose trip -- here, pickup, drop-off, home -- is routable
/// within [`TRADE_MAX_JUMPS`]; the most `reward - jumps x ISK_PER_JUMP`
/// clearing `costs.min_profit` wins, ties to fewer jumps then the lower id.
pub fn best_courier(offers: &[Offer], me: i64, means: Means<'_>, costs: Costs) -> Option<Courier> {
    let Means {
        trips,
        system,
        home,
        hold,
        wallet,
    } = means;
    let home_sys = trips.map.system_of(home)?;
    let mut best: Option<Courier> = None;
    for o in offers {
        if o.issuer_id == me
            || o.volume > hold
            || o.collateral > wallet * COLLATERAL_SHARE
            || o.reward <= 0.0
        {
            continue;
        }
        let Some((s_sys, out)) = jumps_to(trips, system, o.start_station) else {
            continue;
        };
        let Some((e_sys, across)) = jumps_to(trips, s_sys, o.end_station) else {
            continue;
        };
        let Some(back) = trips.map.jumps(e_sys, home_sys, trips.floor) else {
            continue;
        };
        let jumps = out + across + back;
        if jumps > TRADE_MAX_JUMPS {
            continue;
        }
        let profit = o.reward - f64::from(jumps) * costs.isk_per_jump;
        if profit < costs.min_profit {
            continue;
        }
        let c = Courier {
            offer: *o,
            jumps,
            profit,
        };
        let wins = best.is_none_or(|b| {
            c.profit
                .total_cmp(&b.profit)
                .then(b.jumps.cmp(&c.jumps))
                .then(b.offer.contract_id.cmp(&c.offer.contract_id))
                .is_gt()
        });
        if wins {
            best = Some(c);
        }
    }
    best
}

/// A `KeyVal`'s attribute.
fn attr<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
    crate::dict_get(crate::attributes(v), name)
}

fn num(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Int(n) => Some(*n as f64),
        PyValue::Real(r) => Some(*r),
        _ => None,
    }
}

fn int(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        PyValue::Real(r) => Some(r.round() as i64),
        PyValue::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

/// The open courier offers in a `SearchContracts` answer: `ret.contracts`,
/// each `.contract` a row with `contractID`, `type`, `status`, `issuerID`,
/// `startStationID`, `endStationID`, `reward`, `collateral`, `volume`
/// (`contractsearch.py`, `Contract::row`) **[V]**. Rows that do not read,
/// or are not outstanding couriers, are skipped.
pub fn offers_of(answer: &PyValue) -> Vec<Offer> {
    let Some(PyValue::List(found)) = attr(answer, "contracts") else {
        return Vec::new();
    };
    found
        .iter()
        .filter_map(|d| {
            let c = attr(d, "contract")?;
            if int(attr(c, "type"))? != CONTRACT_COURIER
                || int(attr(c, "status"))? != STATUS_OUTSTANDING
            {
                return None;
            }
            Some(Offer {
                contract_id: int(attr(c, "contractID"))?,
                issuer_id: int(attr(c, "issuerID")).unwrap_or(0),
                start_station: int(attr(c, "startStationID"))?,
                end_station: int(attr(c, "endStationID"))?,
                reward: num(attr(c, "reward")).unwrap_or(0.0),
                collateral: num(attr(c, "collateral")).unwrap_or(0.0),
                volume: num(attr(c, "volume")).unwrap_or(0.0),
            })
        })
        .collect()
}

// ---- the trip, flown -------------------------------------------------------------

/// What a trader is given for a cycle.
#[derive(Debug, Clone, Copy)]
pub struct Setup<'a> {
    pub trips: Trips<'a>,
    /// The type table (volumes, the hull's hold); `None`: no trade.
    pub content: Option<&'a Content>,
    /// The types to probe; empty: [`trade_goods`] of the content.
    pub types: &'a [i64],
    /// Which window of the types this cycle probes ([`probe_window`]).
    pub offset: usize,
    /// Look for courier contracts too.
    pub couriers: bool,
    pub costs: Costs,
    /// Whether the cycle reads and trains the pilot's own skills.
    pub skills: SkillUse,
}

/// What one [`run`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Traded {
    pub deal: Option<Deal>,
    pub courier: Option<Courier>,
    /// Units bought, sold into bids, and listed.
    pub bought: i64,
    pub sold: i64,
    pub listed: i64,
    /// The courier delivered.
    pub delivered: bool,
    pub jumps: u32,
    /// The wallet before and after.
    pub before: f64,
    pub after: f64,
    /// Why something was not done.
    pub notes: Vec<String>,
    /// The speculator's tape rows this cycle ([`speculate::spec_row`]).
    pub spec: Vec<String>,
}

impl Traded {
    /// The act line's note.
    pub fn summary(&self) -> String {
        let mut s = match (&self.deal, &self.courier) {
            (Some(d), _) => format!(
                "{}; bought {}, sold {}, listed {}",
                d.summary(),
                self.bought,
                self.sold,
                self.listed
            ),
            (None, Some(c)) => format!(
                "courier {} from {} to {} for {:.0} ISK ({} jumps): {}",
                c.offer.contract_id,
                c.offer.start_station,
                c.offer.end_station,
                c.offer.reward,
                c.jumps,
                if self.delivered {
                    "delivered"
                } else {
                    "not delivered"
                }
            ),
            (None, None) => "no deal".to_owned(),
        };
        s.push_str(&format!(
            "; {} jumps; wallet {:.2} -> {:.2} ISK ({:+.2})",
            self.jumps,
            self.before,
            self.after,
            self.after - self.before
        ));
        for n in &self.notes {
            s.push_str("; ");
            s.push_str(n);
        }
        for row in &self.spec {
            s.push_str("; ");
            s.push_str(&row.replace('\t', " "));
        }
        s
    }
}

/// A refusal is a note and the trip goes on; a broken connection ends it.
fn noted<T>(r: Result<T>, notes: &mut Vec<String>, what: &str) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => {
            notes.push(format!("{what}: {e}"));
            Ok(None)
        }
    }
}

/// **The hold**, m3: the active hull's capacity in the content's type
/// table, boarding or being given the rookie ship first when the pilot
/// sits in a capsule. `Ok(Err(why))` when there is none.
pub fn hold_of(c: &mut Conn, content: &Content) -> Result<std::result::Result<f64, String>> {
    let home = c
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("trade: not docked".to_owned()))?;
    if crate::acquire::flying_capsule(c)?
        && let Err(why) = crate::acquire::rookie_ship(c, home)?
    {
        return Ok(Err(format!(
            "a capsule has no hold, and no rookie ship ({why})"
        )));
    }
    let ship = c.session().ship_id();
    let hull = c
        .hangar_items()?
        .into_iter()
        .find(|r| Some(r.item_id) == ship)
        .map(|r| r.type_id);
    let Some(hull) = hull else {
        return Ok(Err("the active ship is not in the hangar".to_owned()));
    };
    match eve_server::item::find_in(content, hull).map(|t| t.capacity) {
        Some(m3) if m3 > 0.0 => Ok(Ok(m3)),
        _ => Ok(Err(format!("hull {hull} has no hold the type table knows"))),
    }
}

/// **One trader cycle, docked**, with a fresh desk that remembers nothing
/// past the call ([`run_with`] is the driver's).
pub fn run(c: &mut Conn, setup: &Setup<'_>) -> Result<Traded> {
    run_with(c, setup, &mut Desk::default())
}

/// One probed type read into a [`Candidate`]: its volume, its live book
/// (written into the desk's memory under `region`), `V` (folded into the
/// anchor) and the region's daily volume. `None` when the type or its book
/// is unreadable.
#[allow(clippy::too_many_arguments)]
fn probe(
    c: &mut Conn,
    content: &Content,
    prices: &mut Option<crate::pricing::Prices>,
    desk: &mut Desk,
    region: i64,
    day: i64,
    type_id: i64,
    notes: &mut Vec<String>,
) -> Result<Option<Candidate>> {
    let Some(volume) = eve_server::item::find_in(content, type_id).map(|t| t.volume) else {
        return Ok(None);
    };
    let Some(book) = noted(c.market_orders(type_id), notes, "book")? else {
        return Ok(None);
    };
    let (p_mkt, daily_volume) = match prices.as_mut() {
        Some(p) => (
            p.p_mkt(c, type_id).ok().flatten(),
            p.daily_volume(c, type_id).unwrap_or(0.0),
        ),
        None => (None, 0.0),
    };
    desk.memory.record(region, type_id, &book, day);
    if let Some(v) = p_mkt {
        desk.memory.observe(region, type_id, v, day);
    }
    desk.touch();
    Ok(Some(Candidate {
        type_id,
        volume,
        book,
        p_mkt,
        daily_volume,
    }))
}

/// **One trader cycle, docked**: the hold; the probes (each type's book,
/// `P_mkt` and sales, written into the `desk`'s memory); the speculator's
/// sells and buys at this station ([`speculate_here`]); the open couriers
/// when `setup.couriers`; the best of the best local deal, the best
/// cross-region deal ([`best_remote`]) and the best courier by profit; then
/// that trip, home at the end. Nothing worth doing is `Ok` with no deal. A
/// refusal on the way is a note; a trip that cannot be flown is the error
/// (the driver then flies the bot home, SOC-A2).
pub fn run_with(c: &mut Conn, setup: &Setup<'_>, desk: &mut Desk) -> Result<Traded> {
    let home = c
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("trade: not docked".to_owned()))?;
    let system = c
        .session()
        .solar_system_id2()
        .ok_or_else(|| Error::NotSent("trade: no system".to_owned()))?;
    let mut got = Traded {
        before: c.cash_balance()?,
        ..Traded::default()
    };
    got.after = got.before;
    let Some(content) = setup.content else {
        got.notes
            .push("no type table (eve-bots --content): no volumes, no hold".to_owned());
        return Ok(got);
    };
    let hold = match hold_of(c, content)? {
        Ok(m3) => m3,
        Err(why) => {
            got.notes.push(why);
            return Ok(got);
        }
    };
    let region = setup.trips.map.region_of(system).unwrap_or(0);
    let day = desk.today();

    let all = if setup.types.is_empty() {
        let index = noted(c.adjusted_prices(), &mut got.notes, "index")?.unwrap_or_default();
        let watch = watch_set(content, &index);
        if watch.is_empty() {
            trade_goods(content)
        } else {
            watch
        }
    } else {
        setup.types.to_vec()
    };
    let mut prices = noted(crate::pricing::Prices::read(c), &mut got.notes, "prices")?;
    let mut candidates = Vec::new();
    for type_id in probe_window(&all, setup.offset, PROBES_PER_CYCLE) {
        if let Some(cand) = probe(
            c,
            content,
            &mut prices,
            desk,
            region,
            day,
            type_id,
            &mut got.notes,
        )? {
            candidates.push(cand);
        }
    }
    if region != 0 {
        speculate_here(
            c,
            content,
            &mut prices,
            desk,
            &mut candidates,
            (region, day, home),
            &mut got,
        )?;
    }
    let wallet = c.cash_balance()?;
    let means = Means {
        trips: setup.trips,
        system,
        home,
        hold,
        wallet,
    };
    let costs = match setup.skills {
        SkillUse::Given => setup.costs,
        SkillUse::Own => skilled_costs(c, content, setup.costs, wallet, &mut got.notes)?,
    };
    let local = best_deal(&candidates, means, costs);
    let remote = if region != 0 {
        best_remote(&candidates, desk, region, day, means, costs)
    } else {
        None
    };
    let deal = match (local, remote) {
        (Some(a), Some(b)) => Some(if better(&b, &a) { b } else { a }),
        (a, b) => a.or(b),
    };
    let courier = if setup.couriers {
        let me = c.session().char_id().unwrap_or(0);
        noted(search_couriers(c), &mut got.notes, "contracts")?
            .and_then(|offers| best_courier(&offers, me, means, costs))
    } else {
        None
    };
    match (deal, courier) {
        (Some(d), Some(k)) if k.profit > d.profit => haul(c, setup.trips, k, &mut got)?,
        (Some(d), _) => trade(c, setup.trips, d, costs, desk, &mut got)?,
        (None, Some(k)) => haul(c, setup.trips, k, &mut got)?,
        (None, None) => {}
    }
    desk.memory.prune(day);
    got.after = c.cash_balance()?;
    Ok(got)
}

/// The units of `type_id` in the hangar here, loose (not fitted, not a
/// packaged hull's singleton).
fn loose(c: &mut Conn, type_id: i64) -> Result<Vec<crate::verbs::ItemRow>> {
    Ok(c.hangar_items()?
        .into_iter()
        .filter(|r| r.type_id == type_id && !r.is_fitted() && !r.singleton)
        .collect())
}

/// **The speculator's turn at this station** (FE-TRADE, [`crate::speculate`]):
/// its holdings here are sold when `V` has climbed over the anchor or they
/// have aged out, then types under their anchor are bought within the caps.
/// `at` is `(region, day, station)`. Off with [`speculate::SpecKnobs::wallet_share`]
/// 0. A refusal is a note; only a dead connection is the error.
fn speculate_here(
    c: &mut Conn,
    content: &Content,
    prices: &mut Option<crate::pricing::Prices>,
    desk: &mut Desk,
    candidates: &mut Vec<Candidate>,
    at: (i64, i64, i64),
    got: &mut Traded,
) -> Result<()> {
    let (region, day, station) = at;
    let k = desk.knobs;
    if k.wallet_share <= 0.0 && desk.holds.is_empty() {
        return Ok(());
    }
    // Sells first: what is held here, and why it goes.
    let held: Vec<i64> = desk
        .holds
        .iter()
        .filter(|(_, h)| h.station == station)
        .map(|(t, _)| *t)
        .collect();
    for t in held {
        let Some(h) = desk.holds.get(&t).copied() else {
            continue;
        };
        if !candidates.iter().any(|x| x.type_id == t)
            && let Some(cand) = probe(c, content, prices, desk, region, day, t, &mut got.notes)?
        {
            candidates.push(cand);
        }
        let v = candidates
            .iter()
            .find(|x| x.type_id == t)
            .and_then(|x| x.p_mkt);
        let rows = loose(c, t)?;
        let have: i64 = rows.iter().map(|r| r.quantity).sum();
        if have <= 0 {
            got.notes
                .push(format!("holding of {t} is gone from the hangar"));
            desk.holds.remove(&t);
            desk.touch();
            continue;
        }
        let anchor = desk.memory.anchor(h.region, t).copied();
        let Some(why) = speculate::exit_why(&k, &h, anchor.as_ref(), v.unwrap_or(0.0), day) else {
            continue;
        };
        // No V to price by (aged out with no read): at what it cost.
        let price = match (why, v) {
            (_, Some(v)) => speculate::exit_price(why, v),
            (Exit::Aged, None) => h.cost / h.units.max(1) as f64,
            (Exit::Gain, None) => continue,
        };
        let mut left = h.units.min(have);
        let mut lines = Vec::new();
        for r in rows {
            let q = r.quantity.min(left);
            if q <= 0 {
                break;
            }
            left -= q;
            lines.push(SellLine {
                station_id: station,
                type_id: t,
                item_id: r.item_id,
                price,
                quantity: q,
            });
        }
        let units: i64 = lines.iter().map(|l| l.quantity).sum();
        if noted(
            c.sell(&lines, LIST_DAYS, BROKER_FEE),
            &mut got.notes,
            "spec sell",
        )?
        .is_some()
        {
            got.listed += units;
            got.notes.push(format!(
                "spec: listed {units} of {t} at {price:.2} ({})",
                match why {
                    Exit::Gain => "over its anchor",
                    Exit::Aged => "held long enough",
                }
            ));
            desk.holds.remove(&t);
            desk.cool.insert(t, day + k.cool_days);
            desk.touch();
        }
    }
    // Buys: the types under their anchor, deepest discount first.
    if k.wallet_share > 0.0 && desk.holds.len() < k.max_positions {
        let mut picks: Vec<(f64, usize)> = Vec::new();
        for (i, cand) in candidates.iter().enumerate() {
            let (Some(v), Some(a)) = (cand.p_mkt, desk.memory.anchor(region, cand.type_id)) else {
                continue;
            };
            if desk.holds.contains_key(&cand.type_id)
                || desk.cool.get(&cand.type_id).is_some_and(|&d| day < d)
                || !speculate::wants_entry(&k, a, v)
            {
                continue;
            }
            picks.push((v / a.ewma, i));
        }
        picks.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        for (_, i) in picks {
            let cand = &candidates[i];
            let Some(v) = cand.p_mkt else { continue };
            let wallet = c.cash_balance()?;
            let held_cost = desk.held_cost();
            let ask_units: i64 = cand
                .book
                .asks
                .iter()
                .filter(|o| !o.bid && o.station_id == station && o.price <= v && o.price > 0.0)
                .map(|o| o.vol_remaining.max(0))
                .sum();
            let units = speculate::entry_units(
                &k,
                wallet + held_cost,
                held_cost,
                0,
                desk.holds.len(),
                cand.daily_volume,
                v,
                ask_units,
            );
            if units <= 0 {
                continue;
            }
            let money = (k.wallet_share * (wallet + held_cost) - held_cost).max(0.0);
            let plan = crate::acquire::plan(&cand.book, station, Some(v), money.min(wallet), units);
            let (mut bought, mut paid) = (0, 0.0);
            for (ask, n) in plan {
                let Some(row) = noted(
                    c.buy(station, cand.type_id, ask.price, n, 0, BROKER_FEE),
                    &mut got.notes,
                    "spec buy",
                )?
                else {
                    break;
                };
                let filled = n - row.vol_remaining.clamp(0, n);
                bought += filled;
                paid += filled as f64 * ask.price;
            }
            if bought > 0 {
                desk.holds.insert(
                    cand.type_id,
                    crate::price_memory::Hold {
                        units: bought,
                        cost: paid,
                        day,
                        station,
                        region,
                    },
                );
                desk.touch();
                got.notes.push(format!(
                    "spec: bought {bought} of {} at {:.2} (V {v:.2})",
                    cand.type_id,
                    paid / bought as f64
                ));
            }
            if desk.holds.len() >= k.max_positions {
                break;
            }
        }
    }
    let (units, isk) = desk
        .holds
        .values()
        .filter(|h| h.region == region)
        .fold((0, 0.0), |(u, i), h| (u + h.units, i + h.cost));
    if units > 0 {
        got.spec.push(speculate::spec_row(region, units, isk));
    }
    Ok(())
}

/// **The cycle's costs at the pilot's own skills** (X-TRADE): its trained
/// levels ([`crate::acquire::skill_levels`]) set the sales tax and the
/// broker's fee, and the next steps of [`SKILL_PLAN`] that its books and
/// wallet allow are bought and queued here ([`crate::acquire::learn_skills`],
/// this station's books only **[A]**: falsified by a trader whose home sells
/// no Trade book and who never learns). A refusal is a note; only a dead
/// connection is the error. Without a skill read the costs stay as given.
fn skilled_costs(
    c: &mut Conn,
    content: &Content,
    base: Costs,
    wallet: f64,
    notes: &mut Vec<String>,
) -> Result<Costs> {
    let Some(levels) = noted(crate::acquire::skill_levels(c), notes, "skills")? else {
        return Ok(base);
    };
    let stage = plan_stage(&levels, wallet, |s| {
        eve_server::skill::required_in(content, s).to_vec()
    });
    if !stage.is_empty()
        && let Some(got) = noted(
            crate::acquire::learn_skills(c, &stage, None),
            notes,
            "learn",
        )?
    {
        notes.extend(got.missing);
        if !got.learned.is_empty() {
            notes.push(format!("queued skills {:?}", got.learned));
        }
    }
    let skills = TradeSkills::read(|t| levels.get(&t).copied().unwrap_or(0));
    Ok(base.with_rates(Rates::at(&skills, Standings::default())))
}

/// Fly to `station` unless docked there; the jumps.
fn go(c: &mut Conn, trips: Trips<'_>, station: i64) -> Result<u32> {
    if c.session().station_id() == Some(station) {
        return Ok(0);
    }
    Ok(travel::go(c, trips, station)?.jumps)
}

/// **A trade trip**: to the buy station; the buys, cheapest first up to the
/// deal's cap and units inside the wallet; the hold loaded; to the sell
/// station; the hold unloaded and the goods sold into the bids at the
/// deal's floor (or listed at its price); home. A **remote** deal is read
/// again on arrival ([`settle_remote`]).
fn trade(
    c: &mut Conn,
    trips: Trips<'_>,
    deal: Deal,
    costs: Costs,
    desk: &mut Desk,
    got: &mut Traded,
) -> Result<()> {
    let home = c.session().station_id().unwrap_or(deal.sell_station);
    got.deal = Some(deal);
    got.jumps += go(c, trips, deal.buy_station)?;
    if let Some(book) = noted(c.market_orders(deal.type_id), &mut got.notes, "book")? {
        let wallet = c.cash_balance()?;
        let plan = crate::acquire::plan(
            &book,
            deal.buy_station,
            Some(deal.max_ask),
            wallet * WALLET_SHARE,
            deal.units,
        );
        for (ask, k) in plan {
            let Some(row) = noted(
                c.buy(deal.buy_station, deal.type_id, ask.price, k, 0, BROKER_FEE),
                &mut got.notes,
                "buy",
            )?
            else {
                break;
            };
            got.bought += k - row.vol_remaining.clamp(0, k);
        }
    }
    if got.bought > 0 && deal.sell_station != deal.buy_station {
        for row in c.hangar_items()? {
            if row.type_id == deal.type_id && !row.is_fitted() {
                noted(c.load_cargo(row.item_id, None), &mut got.notes, "load")?;
            }
        }
        got.jumps += go(c, trips, deal.sell_station)?;
        for row in c.cargo()? {
            if row.type_id == deal.type_id {
                noted(c.unload(row.item_id, None), &mut got.notes, "unload")?;
            }
        }
    }
    if got.bought > 0 {
        match deal.outlet {
            Outlet::Remote => settle_remote(c, trips, deal, costs, desk, home, got)?,
            _ => sell(c, deal, got)?,
        }
    }
    got.jumps += go(c, trips, home)?;
    Ok(())
}

/// **The arrival of a remote leg** (FE-TRADE): the live book here is read
/// and written into the desk's memory; if its best bid at this station pays
/// the unit cost plus the margin after tax ([`speculate::confirm`]) the goods
/// are sold into the bids at that floor; otherwise the leg is **aborted**:
/// the goods go home and are listed there at the value the plan knew.
fn settle_remote(
    c: &mut Conn,
    trips: Trips<'_>,
    deal: Deal,
    costs: Costs,
    desk: &mut Desk,
    home: i64,
    got: &mut Traded,
) -> Result<()> {
    let station = c.session().station_id().unwrap_or(deal.sell_station);
    let region = trips.map.region_of_station(station).unwrap_or(0);
    let book = noted(c.market_orders(deal.type_id), &mut got.notes, "book")?;
    if let Some(b) = &book {
        desk.memory.record(region, deal.type_id, b, desk.today());
        desk.touch();
    }
    let live = book.as_ref().and_then(|b| {
        b.bids
            .iter()
            .filter(|o| o.bid && o.station_id == station && o.vol_remaining > 0)
            .map(|o| o.price)
            .max_by(f64::total_cmp)
    });
    let unit_cost = deal.spend / deal.units.max(1) as f64;
    if speculate::confirm(live, unit_cost, costs.margin, costs.sales_tax) {
        let floor = speculate::confirm_floor(unit_cost, costs.margin, costs.sales_tax);
        let here = Deal {
            sell_station: station,
            min_bid: floor,
            ..deal
        };
        return sell(c, here, got);
    }
    got.notes.push(format!(
        "remote leg aborted: live bid {} under cost {unit_cost:.2} plus margin",
        live.map_or("none".to_owned(), |b| format!("{b:.2}"))
    ));
    got.jumps += go(c, trips, home)?;
    if deal.home_value > 0.0 {
        let listed = Deal {
            outlet: Outlet::List,
            sell_station: home,
            min_bid: deal.home_value,
            ..deal
        };
        sell(c, listed, got)?;
    } else {
        got.notes
            .push("no home value known: the goods stay in the hangar".to_owned());
    }
    Ok(())
}

/// The goods of `deal`'s type in the hangar here, sold into the bids at
/// the deal's floor, or listed at its price.
fn sell(c: &mut Conn, deal: Deal, got: &mut Traded) -> Result<()> {
    let station = deal.sell_station;
    let lines: Vec<SellLine> = c
        .hangar_items()?
        .into_iter()
        .filter(|r| r.type_id == deal.type_id && !r.is_fitted() && !r.singleton)
        .map(|r| SellLine {
            station_id: station,
            type_id: r.type_id,
            item_id: r.item_id,
            price: deal.min_bid,
            quantity: r.quantity,
        })
        .collect();
    let units: i64 = lines.iter().map(|l| l.quantity).sum();
    if units == 0 {
        got.notes
            .push(format!("nothing of type {} to sell", deal.type_id));
        return Ok(());
    }
    let (days, fee) = match deal.outlet {
        Outlet::Bid | Outlet::Remote => (0, 0.0),
        Outlet::List => (LIST_DAYS, BROKER_FEE),
    };
    if noted(c.sell(&lines, days, fee), &mut got.notes, "sell")?.is_none() {
        return Ok(());
    }
    let left: i64 = c
        .hangar_items()?
        .iter()
        .filter(|r| r.type_id == deal.type_id && !r.is_fitted() && !r.singleton)
        .map(|r| r.quantity)
        .sum();
    match deal.outlet {
        Outlet::Bid | Outlet::Remote => {
            got.sold += units - left;
            if left > 0 {
                got.notes
                    .push(format!("{left} units unsold at station {station}"));
            }
        }
        Outlet::List => got.listed += units - left,
    }
    Ok(())
}

/// `contractProxy.SearchContracts(availability=public,
/// contractType=courier, startNum=0)`: the Search tab's call with the
/// filters a hauler sets (`contractsearch.py` `SearchContracts`) **[V]**
/// for the call; which filters a bot sends is **[A]**.
pub fn search_couriers(c: &mut Conn) -> Result<Vec<Offer>> {
    let kw = |k: &str| eve_server::pyutil::bstr(k);
    let outcome = c.call_kw(
        "contractProxy",
        "SearchContracts",
        vec![],
        vec![
            (PyValue::Int(AVAIL_PUBLIC), kw("availability")),
            (PyValue::Int(CONTRACT_COURIER), kw("contractType")),
            (PyValue::Int(0), kw("startNum")),
        ],
    )?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!("SearchContracts: {key}")));
    }
    Ok(outcome.value().as_ref().map(offers_of).unwrap_or_default())
}

/// **A courier run**: to the pickup; `AcceptContract(id, forCorp=False)`
/// (`contracts.py` `AcceptContract`) **[V]**, which leaves the package in
/// the hangar there; the package loaded; to the drop-off; unloaded;
/// `CompleteContract(id, conStatusFinished)` (`contracts.py`
/// `CompleteContract`) **[V]**; home.
fn haul(c: &mut Conn, trips: Trips<'_>, k: Courier, got: &mut Traded) -> Result<()> {
    let home = c.session().station_id().unwrap_or(k.offer.end_station);
    got.courier = Some(k);
    let id = k.offer.contract_id;
    got.jumps += go(c, trips, k.offer.start_station)?;
    let accepted = noted(
        c.call(
            "contractProxy",
            "AcceptContract",
            vec![PyValue::Int(id), PyValue::Bool(false)],
        ),
        &mut got.notes,
        "accept",
    )?;
    let accepted = match accepted {
        Some(o) => match o.refusal() {
            Some(key) => {
                got.notes.push(format!("accept {id}: {key}"));
                false
            }
            None => true,
        },
        None => false,
    };
    if accepted {
        // The package is the one `typePlasticWrap` the accept left in the
        // hangar here; the contract row does not name it.
        for row in c.hangar_items()? {
            if row.type_id == TYPE_PLASTIC_WRAP {
                noted(c.load_cargo(row.item_id, None), &mut got.notes, "load")?;
            }
        }
        got.jumps += go(c, trips, k.offer.end_station)?;
        for row in c.cargo()? {
            if row.type_id == TYPE_PLASTIC_WRAP {
                noted(c.unload(row.item_id, None), &mut got.notes, "unload")?;
            }
        }
        let done = noted(
            c.call(
                "contractProxy",
                "CompleteContract",
                vec![PyValue::Int(id), PyValue::Int(STATUS_FINISHED)],
            ),
            &mut got.notes,
            "complete",
        )?;
        got.delivered = match done {
            Some(o) => match o.refusal() {
                Some(key) => {
                    got.notes.push(format!("complete {id}: {key}"));
                    false
                }
                None => true,
            },
            None => false,
        };
    }
    got.jumps += go(c, trips, home)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Invented ids, prices and volumes only; no [A] value in an
    //! assertion (the costs are spelled out per test).
    use super::*;
    use crate::travel::{ANY_SECURITY, Map};

    const S1: i64 = 39_100_001;
    const S2: i64 = 39_100_002;
    const S3: i64 = 39_100_003;
    const HOME: i64 = 69_100_001;
    const NEAR: i64 = 69_100_002;
    const FAR: i64 = 69_100_003;
    const GOOD: i64 = 990_500_001;
    const OTHER: i64 = 990_500_002;

    /// Free trips, no tax: the tests set what they need.
    const FREE: Costs = Costs {
        sales_tax: 0.0,
        broker_fee: 0.0,
        isk_per_jump: 0.0,
        margin: 0.0,
        min_profit: 1.0,
    };

    fn gate(a: i64, b: i64) -> i64 {
        59_100_000 + (a % 1000) * 1000 + b % 1000
    }

    /// HOME in S1 - NEAR in S2 - FAR in S3, a line.
    fn map() -> Map {
        let mut m = Map::new();
        for (a, b) in [(S1, S2), (S2, S3)] {
            m = m
                .with_gate(a, gate(a, b), b, gate(b, a))
                .with_gate(b, gate(b, a), a, gate(a, b));
        }
        m.with_station(HOME, S1)
            .with_station(NEAR, S2)
            .with_station(FAR, S3)
    }

    fn means(m: &Map, hold: f64, wallet: f64) -> Means<'_> {
        Means {
            trips: Trips {
                map: m,
                floor: ANY_SECURITY,
            },
            system: S1,
            home: HOME,
            hold,
            wallet,
        }
    }

    fn order(id: i64, station: i64, price: f64, vol: i64, bid: bool) -> BookOrder {
        BookOrder {
            order_id: id,
            type_id: GOOD,
            station_id: station,
            price,
            vol_remaining: vol,
            bid,
        }
    }

    fn cand(type_id: i64, asks: Vec<BookOrder>, bids: Vec<BookOrder>) -> Candidate {
        Candidate {
            type_id,
            volume: 1.0,
            book: Book { asks, bids },
            p_mkt: None,
            daily_volume: 0.0,
        }
    }

    /// An ask under another station's bid is a deal: bought at the ask,
    /// sold into the bid, the trip home-NEAR-FAR-home is four jumps.
    #[test]
    fn an_ask_under_another_stations_bid_is_a_deal() {
        let m = map();
        let c = cand(
            GOOD,
            vec![order(1, NEAR, 100.0, 50, false)],
            vec![order(2, FAR, 150.0, 30, true)],
        );
        let d = best_deal(&[c], means(&m, 1e9, 1e9), FREE).expect("a deal");
        assert_eq!((d.buy_station, d.sell_station), (NEAR, FAR));
        assert_eq!(d.outlet, Outlet::Bid);
        assert_eq!(d.units, 30, "capped by the bid's volume");
        assert_eq!(d.jumps, 1 + 1 + 2);
        assert_eq!(d.spend, 3_000.0);
        assert_eq!(d.revenue, 4_500.0);
        assert_eq!(d.profit, 1_500.0);
        assert_eq!((d.max_ask, d.min_bid), (100.0, 150.0));
    }

    /// The tax and the jumps decide: a spread the tax eats is no deal, and
    /// one the jumps eat is no deal either.
    #[test]
    fn tax_and_jumps_eat_a_thin_spread() {
        let m = map();
        let c = cand(
            GOOD,
            vec![order(1, NEAR, 94.0, 100, false)],
            vec![order(2, FAR, 100.0, 100, true)],
        );
        let taxed = Costs {
            sales_tax: 0.08,
            ..FREE
        };
        assert_eq!(
            best_deal(std::slice::from_ref(&c), means(&m, 1e9, 1e9), taxed),
            None
        );
        let light = Costs {
            sales_tax: 0.036,
            ..FREE
        };
        let d = best_deal(std::slice::from_ref(&c), means(&m, 1e9, 1e9), light)
            .expect("a lighter tax pays");
        assert_eq!(d.units, 100);
        let dear = Costs {
            isk_per_jump: d.profit / f64::from(d.jumps) + 1.0,
            ..light
        };
        assert_eq!(best_deal(&[c], means(&m, 1e9, 1e9), dear), None);
    }

    /// The ladders fill while a unit earns: the second ask is dearer than
    /// the second bid, so it stops there; the hold and the wallet bound it.
    #[test]
    fn the_fill_stops_where_a_unit_stops_earning_and_fits_hold_and_wallet() {
        let m = map();
        let c = cand(
            GOOD,
            vec![
                order(1, NEAR, 10.0, 5, false),
                order(2, NEAR, 30.0, 100, false),
            ],
            vec![order(3, FAR, 40.0, 8, true), order(4, FAR, 20.0, 100, true)],
        );
        let d = best_deal(std::slice::from_ref(&c), means(&m, 1e9, 1e9), FREE).expect("a deal");
        assert_eq!(d.units, 8, "5 at 10 and 3 at 30 into 40; 30 into 20 loses");
        assert_eq!(d.max_ask, 30.0);
        assert_eq!(d.min_bid, 40.0);

        let small = Candidate {
            volume: 2.0,
            ..c.clone()
        };
        let d = best_deal(&[small], means(&m, 9.0, 1e9), FREE).expect("a deal");
        assert_eq!(d.units, 4, "9 m3 of hold holds four 2 m3 units");

        let budget = 70.0 / WALLET_SHARE;
        let d = best_deal(&[c], means(&m, 1e9, budget), FREE).expect("a deal");
        assert_eq!(
            d.units, 5,
            "five at 10 spend 50; a sixth at 30 is over the share"
        );
    }

    /// Of two deals the more profitable wins; an NPC side is capped by its
    /// stock (its `vol_remaining`); a station off the map is never used.
    #[test]
    fn the_best_deal_is_the_most_profit_and_never_off_the_map() {
        let m = map();
        let thin = cand(
            GOOD,
            vec![order(1, NEAR, 100.0, 10, false)],
            vec![order(2, FAR, 110.0, 10, true)],
        );
        let fat = cand(
            OTHER,
            vec![
                order(3, HOME, 100.0, 4, false),
                order(4, 69_100_404, 1.0, 1_000, false),
            ],
            vec![order(5, NEAR, 200.0, 1_000, true)],
        );
        let d = best_deal(&[thin, fat], means(&m, 1e9, 1e9), FREE).expect("a deal");
        assert_eq!(d.type_id, OTHER);
        assert_eq!(d.buy_station, HOME);
        assert_eq!(d.units, 4, "the ask's stock caps it");
    }

    /// Listing at home: with no bid anywhere, goods bought where they are
    /// cheap are listed at `P_mkt` after tax and the broker's fee, sized to
    /// the region's sales; never when home already asks that or less.
    #[test]
    fn a_listing_at_home_needs_p_mkt_sales_and_no_cheaper_ask_at_home() {
        let m = map();
        let mut c = cand(GOOD, vec![order(1, NEAR, 50.0, 1_000, false)], vec![]);
        c.p_mkt = Some(100.0);
        c.daily_volume = 10.0;
        let costs = Costs {
            sales_tax: 0.1,
            broker_fee: 0.1,
            ..FREE
        };
        let d = best_deal(std::slice::from_ref(&c), means(&m, 1e9, 1e9), costs).expect("a listing");
        assert_eq!(d.outlet, Outlet::List);
        assert_eq!(d.sell_station, HOME);
        assert_eq!(d.units, (10.0 * LIST_DAYS_OF_VOLUME) as i64);
        assert!((d.revenue - d.units as f64 * 80.0).abs() < 1e-6);

        let mut undercut = c.clone();
        undercut.book.asks.push(order(2, HOME, 99.0, 1, false));
        let d = best_deal(&[undercut], means(&m, 1e9, 1e9), costs);
        assert!(
            d.is_none_or(|d| d.outlet != Outlet::List),
            "home asks under P_mkt: {d:?}"
        );

        c.daily_volume = 0.0;
        assert_eq!(best_deal(&[c], means(&m, 1e9, 1e9), costs), None);
    }

    /// The margin and the floor: a deal must clear both.
    #[test]
    fn a_deal_must_clear_the_margin_and_the_least_profit() {
        let m = map();
        let c = cand(
            GOOD,
            vec![order(1, NEAR, 100.0, 10, false)],
            vec![order(2, NEAR, 104.0, 10, true)],
        );
        let d = best_deal(std::slice::from_ref(&c), means(&m, 1e9, 1e9), FREE).expect("40 ISK");
        assert_eq!((d.buy_station, d.sell_station, d.jumps), (NEAR, NEAR, 2));
        let margin = Costs {
            margin: 0.05,
            ..FREE
        };
        assert_eq!(
            best_deal(std::slice::from_ref(&c), means(&m, 1e9, 1e9), margin),
            None
        );
        let floor = Costs {
            min_profit: 41.0,
            ..FREE
        };
        assert_eq!(best_deal(&[c], means(&m, 1e9, 1e9), floor), None);
    }

    #[test]
    fn the_probe_window_rotates_and_wraps() {
        let t = [1, 2, 3, 4, 5];
        assert_eq!(probe_window(&t, 0, 2), vec![1, 2]);
        assert_eq!(probe_window(&t, 4, 3), vec![5, 1, 2]);
        assert_eq!(probe_window(&t, 7, 9), vec![3, 4, 5, 1, 2]);
        assert!(probe_window(&[], 3, 2).is_empty());
    }

    fn offer(id: i64, start: i64, end: i64, reward: f64) -> Offer {
        Offer {
            contract_id: id,
            issuer_id: 90_100_001,
            start_station: start,
            end_station: end,
            reward,
            collateral: 0.0,
            volume: 10.0,
        }
    }

    /// The courier: the best reward net of the jumps; never one's own,
    /// never one that does not fit, never collateral past the share.
    #[test]
    fn the_best_courier_fits_the_hold_and_the_collateral_share() {
        let m = map();
        let costs = Costs {
            isk_per_jump: 10.0,
            ..FREE
        };
        let offers = [
            offer(1, NEAR, FAR, 100.0),
            offer(2, HOME, NEAR, 90.0),
            Offer {
                volume: 1_000.0,
                ..offer(3, HOME, NEAR, 1_000.0)
            },
            Offer {
                collateral: 10_000.0,
                ..offer(4, HOME, NEAR, 1_000.0)
            },
            Offer {
                issuer_id: 90_100_009,
                ..offer(5, HOME, NEAR, 1_000.0)
            },
            offer(6, HOME, 69_100_404, 1_000.0),
        ];
        let k = best_courier(&offers, 90_100_009, means(&m, 100.0, 1_000.0), costs).expect("one");
        assert_eq!(k.offer.contract_id, 2, "90 - 2 jumps beats 100 - 4 jumps");
        assert_eq!(k.jumps, 2);
        assert_eq!(k.profit, 70.0);
    }

    /// `SearchContracts`' answer reads to its outstanding couriers.
    #[test]
    fn offers_read_from_a_search_answer() {
        use eve_server::pyutil::keyval;
        let row = |id: i64, kind: i64, status: i64| {
            keyval(vec![(
                "contract",
                keyval(vec![
                    ("contractID", PyValue::Int(id)),
                    ("type", PyValue::Int(kind)),
                    ("status", PyValue::Int(status)),
                    ("issuerID", PyValue::Int(90_100_001)),
                    ("startStationID", PyValue::Int(NEAR)),
                    ("endStationID", PyValue::Int(FAR)),
                    ("reward", PyValue::Real(5_000.0)),
                    ("collateral", PyValue::Real(1_000.0)),
                    ("volume", PyValue::Real(12.5)),
                ]),
            )])
        };
        let answer = keyval(vec![(
            "contracts",
            PyValue::List(vec![
                row(7, CONTRACT_COURIER, STATUS_OUTSTANDING),
                row(8, 1, STATUS_OUTSTANDING),
                row(9, CONTRACT_COURIER, STATUS_FINISHED),
            ]),
        )]);
        let got = offers_of(&answer);
        assert_eq!(
            got,
            vec![Offer {
                contract_id: 7,
                issuer_id: 90_100_001,
                start_station: NEAR,
                end_station: FAR,
                reward: 5_000.0,
                collateral: 1_000.0,
                volume: 12.5,
            }]
        );
    }

    // ---- X-TRADE: the skills, the plan, the profit per route shape --------

    use eve_server::econ_exchange::{TIER_A_BID, TIER_A_STOCK, TRADE_GOOD_ASK};

    /// The default costs of a pilot with these two skills.
    fn skilled(accounting: i64, broker_relations: i64) -> Costs {
        Costs::at_skills(&TradeSkills {
            accounting,
            broker_relations,
            ..TradeSkills::default()
        })
    }

    /// Costs for isolating the tax: the skills' rates, free trips.
    fn taxes_only(accounting: i64, broker_relations: i64) -> Costs {
        let k = skilled(accounting, broker_relations);
        Costs {
            isk_per_jump: 0.0,
            margin: 0.0,
            min_profit: 1.0,
            ..k
        }
    }

    /// **Route shape 1, regional arbitrage** (a player ask, a bid one
    /// station on): the spread the 8 % tax eats at Accounting 0 pays at
    /// Accounting IV, and the profit is exactly the units times the net.
    #[test]
    fn arbitrage_into_a_bid_loses_at_accounting_0_and_pays_at_iv() {
        let m = map();
        let (ask, bid, stock) = (94.0, 100.0, 1_000);
        let c = cand(
            GOOD,
            vec![order(1, NEAR, ask, stock, false)],
            vec![order(2, FAR, bid, stock, true)],
        );
        let go = |acc, br| {
            best_deal(
                std::slice::from_ref(&c),
                means(&m, 1e9, 1e9),
                taxes_only(acc, br),
            )
        };
        assert_eq!(go(0, 0), None, "100 x 0.92 = 92 < 94");
        let d = go(4, 0).expect("100 x 0.9552 = 95.52 > 94");
        let tax = eve_server::market_fees::sales_tax_rate(4);
        assert_eq!(d.units, stock);
        assert!((d.profit - stock as f64 * (bid * (1.0 - tax) - ask)).abs() < 1e-6);
        assert!(go(5, 0).expect("more").profit > d.profit);
        // Broker Relations does nothing for a sale into a bid.
        assert_eq!(go(4, 5).expect("same").profit, d.profit);
    }

    /// **Route shape 2, a listing at home** (bought cheap, listed at
    /// `P_mkt`): the broker's fee is in the price, so Broker Relations
    /// moves a thin listing across the line that Accounting alone leaves.
    #[test]
    fn a_listing_pays_the_brokers_fee_so_broker_relations_counts() {
        let m = map();
        let mut c = cand(GOOD, vec![order(1, NEAR, 93.0, 1_000, false)], vec![]);
        c.p_mkt = Some(100.0);
        c.daily_volume = 10_000.0;
        let go = |acc, br| {
            best_deal(
                std::slice::from_ref(&c),
                means(&m, 1e9, 1e9),
                taxes_only(acc, br),
            )
        };
        assert_eq!(go(0, 0), None, "100 x (1 - .08 - .03) = 89 < 93");
        assert_eq!(go(4, 0), None, "100 x (1 - .0448 - .03) = 92.52 < 93");
        let d = go(4, 4).expect("100 x (1 - .0448 - .018) = 93.72 > 93");
        assert_eq!(d.outlet, Outlet::List);
        let rates = Rates::at(
            &TradeSkills {
                accounting: 4,
                broker_relations: 4,
                ..TradeSkills::default()
            },
            Standings::default(),
        );
        // Whatever the units, the revenue is the listing's net of both.
        assert!((d.revenue - d.units as f64 * rates.net_of_listing(100.0)).abs() < 1e-6);
        assert!(d.revenue > d.spend, "net-positive: {d:?}");
    }

    /// The listing's minimum fee is in the profit: a tiny listing whose
    /// rate-fee is under `mktMinimumFee` pays the minimum.
    #[test]
    fn a_tiny_listing_pays_the_minimum_fee() {
        let m = map();
        let mut c = cand(GOOD, vec![order(1, NEAR, 10.0, 1, false)], vec![]);
        c.p_mkt = Some(20.0);
        c.daily_volume = 1_000.0;
        let costs = Costs {
            sales_tax: 0.0,
            broker_fee: 0.01,
            ..FREE
        };
        // 20 ISK listed, fee floor 100: a 10 ISK profit becomes a loss.
        assert_eq!(best_deal(&[c], means(&m, 1e9, 1e9), costs), None);
    }

    /// **Route shape 3, the NPC trade-goods loop, and the econ check**: NPC
    /// asks at [`TRADE_GOOD_ASK`] x base and bids at [`TIER_A_BID`] x base,
    /// stock [`TIER_A_STOCK`] a side, the bid one jump from the ask's station
    /// (home): two jumps there and back. At Accounting 0 the loop loses on
    /// every unit and the trader takes nothing; at Accounting IV it makes
    /// money, by exactly the net of the tax -- with the default (realistic)
    /// jump cost and floors.
    #[test]
    fn the_npc_trade_goods_loop_loses_at_accounting_0_and_makes_money_at_iv() {
        let base = 10_000.0;
        let (ask, bid) = (base * TRADE_GOOD_ASK, base * TIER_A_BID);
        let m = map();
        let c = cand(
            GOOD,
            vec![order(1, HOME, ask, TIER_A_STOCK, false)],
            vec![order(2, NEAR, bid, TIER_A_STOCK, true)],
        );
        let wallet = 100.0 * ask * TIER_A_STOCK as f64;
        let go = |acc| {
            best_deal(
                std::slice::from_ref(&c),
                means(&m, 1e9, wallet),
                skilled(acc, 0),
            )
        };

        // The loop, forced, at Accounting 0: every unit loses ISK.
        let r0 = Rates::at(&TradeSkills::default(), Standings::default());
        assert!(
            r0.net_of_sale(bid) < ask,
            "{} vs {ask}",
            r0.net_of_sale(bid)
        );
        assert_eq!(go(0), None, "a trader does not bleed ISK");

        let r4 = Rates::at(
            &TradeSkills {
                accounting: 4,
                ..TradeSkills::default()
            },
            Standings::default(),
        );
        assert!(r4.net_of_sale(bid) > ask);
        let d = go(4).expect("Accounting IV makes the loop pay");
        assert_eq!(d.units, TIER_A_STOCK, "capped by the NPC stock");
        let want =
            TIER_A_STOCK as f64 * (r4.net_of_sale(bid) - ask) - f64::from(d.jumps) * ISK_PER_JUMP;
        assert!((d.profit - want).abs() < 1e-6, "{} vs {want}", d.profit);
        assert!(d.profit > MIN_PROFIT);
        // The wallet after the trip is up by that, jumps aside.
        assert!(d.revenue - d.spend > 0.0);
        assert!(go(5).expect("V").profit > d.profit);
    }

    /// The plan: prerequisites first, the wallet gates, nothing past
    /// [`LEARN_PER_CYCLE`], and a step whose prerequisite book is not yet
    /// injected waits (so a failed purchase cannot poison the queue).
    #[test]
    fn the_plan_stages_prerequisites_first_and_only_what_the_wallet_allows() {
        // Prerequisites as the skill table would say (invented levels).
        let requires = |s: i64| -> Vec<(i64, i64)> {
            match s {
                TYPE_ACCOUNTING => vec![(TYPE_TRADE, 4)],
                TYPE_BROKER_RELATIONS => vec![(TYPE_TRADE, 2)],
                TYPE_MARGIN_TRADING => vec![(TYPE_BROKER_RELATIONS, 4), (TYPE_ACCOUNTING, 4)],
                _ => Vec::new(),
            }
        };
        let none = BTreeMap::new();
        // A fresh pilot: Trade first; Accounting and Broker Relations wait
        // for it (its book is not even injected yet).
        assert_eq!(plan_stage(&none, 1.0e6, requires), vec![(TYPE_TRADE, 4)]);
        // Trade injected but untrained: the rest may queue behind it, and
        // a skill's later levels join its first step, in plan order.
        let injected = BTreeMap::from([(TYPE_TRADE, 0)]);
        assert_eq!(
            plan_stage(&injected, 1.0e6, requires),
            vec![
                (TYPE_TRADE, 4),
                (TYPE_ACCOUNTING, 5),
                (TYPE_BROKER_RELATIONS, 5)
            ]
        );
        // Trade IV trained: the tax steps alone.
        let trade4 = BTreeMap::from([(TYPE_TRADE, 4)]);
        assert_eq!(
            plan_stage(&trade4, 0.0, requires),
            vec![(TYPE_ACCOUNTING, 5), (TYPE_BROKER_RELATIONS, 5)]
        );
        // Everything in the first tier done: a poor wallet stops, a richer
        // one goes on to Retail.
        let done = BTreeMap::from([
            (TYPE_TRADE, 4),
            (TYPE_ACCOUNTING, 5),
            (TYPE_BROKER_RELATIONS, 5),
        ]);
        assert!(plan_stage(&done, 1_000.0, requires).is_empty());
        let rich = plan_stage(&done, 3.0e6, requires);
        assert!(rich.contains(&(TYPE_RETAIL, 4)), "{rich:?}");
        // Margin Trading needs Accounting IV and Broker Relations IV trained.
        assert!(rich.contains(&(TYPE_MARGIN_TRADING, 4)), "{rich:?}");
        // A pilot who has it all: nothing to learn.
        let all: BTreeMap<i64, i64> = SKILL_PLAN.iter().map(|p| (p.skill, 5)).collect();
        assert!(plan_stage(&all, f64::MAX, requires).is_empty());
    }

    /// The plan lists every skill the market reads that a trader can use,
    /// each at most once per level, and never asks for more than V.
    #[test]
    fn the_plan_is_well_formed() {
        for p in SKILL_PLAN {
            assert!((1..=5).contains(&p.level), "{p:?}");
        }
        for pair in SKILL_PLAN.windows(2) {
            assert!(pair[0].wallet <= pair[1].wallet, "wallet gates only rise");
        }
        for want in [
            TYPE_TRADE,
            TYPE_ACCOUNTING,
            TYPE_BROKER_RELATIONS,
            TYPE_MARGIN_TRADING,
            TYPE_MARKETING,
            TYPE_PROCUREMENT,
            TYPE_DAYTRADING,
            TYPE_VISIBILITY,
            TYPE_RETAIL,
            TYPE_WHOLESALE,
            TYPE_TYCOON,
        ] {
            assert!(SKILL_PLAN.iter().any(|p| p.skill == want), "{want}");
        }
    }

    /// Standings with the station owner lower only the listing's fee.
    #[test]
    fn costs_follow_the_rates() {
        let k = Costs::at_skills(&TradeSkills::default());
        assert_eq!(k, Costs::default());
        let liked = Costs::default().with_rates(Rates::at(
            &TradeSkills::default(),
            Standings {
                faction: 10.0,
                corp: 10.0,
            },
        ));
        assert!(liked.broker_fee < Costs::default().broker_fee);
        assert_eq!(liked.sales_tax, Costs::default().sales_tax);
    }

    // ---- FE-TRADE: regions, memory, the watch set ----------------------------

    const RA: i64 = 10_000_801;
    const RB: i64 = 10_000_802;

    /// `map()` with HOME and NEAR in region A and FAR in region B.
    fn regional() -> Map {
        map()
            .with_region(S1, RA)
            .with_region(S2, RA)
            .with_region(S3, RB)
    }

    /// A desk that has been to region B and seen `bid` at FAR for `units`
    /// on day 0.
    fn desk_with_bid(bid: f64, units: i64) -> Desk {
        let mut d = Desk::default();
        let seen = Book {
            asks: vec![],
            bids: vec![order(9, FAR, bid, units, true)],
        };
        d.memory.record(RB, GOOD, &seen, 0);
        d
    }

    fn knobs(risk: f64) -> crate::speculate::SpecKnobs {
        crate::speculate::SpecKnobs {
            region_risk: risk,
            ..crate::speculate::SpecKnobs::NORMAL
        }
    }

    /// The home region asks 100 at NEAR; region B was seen bidding 150 at
    /// FAR: bought here, carried over the border, sold into the remembered bid.
    #[test]
    fn a_remembered_bid_in_another_region_makes_a_remote_deal() {
        let m = regional();
        let c = cand(GOOD, vec![order(1, NEAR, 100.0, 50, false)], vec![]);
        let d = desk_with_bid(150.0, 30);
        let deal = best_remote(&[c], &d, RA, 0, means(&m, 1e9, 1e9), FREE).expect("a deal");
        assert_eq!(deal.outlet, Outlet::Remote);
        assert_eq!((deal.buy_station, deal.sell_station), (NEAR, FAR));
        assert_eq!(deal.units, 30, "capped by the remembered depth");
        assert_eq!(deal.jumps, 1 + 1 + 2);
        assert_eq!(deal.profit, 30.0 * 50.0);
    }

    /// No memory of another region, no deal: the trader is not omniscient.
    #[test]
    fn a_region_never_visited_is_no_outlet() {
        let m = regional();
        let c = cand(GOOD, vec![order(1, NEAR, 100.0, 50, false)], vec![]);
        assert_eq!(
            best_remote(&[c], &Desk::default(), RA, 0, means(&m, 1e9, 1e9), FREE),
            None
        );
    }

    /// Memory fades: the same bid, ten days on, is worth less, and past the
    /// longest memory it is gone.
    #[test]
    fn an_old_memory_is_discounted_and_then_forgotten() {
        let m = regional();
        let c = cand(GOOD, vec![order(1, NEAR, 100.0, 50, false)], vec![]);
        let d = desk_with_bid(150.0, 30);
        let fresh = best_remote(
            std::slice::from_ref(&c),
            &d,
            RA,
            0,
            means(&m, 1e9, 1e9),
            FREE,
        )
        .expect("fresh");
        let aged = best_remote(
            std::slice::from_ref(&c),
            &d,
            RA,
            5,
            means(&m, 1e9, 1e9),
            FREE,
        )
        .expect("aged");
        assert!(aged.profit < fresh.profit && aged.min_bid < fresh.min_bid);
        assert_eq!(
            best_remote(
                &[c],
                &d,
                RA,
                crate::price_memory::MEMORY_MAX_DAYS + 1,
                means(&m, 1e9, 1e9),
                FREE
            ),
            None
        );
    }

    /// A spread that beats the usual margin but not the risk margin on top is
    /// left alone; the jumps count too.
    #[test]
    fn a_thin_spread_does_not_beat_the_round_trip_and_the_risk() {
        let m = regional();
        let c = cand(GOOD, vec![order(1, NEAR, 100.0, 50, false)], vec![]);
        let mut d = desk_with_bid(104.0, 50);
        d.knobs = knobs(0.05);
        let costs = Costs {
            margin: 0.01,
            ..FREE
        };
        assert_eq!(
            best_remote(
                std::slice::from_ref(&c),
                &d,
                RA,
                0,
                means(&m, 1e9, 1e9),
                costs
            ),
            None
        );
        d.knobs = knobs(0.0);
        let deal = best_remote(
            std::slice::from_ref(&c),
            &d,
            RA,
            0,
            means(&m, 1e9, 1e9),
            costs,
        )
        .expect("without the risk margin 4 % clears 1 %");
        // The jumps eat it too.
        let dear = Costs {
            isk_per_jump: deal.profit / f64::from(deal.jumps) + 1.0,
            ..costs
        };
        assert_eq!(
            best_remote(&[c], &d, RA, 0, means(&m, 1e9, 1e9), dear),
            None
        );
    }

    /// The trip may not pass `region_max_jumps`.
    #[test]
    fn the_regional_trip_stays_inside_the_jump_limit() {
        let m = regional();
        let c = cand(GOOD, vec![order(1, NEAR, 100.0, 50, false)], vec![]);
        let mut d = desk_with_bid(150.0, 30);
        d.knobs = crate::speculate::SpecKnobs {
            region_max_jumps: 3,
            ..d.knobs
        };
        assert_eq!(
            best_remote(&[c], &d, RA, 0, means(&m, 1e9, 1e9), FREE),
            None
        );
    }

    /// This region's own memory is not another region's outlet.
    #[test]
    fn the_regions_own_memory_is_not_a_remote_outlet() {
        let m = regional();
        let c = cand(GOOD, vec![order(1, NEAR, 100.0, 50, false)], vec![]);
        let mut d = Desk::default();
        d.memory.record(
            RA,
            GOOD,
            &Book {
                asks: vec![],
                bids: vec![order(9, HOME, 500.0, 50, true)],
            },
            0,
        );
        assert_eq!(
            best_remote(&[c], &d, RA, 0, means(&m, 1e9, 1e9), FREE),
            None
        );
    }

    /// An NPC ask at 0.94 x base into a remembered NPC bid at 0.92 x base
    /// loses at the 8 % tax (0.92 x 0.92 < 0.94): the loop is not taken
    /// across a border either.
    #[test]
    fn loop_does_not_buy_npc_ask_at_a_loss() {
        let m = regional();
        let base = 10_000.0;
        let c = cand(GOOD, vec![order(1, NEAR, 0.94 * base, 1000, false)], vec![]);
        let d = desk_with_bid(0.92 * base, 1000);
        let taxed = Costs {
            sales_tax: SALES_TAX,
            ..FREE
        };
        assert_eq!(
            best_remote(&[c], &d, RA, 0, means(&m, 1e12, 1e12), taxed),
            None
        );
    }

    fn typed(rows: &[(i64, i64, i64, bool)]) -> Content {
        let mut text = eve_server::typedata::FIXTURE.to_owned();
        for (id, group, category, published) in rows {
            text.push_str(&format!(
                "type\t{id}\t{group}\t{category}\t{}\t1\t1.0\t0.0\t1.0\t1.0\t0\t\t\tInvented {id}\n",
                i32::from(*published)
            ));
        }
        let td = eve_server::typedata::Table::parse(&text).expect("rows parse");
        let mut c = Content::fixture();
        c.insert("typedata", eve_server::typedata::Installed::build(td));
        c
    }

    /// The watch set holds the trade goods, the minerals and the dearest
    /// hulls under the ceiling (not the capital), modules and charges.
    #[test]
    fn watch_set_includes_dear_hulls() {
        let (good, mineral) = (990_600_001, 990_600_002);
        let ships: Vec<i64> = (0..8).map(|i| 990_610_000 + i).collect();
        let capital = 990_619_999;
        let module = 990_620_001;
        let mut rows = vec![
            (good, TRADE_GOOD_GROUPS[0], CATEGORY_COMMODITY, true),
            (mineral, GROUP_MINERAL, CATEGORY_MATERIAL, true),
            (capital, 99_001, CATEGORY_SHIP, true),
            (module, 99_002, CATEGORY_MODULE, true),
        ];
        rows.extend(ships.iter().map(|s| (*s, 99_001, CATEGORY_SHIP, true)));
        let content = typed(&rows);
        let mut index: Vec<(i64, f64)> = ships
            .iter()
            .enumerate()
            .map(|(i, s)| (*s, 1_000_000.0 * (i as f64 + 1.0)))
            .collect();
        index.push((capital, 10.0 * WATCH_HULL_CEILING));
        index.push((module, 5_000.0));
        let w = watch_set(&content, &index);
        assert!(w.contains(&good) && w.contains(&mineral) && w.contains(&module));
        assert!(!w.contains(&capital), "a capital has no book to read");
        let hulls: Vec<_> = w.iter().filter(|t| ships.contains(t)).collect();
        assert_eq!(hulls.len(), WATCH_HULLS);
        // The dearest ones: the two cheapest of eight are out.
        assert!(!w.contains(&ships[0]) && !w.contains(&ships[1]));
        assert!(w.contains(&ships[7]));
        assert_eq!(w, {
            let mut sorted = w.clone();
            sorted.sort_unstable();
            sorted.dedup();
            sorted
        });
    }
}
