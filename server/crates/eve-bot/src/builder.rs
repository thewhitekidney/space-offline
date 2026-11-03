//! **The bot market roles** (W16 packet CP47) and **the builder's industry
//! act** (R-ECON §3 row E9): the builder's row, and the market cycles a
//! builder and a miner run over the real `marketProxy` and `industryManager`
//! verbs ([`Conn::sell`], [`Conn::buy`], [`Conn::market_orders`],
//! [`Conn::modify_order`], [`Conn::install_job`], [`Conn::complete_job`]).
//!
//! Spec: C5 §7.2 (`docs/content/C5-economy.md`), R-ECON §2.2, §2.4 and §2.5
//! (pricing, `crate::pricing`), plan CA19. Lane notes: `docs/lanes/cp47.md`,
//! `docs/lanes/e9.md`, `docs/lanes/e10.md`.
//!
//! - **The builder** ([`run`]), docked, once a cycle:
//!   1. **delivers** every job whose end time has passed
//!      (`GetJobsByOwner`, `CompleteJob`);
//!   2. **lists** each product stack ([`is_product`]) at the seller's ask
//!      ([`crate::pricing::ask`], E10: `P_mkt` less the undercut, never under
//!      its materials' cost plus the margin, inside the clamp, a step lower a
//!      day while its stock exceeds the region's sales): **one ask per
//!      (type, station)**, repriced with `ModifyCharOrder` when its price is
//!      off, never inside the server's modification delay ([`reprice`]);
//!   3. **climbs the blueprint ladder** ([`BLUEPRINT_RUNGS`], SOC-B7: each
//!      rung a set of interchangeable originals): for the first rung it
//!      lacks that a seller in reach has, it buys the cheapest of the rung
//!      asked at its station (under O5-a the NPC sells originals at its
//!      corporations' stations, E5) under the consumer's cap
//!      ([`crate::pricing::reservation`]), else fetches the nearest one
//!      ([`crate::travel`]); the first with any wallet, the next only with
//!      [`CLIMB_CUSHION`] x its price ([`may_climb`]);
//!   4. **builds**: an idle Tech II copy first, when the hangar (or the
//!      asks here, all of it) covers a run ([`t2_candidates`], SOC-B7);
//!      else for its best idle original ([`candidates`]) it buys the
//!      materials from asks at its station at no more than the margin rule's
//!      price ([`margin_bids`]), installs a job of the runs the hangar covers
//!      (`InstallJob`), and otherwise rests **one standing bid per (type,
//!      station)** for what is short, at the margin rule's price, repriced
//!      as the asks are;
//!   5. **the laboratory** (SOC-B7, with [`INVENTION_MIN_WALLET`]): buys a
//!      batch of datacores from asks here, then copies an original with an
//!      invention or invents from the copy ([`plan_lab`],
//!      [`install_lab_job`]);
//!   6. buys the minerals it `wants` from player asks with at most
//!      [`WALLET_SHARE`] of its wallet a cycle ([`buy_plan`]). The demand
//!      budget that once bounded this is the server's since E11 (a bot's
//!      fill on a human's ask stops at `BOT_DEMAND_PER_HUMAN`), so the bot
//!      keeps none (SOC-A2).
//! - **The miner's sale** ([`sell_goods`]) sells raw materials into bids at
//!   the NPC floor or better, else at the clamp's low end
//!   ([`crate::pricing::floor`]): the market fills the best bid first, so a
//!   builder's standing bid wins.
//!
//! **Prices** are [`crate::pricing`]'s (E10): `P_mkt` from the region's
//! history once a type has traded long enough, else `P_ref`, read once a
//! cycle ([`Prices`]).
//! - **Bot ISK is fuel** (C5 §7.2), and the server holds the line: since
//!   E11 the market clamps a bot's fill on a human's ask to what is left of
//!   that human's daily demand budget. The wire cannot tell a human's ask
//!   from a bot's, which is why the bot never could; bot-to-bot trades are
//!   not limited. A `wants` buy and the industry act's material buys are
//!   each bounded by [`WALLET_SHARE`] of the wallet **[A]** (falsified by
//!   builders that stall on `NotEnoughMoney` for their install fee).
//!
//! **The recipes** are the server's own `industry::Table` in the bundle
//! (`eve-bots --content`, [`recipes_of`]), the data a client reads from its
//! `blueprints.static`; without one the builder builds nothing and only
//! trades.
//!
//! **The driver acts** (FX6): the row docked asks for [`Act::Market`], one
//! [`run`] every [`MARKET_EVERY`] (jittered), and the miner's row asks for
//! [`Act::Sell`] once a visit when its hangar holds goods
//! ([`is_miner_goods`]); the sale refines the ore first, at the station's
//! refinery, the way C5 §7.2's miner does.
//!
//! **Every rule here is [A] policy**, what a player would do, except the
//! client's own call shapes, which the verbs cite.

use crate::brain::{self, Act, Memory, Observation};
use crate::pricing::{self, Prices, Quote};
use crate::roles::{Lacks, RoleSpec, Summary};
pub use crate::verbs::{BlueprintRow, Book, BookOrder, JobRow, OwnOrder, RANGE_STATION, SellLine};
use crate::{Conn, Error, ItemRow, Result};
use eve_marshal::PyValue;
use eve_server::econ_exchange::{CEILING_CATEGORIES, FLOOR_GROUPS};
use eve_server::industry::{self, Blueprint};
use std::collections::BTreeMap;
use std::time::Duration;

/// The `--loop builder` row: a civilian.
pub static ROLE: RoleSpec = RoleSpec {
    name: "builder",
    greeting: "o7 -- building for the market",
    civilian: true,
    docked,
    in_space,
    banner,
    setup,
};

/// How often a builder runs one [`run`] cycle, docked: a range, jittered
/// per bot **[A]** (a player who trades checks the market now and then;
/// falsified by asks that sit stale for an hour after a sale, or by `[live]`
/// showing market calls crowding human turns).
pub const MARKET_EVERY: (Duration, Duration) =
    (Duration::from_secs(5 * 60), Duration::from_secs(15 * 60));
/// How many units of each `wants` mineral a builder asks for in one cycle
/// **[A]** (a T1 frigate's worth, about; falsified by builders that hoard
/// minerals their jobs never use).
pub const WANT_UNITS: i64 = 10_000;
/// `groupMineral` **[V]**: the first of `econ_exchange::FLOOR_GROUPS`.
pub const GROUP_MINERAL: i64 = 18;

/// The market's price step, one cent ([`pricing::TICK`]). The builder sits
/// at least one step under an NPC ceiling.
pub const TICK: f64 = pricing::TICK;
/// With no NPC floor for a type, the least the miner takes, as a fraction
/// of `P_ref`: the bot price clamp's low end ([`pricing::BOT_PRICE_MIN`]).
pub const SELL_FLOOR_OF_REF: f64 = pricing::BOT_PRICE_MIN;
/// How long a builder's ask rests, days: one of the Sell window's
/// durations **[A]**.
pub const ORDER_DAYS: i64 = 14;
/// `expectedBrokersFee`, the fraction a bot sends ([`pricing::BROKER_FEE`]).
pub const BROKER_FEE: f64 = pricing::BROKER_FEE;
/// `categoryAsteroid` **[V]**: ore, which a miner sells as it comes.
pub const CATEGORY_ASTEROID: i64 = 25;

// ---- the industry act's rules (E9) -----------------------------------------------

/// **The blueprint ladder** (R-ECON §2.2, §2.4; SOC-B7): the rungs a
/// builder climbs, in order, each a set of **interchangeable** originals --
/// it holds a rung when it holds any one of them, and buys whichever is
/// sold here or nearest within a trip (`climb`). The type ids are the
/// bundle's blueprints for those products **[V]**; the sets and the order
/// are **[A]** (falsified by a G3 soak whose builders sit on a rung no
/// station in reach sells, or never afford the next):
///
/// 0. ammo: EMP S;
/// 1. the miner's module: Miner I;
/// 2. a T1 frigate: Rifter, Slasher, Breacher, Punisher, Condor, Atron;
/// 3. a T1 destroyer: Thrasher, Coercer, Catalyst, Cormorant;
/// 4. a fuel block, from ice products and PI goods (B1, B6): the four
///    racial fuel blocks;
/// 5. a T1 cruiser: Rupture, Maller, Thorax, Moa;
/// 6. a T1 hauler: Mammoth, Bestower, Nereus, Badger;
/// 7. a light combat drone (ship-pool): Hobgoblin I, Acolyte I, Hornet I,
///    Warrior I, which the combat bots' bays take.
///
/// **Why sets** (measured against the bundle, `docs/lanes/soc-b7.md`): a
/// race's hull blueprints are sold only in that race's space, and the
/// market a bot reads is its own region's, so one global hull per rung
/// strands every other race's schools. With a set, every school reaches a
/// destroyer, a cruiser and a hauler of its own race within
/// [`crate::travel::TRIP_MAX_JUMPS`] in high-sec except where the note
/// says. **E5b's finding fixed:** the Venture rung is gone -- ORE sells its
/// blueprint only in null-sec, 21 to 32 jumps from every school, so no
/// builder at the high-sec floor could ever climb past it.
pub const BLUEPRINT_RUNGS: [&[i64]; 10] = [
    &[886],
    &[785],
    &[691, 689, 945, 944, 684, 955],
    &[16_243, 16_237, 16_241, 16_239],
    &[4_313, 4_314, 4_315, 4_316],
    &[974, 969, 972, 968],
    &[987, 1_945, 985, 983],
    &[2_455, 2_204, 2_465, 2_487],
    STARTER_AMMO,
    crate::rig::RIG_RUNG,
];
/// **The starter rung** (GB-FIX), last on the ladder and **built last**
/// ([`rank`]): small ammunition whose recipes take only Tritanium and
/// Pyerite -- Iron Charge S, Tungsten Charge S, Carbonized Lead S, Nuclear S
/// **[V]**. The two ores every school
/// system's belts hold yield exactly those two minerals, and every other
/// rung needs one they do not (EMP S a unit of Nocxium, the Miner I a
/// hundred Mexallon): with [`crate::supply::Supply`] ruling those out, this
/// is the rung a builder in such a system climbs, and the job it installs
/// is the loop's first. Cheap too (20 to 30 k ISK), so a 1 M ISK builder
/// holds one with its cushion. **[A]** that these four have an NPC seller
/// within a trip's reach of a school; falsified by a closed soak whose
/// builders report "no seller in reach" for this rung too.
pub const STARTER_AMMO: &[i64] = &[1_130, 1_131, 879, 880];
/// The index of [`STARTER_AMMO`] in [`BLUEPRINT_RUNGS`].
pub const STARTER_RUNG: usize = 8;
/// Every ladder blueprint, rung by rung ([`BLUEPRINT_RUNGS`] flattened):
/// what the signals price (`signal.rs`), and `BLUEPRINT_LADDER[0]` is the
/// first rung's one blueprint, which the tests name.
pub const BLUEPRINT_LADDER: [i64; 35] = [
    886, 785, 691, 689, 945, 944, 684, 955, 16_243, 16_237, 16_241, 16_239, 4_313, 4_314, 4_315,
    4_316, 974, 969, 972, 968, 987, 1_945, 985, 983, 2_455, 2_204, 2_465, 2_487, 1_130, 1_131, 879,
    880, 31_681, 30_988, 31_118,
];
/// A builder that already holds a blueprint buys the next rung only with
/// this many times its price in the wallet **[A]**: it keeps working capital
/// for materials and fees. Falsified by builders that buy a hull blueprint
/// and then cannot afford one run of it (too low), or sit on millions with
/// one ammo blueprint (too high).
pub const CLIMB_CUSHION: f64 = 3.0;
/// How long a job a builder installs, about **[A]**: a player sets a job to
/// end within a session. Runs = this / the blueprint's time, at least one.
/// Falsified by builders whose products reach the book hours after the
/// minerals (too long) or whose `[turns]` show installs crowding the market
/// (too short).
pub const JOB_TARGET: Duration = Duration::from_secs(2 * 60 * 60);
/// The most of its wallet one job's materials may take, escrowed bids
/// included **[A]**: the rest pays fees, taxes and the next rung. Falsified
/// by builders that stall on `NotEnoughMoney` for the install fee.
pub const WALLET_SHARE: f64 = 0.5;
/// **The margin** a builder keeps over its material cost
/// ([`pricing::MARGIN`]).
pub const MARGIN: f64 = pricing::MARGIN;
/// The sales tax on a sale ([`pricing::SALES_TAX`]).
pub const SALES_TAX: f64 = pricing::SALES_TAX;
/// Manufacturing slots a builder uses: a character's base slot
/// (`industry::CHARACTER_BASE_MANUFACTURE_SLOTS`) **[V]**; the slot skills
/// are not read **[A]** (falsified by a builder with Mass Production and an
/// idle second slot).
pub const MANUFACTURE_SLOTS: usize = industry::CHARACTER_BASE_MANUFACTURE_SLOTS as usize;
/// The skill a manufacturing job needs on every ladder rung: Industry I
/// (`industry::SKILL_INDUSTRY`) **[V]**. The builder buys and queues it the
/// way [`crate::acquire`] does when an install is refused.
pub const BUILDER_SKILLS: &[(i64, i64)] = &[(industry::SKILL_INDUSTRY, 1)];

// ---- invention (SOC-B7) ----------------------------------------------------------

/// Laboratory slots a builder uses for copying and invention: a
/// character's base slot (`industry::CHARACTER_BASE_LABORATORY_SLOTS`)
/// **[V]**; the laboratory skills are not read **[A]** (falsified by an
/// inventor with Laboratory Operation and an idle second slot).
pub const LAB_SLOTS: usize = industry::CHARACTER_BASE_LABORATORY_SLOTS as usize;
/// A builder invents only with at least this much in the wallet **[A]**:
/// invention is a bet (a third of the attempts succeed) whose copies cannot
/// be built until stage D, so only a builder with spare capital places it.
/// Falsified by builders that starve their manufacturing slot for
/// datacores (too low), or by rich builders whose labs stay idle while
/// datacores sit in their hangar (too high).
pub const INVENTION_MIN_WALLET: f64 = 5_000_000.0;
/// Attempts per invention job, and the runs a copy for invention is made
/// with **[A]**: one copy feeds one invention job of this many attempts.
/// Falsified by copies left with runs no datacore covers (too high), or by
/// a lab that copies after every job (too low).
pub const INVENTION_BATCH: i64 = 10;

// ---- the row --------------------------------------------------------------------

/// Docked: one [`Act::Market`] when its time has come, then wait for the
/// next ([`MARKET_EVERY`], jittered); give up and reconnect after repeated
/// failures, as every row does.
fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if o.failures.values().any(|&n| n >= brain::GIVE_UP_AFTER) {
        return vec![Act::Reconnect];
    }
    m.homing = None;
    if let Some(next) = m.market_next
        && o.now < next
    {
        return vec![Act::Wait(next - o.now)];
    }
    let next = o.now + m.jitter(MARKET_EVERY);
    m.market_next = Some(next);
    vec![Act::Market, Act::Wait(next - o.now)]
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

/// A builder trades docked and needs no hull, skill or rounds.
fn setup(_lacks: Lacks) -> Vec<String> {
    Vec::new()
}

// ---- pure rules ------------------------------------------------------------------

#[cfg(test)]
use pricing::down_to_cent;

/// **The builder's ask** for a type with nothing known but its reference
/// price `p_ref` (no history, no cost, no excess) under an NPC ceiling ask:
/// [`pricing::ask`]. Always strictly under the ceiling and inside the
/// clamp; `None` when no price is.
pub fn ask_price(p_ref: f64, ceiling: Option<f64>) -> Option<f64> {
    pricing::ask(&Quote {
        ceiling,
        ..Quote::at_ref(p_ref)
    })
}

/// **The miner's least price** for a stack: the NPC floor where the
/// exchange buys this type, else the clamp's low end on `P_ref`
/// ([`pricing::floor`]); `None` with neither (no sale).
pub fn sell_limit(p_ref: Option<f64>, book: &Book) -> Option<f64> {
    book.npc_floor()
        .or_else(|| p_ref.and_then(pricing::floor))
        .filter(|&p| p >= TICK)
}

/// A builder's product: a T1 good the exchange caps with a ceiling
/// (`econ_exchange::CEILING_CATEGORIES`), as a stack it can list.
pub fn is_product(row: &ItemRow) -> bool {
    !row.singleton && CEILING_CATEGORIES.contains(&row.category_id)
}

/// The miner's goods: raw materials (`econ_exchange::FLOOR_GROUPS`) and
/// ore, as stacks it can list.
pub fn is_miner_goods(row: &ItemRow) -> bool {
    !row.singleton && (FLOOR_GROUPS.contains(&row.group_id) || row.category_id == CATEGORY_ASTEROID)
}

/// **What the builder buys** of one mineral: player asks only (never an NPC
/// ask, C5 §7.2), cheapest first, at no more than `cap` per unit
/// ([`pricing::buy_cap`]), up to `want` units, and costing no more than
/// `isk` in all ([`WALLET_SHARE`] of the wallet). `(ask, units)` per order.
pub fn buy_plan(book: &Book, cap: f64, isk: f64, want: i64) -> Vec<(BookOrder, i64)> {
    let mut asks: Vec<&BookOrder> = book
        .asks
        .iter()
        .filter(|o| !o.is_npc() && o.price > 0.0 && o.price <= cap)
        .collect();
    asks.sort_by(|a, b| a.price.total_cmp(&b.price));
    let (mut want, mut isk) = (want, isk);
    let mut plan = Vec::new();
    for ask in asks {
        let afford = (isk / ask.price).floor() as i64;
        let units = want.min(ask.vol_remaining).min(afford);
        if units <= 0 {
            continue;
        }
        want -= units;
        isk -= units as f64 * ask.price;
        plan.push((*ask, units));
    }
    plan
}

// ---- the industry act's pure rules (E9) ------------------------------------------

/// What one run of `bp` may cost in materials under the margin rule, its
/// product at `product_price` ([`pricing::material_budget`]).
pub fn material_budget(bp: &Blueprint, product_price: f64) -> f64 {
    pricing::material_budget(bp, product_price)
}

/// **The margin rule's prices** for `bp`'s materials with nothing known but
/// `P_ref` ([`pricing::margin_bids`] with `P_mkt` = `P_ref`). `None`: nothing
/// to build at a profit.
pub fn margin_bids(bp: &Blueprint, prices: &BTreeMap<i64, f64>) -> Option<BTreeMap<i64, f64>> {
    pricing::margin_bids(bp, prices, prices)
}

/// The runs a builder plans for `bp` (an original): [`JOB_TARGET`] over its
/// time per run, at least one.
pub fn planned_runs(bp: &Blueprint) -> i64 {
    (JOB_TARGET.as_secs() as i64 / bp.time.max(1)).max(1)
}

/// One job's materials, `(typeID, units)`, at the blueprint's ME in an NPC
/// station (facility multiplier 1) -- the server's own formula
/// (`industry::job_materials`).
pub fn materials_for(bp: &Blueprint, runs: i64, material_efficiency: i64) -> Vec<(i64, i64)> {
    industry::job_materials(bp, runs, material_efficiency, 1.0)
        .into_iter()
        .map(|m| (m.type_id, m.quantity))
        .collect()
}

/// What the hangar lacks for `runs`: `(typeID, units)` with units > 0.
pub fn shortfall(
    bp: &Blueprint,
    runs: i64,
    material_efficiency: i64,
    held: &BTreeMap<i64, i64>,
) -> Vec<(i64, i64)> {
    materials_for(bp, runs, material_efficiency)
        .into_iter()
        .map(|(t, q)| (t, q - held.get(&t).copied().unwrap_or(0)))
        .filter(|&(_, short)| short > 0)
        .collect()
}

/// The most runs, up to `planned`, whose missing materials cost no more
/// than `isk` at `bids`; zero when not even one does.
pub fn affordable_runs(
    bp: &Blueprint,
    planned: i64,
    material_efficiency: i64,
    held: &BTreeMap<i64, i64>,
    bids: &BTreeMap<i64, f64>,
    isk: f64,
) -> i64 {
    let cost = |runs: i64| -> f64 {
        shortfall(bp, runs, material_efficiency, held)
            .iter()
            .map(|&(t, n)| n as f64 * bids.get(&t).copied().unwrap_or(f64::INFINITY))
            .sum::<f64>()
            * (1.0 + BROKER_FEE)
    };
    // The cost only grows with the runs: the largest that fits, by halving.
    let (mut lo, mut hi) = (0, planned.max(0));
    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        if cost(mid) <= isk {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// The most runs, up to `planned`, the hangar covers now.
pub fn runs_on_hand(
    bp: &Blueprint,
    planned: i64,
    material_efficiency: i64,
    held: &BTreeMap<i64, i64>,
) -> i64 {
    let (mut lo, mut hi) = (0, planned.max(0));
    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        if shortfall(bp, mid, material_efficiency, held).is_empty() {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// What to do with an open order whose price should be `target` at `now`
/// (a FILETIME).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reprice {
    /// Its price is the target already.
    Keep,
    /// Inside the server's modification delay: try again next cycle.
    Wait,
    /// Modify it to this price.
    Modify(f64),
}

/// **One order per (type, station), repriced, never stacked** (R-ECON
/// §2.5): an order off its target is modified, but only once the server's
/// `mktModificationDelay` has run since its last operation
/// ([`OwnOrder::modifiable_from`], E5), so the builder never earns an
/// `MktOrderDelay`.
pub fn reprice(own: &OwnOrder, target: f64, now: i64) -> Reprice {
    if (own.order.price - target).abs() < TICK / 2.0 {
        Reprice::Keep
    } else if now < own.modifiable_from() {
        Reprice::Wait
    } else {
        Reprice::Modify(target)
    }
}

/// The rungs still to climb, in order: each rung of `rungs` the builder
/// holds no blueprint of, as its blueprints that have a recipe in
/// `recipes` (a rung none of whose blueprints has one is left out).
pub fn lacking_rungs(rungs: &[&[i64]], held: &[i64], recipes: &industry::Table) -> Vec<Vec<i64>> {
    lacking_rungs_by(rungs, held, recipes, &|_| true)
}

/// [`lacking_rungs`] counting only the blueprints `makeable` accepts: a
/// rung's choices are the makeable ones, and a held blueprint that is not
/// makeable holds nothing (GB-FIX: a builder sitting on a blueprint its
/// system's miners cannot feed still lacks the rung).
pub fn lacking_rungs_by(
    rungs: &[&[i64]],
    held: &[i64],
    recipes: &industry::Table,
    makeable: &dyn Fn(i64) -> bool,
) -> Vec<Vec<i64>> {
    rungs
        .iter()
        .filter(|rung| !rung.iter().any(|&t| held.contains(&t) && makeable(t)))
        .map(|rung| {
            rung.iter()
                .copied()
                .filter(|&t| recipes.find(t).is_some() && makeable(t))
                .collect::<Vec<i64>>()
        })
        .filter(|choices| !choices.is_empty())
        .collect()
}

/// The ladder rung to buy next: the first of [`lacking_rungs`].
pub fn next_rung(rungs: &[&[i64]], held: &[i64], recipes: &industry::Table) -> Option<Vec<i64>> {
    lacking_rungs(rungs, held, recipes).into_iter().next()
}

/// A blueprint's rung on the ladder, if it is on it.
pub fn rung_of(type_id: i64) -> Option<usize> {
    BLUEPRINT_RUNGS.iter().position(|r| r.contains(&type_id))
}

/// May the builder pay `price` for its next rung with `wallet`? The first
/// blueprint with any wallet that covers it; a further one only with
/// [`CLIMB_CUSHION`] x its price.
pub fn may_climb(holds_any: bool, wallet: f64, price: f64) -> bool {
    if holds_any {
        wallet >= price * CLIMB_CUSHION
    } else {
        wallet >= price
    }
}

/// The idle originals worth building from, best first: the highest ladder
/// rung first, then any other blueprint with a recipe, by type id.
pub fn candidates<'a>(
    blueprints: &'a [BlueprintRow],
    station: i64,
    recipes: &industry::Table,
) -> Vec<&'a BlueprintRow> {
    let mut v: Vec<&BlueprintRow> = blueprints
        .iter()
        .filter(|b| {
            b.is_original()
                && b.job_id.is_none()
                && b.location_id == station
                && recipes.find(b.type_id).is_some()
        })
        .collect();
    v.sort_by_key(|b| (std::cmp::Reverse(rank(b.type_id)), b.type_id, b.item_id));
    v
}

/// **Demand first** (ship-pool): `candidates` with those whose product has
/// more standing bids than asks (`demand`, blueprint type to unmet units, a
/// combat bot's resting bid for a hull or a drone) moved to the front, the
/// ladder's order kept within each half.
pub fn promote_demanded<'a>(
    candidates: Vec<&'a BlueprintRow>,
    demand: &BTreeMap<i64, i64>,
) -> Vec<&'a BlueprintRow> {
    let mut v = candidates;
    v.sort_by_key(|b| demand.get(&b.type_id).copied().unwrap_or(0) <= 0);
    v
}

/// The ladder rank that orders candidates: the rung, `-1` off the ladder
/// and for the starter rung, which is what a builder falls back on.
fn rank(type_id: i64) -> i64 {
    match rung_of(type_id) {
        Some(STARTER_RUNG) | None => -1,
        Some(i) => i as i64,
    }
}

// ---- invention's pure rules (SOC-B7) --------------------------------------------

/// Every Tech II blueprint some invention makes: the products of every
/// `invention` activity in `recipes`, and the Tech III ones a relic's
/// invention makes ([`t3_types`]).
pub fn invented_types(recipes: &industry::Table) -> std::collections::BTreeSet<i64> {
    recipes
        .science
        .iter()
        .filter_map(|s| s.invention.as_ref())
        .flat_map(|inv| inv.products.iter().map(|p| p.type_id))
        .chain(t3_types(recipes))
        .collect()
}

/// Every Tech III blueprint (hull, subsystem, tactical destroyer): the
/// products of the invention of an ancient relic (T3-B, `recipes.relics`).
pub fn t3_types(recipes: &industry::Table) -> std::collections::BTreeSet<i64> {
    recipes
        .relics
        .iter()
        .filter_map(|r| r.science.invention.as_ref())
        .flat_map(|inv| inv.products.iter().map(|p| p.type_id))
        .collect()
}

/// Which T2 blueprint an invention aims at (`productTypeID`, which the
/// server needs when there are several): the first, in the client's order,
/// whose blueprint has a manufacturing recipe **[A]** (falsified by
/// inventors whose copies are all of the less valuable product; the price
/// could choose once T2 goods trade, stage D).
pub fn invention_product(inv: &industry::Invention, recipes: &industry::Table) -> Option<i64> {
    inv.products
        .iter()
        .map(|p| p.type_id)
        .find(|&t| recipes.find(t).is_some())
}

/// How many invention attempts the hangar's datacores cover, up to `most`.
pub fn datacore_sets(inv: &industry::Invention, held: &BTreeMap<i64, i64>, most: i64) -> i64 {
    inv.materials
        .iter()
        .map(|&(t, q)| held.get(&t).copied().unwrap_or(0) / q.max(1))
        .min()
        .unwrap_or(most)
        .min(most)
        .max(0)
}

/// One laboratory job a builder starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lab {
    /// Invent from a T1 copy: `runs` attempts at the T2 blueprint `product`.
    Invent {
        copy: BlueprintRow,
        product: i64,
        runs: i64,
    },
    /// Copy an original once, the copy carrying `licensed_runs`, to invent
    /// from.
    Copy {
        original: BlueprintRow,
        licensed_runs: i64,
    },
}

/// **The laboratory's next job** (pure): an invention from an idle T1 copy
/// here whose datacores the hangar covers, the best rung first; else a
/// copy of an idle original here with an invention, when the hangar holds
/// at least one attempt's datacores and no copy of it exists here yet.
/// `None`: nothing to invent with.
pub fn plan_lab(
    blueprints: &[BlueprintRow],
    station: i64,
    recipes: &industry::Table,
    held: &BTreeMap<i64, i64>,
) -> Option<Lab> {
    // T3-H: a relic is the Tech III laboratory's (`crate::t3::lab`).
    let invention = |t: i64| {
        recipes
            .science(t)
            .filter(|_| !recipes.is_relic(t))
            .and_then(|s| s.invention.as_ref())
    };
    let mut here: Vec<&BlueprintRow> = blueprints
        .iter()
        .filter(|b| b.location_id == station && invention(b.type_id).is_some())
        .collect();
    here.sort_by_key(|b| (std::cmp::Reverse(rank(b.type_id)), b.type_id, b.item_id));
    for copy in here
        .iter()
        .filter(|b| !b.is_original() && b.runs > 0 && b.job_id.is_none())
    {
        let Some(inv) = invention(copy.type_id) else {
            continue;
        };
        let Some(product) = invention_product(inv, recipes) else {
            continue;
        };
        let runs = datacore_sets(inv, held, copy.runs.min(INVENTION_BATCH));
        if runs > 0 {
            return Some(Lab::Invent {
                copy: **copy,
                product,
                runs,
            });
        }
    }
    for original in here
        .iter()
        .filter(|b| b.is_original() && b.job_id.is_none())
    {
        let Some(inv) = invention(original.type_id) else {
            continue;
        };
        let copied = here
            .iter()
            .any(|b| !b.is_original() && b.type_id == original.type_id);
        if copied || invention_product(inv, recipes).is_none() || datacore_sets(inv, held, 1) < 1 {
            continue;
        }
        let licence = recipes
            .find(original.type_id)
            .map_or(1, |bp| bp.max_production_limit.max(1));
        return Some(Lab::Copy {
            original: **original,
            licensed_runs: INVENTION_BATCH.min(licence),
        });
    }
    None
}

/// The idle copies here worth building from (an invention's product, or a
/// copy found in a data site whose product a doctrine wants, `wanted`; with
/// runs left): Tech II by type id, then Tech III by type id -- a T3 job holds
/// the slot for days and its inputs are the scarcest (T3-F).
pub fn t2_candidates<'a>(
    blueprints: &'a [BlueprintRow],
    station: i64,
    recipes: &industry::Table,
    wanted: &std::collections::BTreeSet<i64>,
) -> Vec<&'a BlueprintRow> {
    let invented = invented_types(recipes);
    let mut v: Vec<&BlueprintRow> = blueprints
        .iter()
        .filter(|b| {
            !b.is_original()
                && b.runs > 0
                && b.job_id.is_none()
                && b.location_id == station
                && (invented.contains(&b.type_id)
                    || recipes
                        .find(b.type_id)
                        .is_some_and(|bp| wanted.contains(&bp.product_type_id)))
                && recipes.find(b.type_id).is_some()
        })
        .collect();
    let t3 = t3_types(recipes);
    v.sort_by_key(|b| (t3.contains(&b.type_id), b.type_id, b.item_id));
    v
}

/// The bundle's recipes, if the content has any (`eve-bots --content`; a
/// test's fixture).
pub fn recipes_of(content: Option<&eve_server::content::Content>) -> Option<&industry::Table> {
    content
        .and_then(|c| c.try_get::<industry::Table>())
        .filter(|t| !t.blueprints.is_empty())
}

// ---- capital inputs: who makes them (CAP-C) --------------------------------------

/// **What the careers make** of the inputs a capital build takes: one flag
/// per class of [`crate::hulls::Input`] that is not a mineral (the miners'
/// minerals are `supply::Supply`'s, per system). Nothing from nowhere: a
/// blueprint with an input nobody makes holds no rung and bids for nothing,
/// as GB-FIX's rule says of a mineral no miner brings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Made {
    /// PI P1 and P2: the farmers (`pi`, `pi_p2`).
    pub pi_basic: bool,
    /// PI P3: an advanced factory fed by P2.
    pub pi_p3: bool,
    /// PI P4.
    pub pi_p4: bool,
    /// Reaction products (composites, polymers): a reactor career.
    pub reactions: bool,
    /// The eleven Neurolink commodities: site loot (`hacker`, CAP-LOOT).
    pub loot: bool,
    /// Capital components, intermediates and advanced components: builders
    /// make them from their own recipes (the capital rungs).
    pub built: bool,
}

/// **What the bots make today.** `pi_basic`: the PI farmer makes P1 and P2.
/// `pi_p3`, `pi_p4`: the same farmer founds P3 colonies, and P4 ones on the
/// planet types with the high-tech facility ([`crate::pi_p3`], X-PI3).
/// `loot`: explorers hack the cans that hold the commodities. `built`: the
/// capital rungs ([`crate::hulls::capital_rungs`]). `reactions`: the
/// reactor career (MN-G, `reactor.rs`) makes reaction products. Each
/// flag flips with the career that makes the good (and its falsifier is the
/// first soak whose bids for it stand for a game week).
pub const MADE_TODAY: Made = Made {
    pi_basic: true,
    pi_p3: true,
    pi_p4: true,
    reactions: true,
    loot: true,
    built: true,
};

/// How deep a component's own recipe is followed (a hull takes components,
/// a component takes composites, an intermediate takes components): one more
/// than the capital chain is long.
const SOURCING_DEPTH: usize = 4;

/// **Can the inputs of a recipe be got?** The capital half of "a blueprint
/// whose inputs cannot be obtained holds no rung" ([`makeable_with`]).
#[derive(Debug, Clone, Copy)]
pub struct Sourcing<'a> {
    content: &'a eve_server::content::Content,
    recipes: &'a industry::Table,
    made: Made,
}

impl<'a> Sourcing<'a> {
    /// With what the bots make today ([`MADE_TODAY`]).
    pub fn new(content: &'a eve_server::content::Content, recipes: &'a industry::Table) -> Self {
        Sourcing::with(content, recipes, MADE_TODAY)
    }

    /// With `made` (a test's world, or a society that grew a reactor).
    pub fn with(
        content: &'a eve_server::content::Content,
        recipes: &'a industry::Table,
        made: Made,
    ) -> Self {
        Sourcing {
            content,
            recipes,
            made,
        }
    }

    /// The inputs of `bp` that nothing makes, sorted: its own materials, and
    /// a component's, an intermediate's or an advanced component's inputs
    /// down its own recipe (a part nobody can make is a part its parent
    /// lacks). Minerals are not asked (`supply::Supply`'s).
    pub fn blocked(&self, bp: &Blueprint) -> Vec<i64> {
        let mut out = std::collections::BTreeSet::new();
        self.walk(bp, &mut vec![bp.blueprint_type_id], &mut out);
        out.into_iter().collect()
    }

    fn walk(&self, bp: &Blueprint, path: &mut Vec<i64>, out: &mut std::collections::BTreeSet<i64>) {
        use crate::hulls::Input;
        for &(t, _) in &bp.materials {
            let ok = match crate::hulls::input_of(self.content, t) {
                Input::Mineral | Input::Other => true,
                Input::PiBasic => self.made.pi_basic,
                Input::PiP3 => self.made.pi_p3,
                Input::PiP4 => self.made.pi_p4,
                Input::Reaction => self.made.reactions,
                Input::Loot => self.made.loot,
                Input::Abyssal => false,
                Input::Built => {
                    if !self.made.built {
                        false
                    } else if let Some(sub) = self.recipes.by_product(t) {
                        if path.len() >= SOURCING_DEPTH || path.contains(&sub.blueprint_type_id) {
                            false
                        } else {
                            path.push(sub.blueprint_type_id);
                            let mut below = std::collections::BTreeSet::new();
                            self.walk(sub, path, &mut below);
                            path.pop();
                            // The leaves are named, not the part they block.
                            out.extend(below);
                            true
                        }
                    } else {
                        false
                    }
                }
            };
            if !ok {
                out.insert(t);
            }
        }
    }
}

/// [`makeable`], and with `sourcing` every input of the recipe has a source
/// ([`Sourcing::blocked`]). A recipe nothing can feed holds no rung and is
/// no choice for one.
pub fn makeable_with(
    local: Option<Local<'_>>,
    sourcing: Option<&Sourcing<'_>>,
    recipes: &industry::Table,
    type_id: i64,
) -> bool {
    makeable(local, recipes, type_id)
        && sourcing.is_none_or(|s| {
            recipes
                .find(type_id)
                .is_some_and(|bp| s.blocked(bp).is_empty())
        })
}

/// **The ladder a builder climbs** (CAP-C): the T1 rungs and the starter
/// rung, then the capital ones ([`crate::hulls::capital_rungs`], found in
/// the recipes). The capital rungs come last because a rung with an ask here
/// the wallet cannot meet ends the climb ([`climb`]): a poor builder must
/// have reached the starter's 20 k ISK blueprint first.
pub fn ladder_with(capital: &[Vec<i64>]) -> Vec<&[i64]> {
    BLUEPRINT_RUNGS
        .iter()
        .copied()
        .chain(capital.iter().map(Vec::as_slice))
        .collect()
}

/// A job this long (nominal game seconds) holds the builder's one
/// manufacturing slot for days: a capital hull is 20 game days, a component
/// about 4.5 hours, a T1 job at most [`JOB_TARGET`] **[A]**. Falsified by
/// builders whose slot sits on a hull while their wares starve.
pub const LONG_JOB_SECS: i64 = 2 * 24 * 60 * 60;

/// **Run times under `--time-scale`.** A job's length is the blueprint's
/// game seconds: the server ends it `clock.span(nominal)` after its start,
/// at any scale (`Install::paced`), and the builder compares that end with
/// the wall clock ([`deliver`]), so it delivers on time at every scale and
/// plans its runs in game seconds ([`planned_runs`]). The nominal length of
/// `runs` runs of `bp`, before the skills' time bonuses.
pub fn nominal_secs(bp: &Blueprint, runs: i64) -> i64 {
    bp.time.max(1).saturating_mul(runs.max(1))
}

/// The scale a job runs at, read off its end: nominal game seconds over the
/// wall seconds left from `now` to `end_date` (FILETIMEs). `None` for a job
/// already over. At scale 1 about 1 (the skills' bonuses shorten the job),
/// at the soak's 8 about 8.
pub fn pace_of(nominal: i64, end_date: i64, now: i64) -> Option<f64> {
    let wall = (end_date - now) as f64 / industry::FILETIME_SECOND as f64;
    (wall > 0.0).then(|| nominal as f64 / wall)
}

// ---- the cycles over the wire ------------------------------------------------------

/// What one [`run`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cycle {
    /// The asks it posted.
    pub posted: Vec<SellLine>,
    /// `(typeID, units, ISK)` per buy that filled: `wants` minerals, a
    /// blueprint, job materials.
    pub bought: Vec<(i64, i64, f64)>,
    /// `(jobID, productTypeID, runs)` per job delivered.
    pub delivered: Vec<(i64, i64, i64)>,
    /// `(jobID, blueprintTypeID, runs)` per job installed.
    pub installed: Vec<(i64, i64, i64)>,
    /// `(jobID, blueprintTypeID, activityID, runs)` per laboratory job
    /// installed: a copy for invention, or an invention (SOC-B7).
    pub lab: Vec<(i64, i64, i64, i64)>,
    /// The blueprint type bought, if one was.
    pub blueprint: Option<i64>,
    /// `(typeID, price, units)` per standing bid placed.
    pub bids: Vec<(i64, f64, i64)>,
    /// `(orderID, from, to)` per order repriced.
    pub repriced: Vec<(i64, f64, f64)>,
    /// Orders off their price but inside the modification delay.
    pub waiting: Vec<i64>,
    /// `(itemID, typeID)` per Sovereignty Hub in its hangar at the end of the
    /// cycle (D-HUB): what the driver posts to a
    /// [`crate::hub_supply::HubBoard`] for the sov bots.
    pub hubs: Vec<(i64, i64)>,
    /// FE-BUILD: `(blueprintTypeID, ISK per game hour at V)` per job the
    /// builder chose this cycle, installed or bid for: what the soak's
    /// job-hours by basket read.
    pub picks: Vec<(i64, f64)>,
    /// Why something was not done: a refusal, a missing recipe.
    pub notes: Vec<String>,
}

impl Cycle {
    /// The act line's note.
    pub fn summary(&self) -> String {
        let spent: f64 = self.bought.iter().map(|&(_, _, isk)| isk).sum();
        let mut s = format!(
            "posted {} asks, bought {} lots for {spent:.2} ISK; delivered {} jobs, installed {}",
            self.posted.len(),
            self.bought.len(),
            self.delivered.len(),
            self.installed.len(),
        );
        if let Some(bp) = self.blueprint {
            s.push_str(&format!("; bought blueprint {bp}"));
        }
        for &(_, bp, activity, runs) in &self.lab {
            let what = if activity == industry::INVENTION {
                "invention"
            } else {
                "copying"
            };
            s.push_str(&format!("; {what} {bp} x{runs}"));
        }
        if !self.bids.is_empty() || !self.repriced.is_empty() || !self.waiting.is_empty() {
            s.push_str(&format!(
                "; {} bids placed, {} repriced, {} waiting out the delay",
                self.bids.len(),
                self.repriced.len(),
                self.waiting.len()
            ));
        }
        for &(bp, pph) in &self.picks {
            s.push_str(&format!("; chose {bp} at {pph:.0} ISK/h"));
        }
        for n in &self.notes {
            s.push_str("; ");
            s.push_str(n);
        }
        s
    }
}

/// What one [`sell_goods`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct Sold {
    /// `(typeID, quantity)` the refinery made of the ore, if it refined.
    pub refined: Vec<(i64, i64)>,
    /// Why the ore was not refined, if it was not (the sale goes on).
    pub not_refined: Option<String>,
    pub lines: Vec<SellLine>,
    /// The wallet before and after.
    pub before: f64,
    pub after: f64,
}

pub(crate) fn station_of(conn: &Conn, what: &str) -> Result<i64> {
    conn.session()
        .station_id()
        .ok_or_else(|| Error::NotSent(format!("{what}: not docked")))
}

/// A refusal is a note and the cycle goes on; a broken connection ends it.
pub(crate) fn noted<T>(r: Result<T>, cycle: &mut Cycle, what: &str) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => {
            cycle.notes.push(format!("{what}: {e}"));
            Ok(None)
        }
    }
}

/// **What this station's miners can bring** (GB-FIX): the mineral supply
/// of the system the builder is docked in ([`crate::supply::Supply`]). With
/// none, nothing is ruled out.
#[derive(Debug, Clone, Copy)]
pub struct Local<'a> {
    pub supply: &'a crate::supply::Supply,
    pub system: i64,
    /// PROG-NULL: the minerals miners out in the deep belts cut count as
    /// obtainable while such a miner is reporting.
    pub rare: Option<&'a crate::rare_ore::RareBoard>,
}

impl Local<'_> {
    /// The materials of `bp` this system's miners cannot bring.
    pub fn missing(&self, bp: &Blueprint) -> Vec<i64> {
        let mut missing = self.supply.missing(self.system, &bp.materials);
        if let Some(board) = self.rare {
            let offered = board.offered();
            missing.retain(|m| !offered.contains(m));
        }
        missing
    }
}

/// Whether the builder may work blueprint `type_id`: it has a recipe, and
/// with a `local` supply every mineral in it is one the miners here bring.
fn makeable(local: Option<Local<'_>>, recipes: &industry::Table, type_id: i64) -> bool {
    recipes
        .find(type_id)
        .is_some_and(|bp| local.is_none_or(|l| l.missing(bp).is_empty()))
}

/// **One builder cycle, docked** (the module header's six steps).
/// `content` holds the recipes ([`recipes_of`]); without them the builder
/// only lists and buys `wants`. No trips: a rung its station does not sell
/// is not climbed.
pub fn run(
    conn: &mut Conn,
    wants: &[(i64, i64)],
    content: Option<&eve_server::content::Content>,
) -> Result<Cycle> {
    run_with(conn, wants, content, None)
}

/// [`run`], and with `trips` (the travel lane) a ladder rung no ask at this
/// station sells is fetched from the nearest station within reach that
/// does ([`crate::travel::find`], [`crate::travel::fetch`]) and brought home
/// in the hold.
pub fn run_with(
    conn: &mut Conn,
    wants: &[(i64, i64)],
    content: Option<&eve_server::content::Content>,
    trips: Option<crate::travel::Trips<'_>>,
) -> Result<Cycle> {
    let station = station_of(conn, "builder")?;
    let mut prices = Prices::read(conn)?;
    let mut cycle = Cycle::default();
    let recipes = recipes_of(content);
    let local = content
        .and_then(|c| c.try_get::<crate::supply::Supply>())
        .zip(conn.session().solar_system_id2())
        .map(|(supply, system)| Local {
            supply,
            system,
            rare: content.and_then(|c| c.try_get::<crate::rare_ore::RareBoard>()),
        });
    let sourcing = content.zip(recipes).map(|(c, r)| Sourcing::new(c, r));
    let parts = content.and_then(crate::doctrine_parts::ctx_of);
    if recipes.is_some() {
        deliver(conn, &mut cycle)?;
    }
    list(conn, station, &mut prices, recipes, &is_product, &mut cycle)?;
    if let Some(recipes) = recipes {
        build(
            conn,
            station,
            recipes,
            &mut prices,
            trips,
            local,
            sourcing.as_ref(),
            parts.as_ref(),
            &mut cycle,
        )?;
        // X-RIG: a rig copy the builder owns is built from like a T2 copy.
        if let Some(content) = content {
            crate::rig::build_copies(
                conn,
                station,
                recipes,
                content,
                &mut prices,
                trips,
                &mut cycle,
            )?;
        }
        // T3-H: a relic's invention towards the doctrine kit, ahead of the T2
        // laboratory (which takes the slot when this finds nothing to do).
        if let Some(content) = content {
            crate::t3::lab(conn, station, recipes, content, &mut prices, &mut cycle)?;
        }
        lab(conn, station, recipes, &mut prices, &mut cycle)?;
        if let Some(rows) = noted(conn.hangar_items(), &mut cycle, "hangar")? {
            cycle.hubs = crate::hub_supply::hubs_in(&rows)
                .iter()
                .map(|r| (r.item_id, r.type_id))
                .collect();
        }
    }
    for &(type_id, want) in wants {
        let Some(p_ref) = prices.p_ref(type_id) else {
            continue;
        };
        let p_mkt = prices.p_mkt(conn, type_id)?.unwrap_or(p_ref);
        let Some(cap) = pricing::buy_cap(p_ref, p_mkt) else {
            continue;
        };
        let book = conn.market_orders(type_id)?;
        let isk = conn.cash_balance()?.max(0.0) * WALLET_SHARE;
        for (ask, units) in buy_plan(&book, cap, isk, want) {
            let row = conn.buy(ask.station_id, type_id, ask.price, units, 0, BROKER_FEE)?;
            let got = units - row.vol_remaining.clamp(0, units);
            let paid = got as f64 * ask.price;
            if got > 0 {
                cycle.bought.push((type_id, got, paid));
            }
        }
    }
    Ok(cycle)
}

/// Step 1: every job whose end time has passed, delivered.
pub(crate) fn deliver(conn: &mut Conn, cycle: &mut Cycle) -> Result<()> {
    let Some(jobs) = noted(conn.industry_jobs(), cycle, "jobs")? else {
        return Ok(());
    };
    let now = eve_server::pyutil::filetime_now();
    for job in jobs.iter().filter(|j| j.ready_at(now)) {
        if noted(conn.complete_job(job), cycle, "deliver")?.is_some() {
            cycle
                .delivered
                .push((job.job_id, job.product_type_id, job.runs));
        }
    }
    Ok(())
}

/// Step 2: one ask per product type at this station, at the seller's ask
/// ([`pricing::ask`]: `P_mkt`, the materials' cost by the recipe, the NPC
/// ceiling, and the days of excess stock); an ask off its price repriced
/// ([`reprice`]), with or without a stack behind it. A stack of a type
/// already listed waits for its order to sell out.
pub(crate) fn list(
    conn: &mut Conn,
    station: i64,
    prices: &mut Prices,
    recipes: Option<&industry::Table>,
    keep: &dyn Fn(&ItemRow) -> bool,
    cycle: &mut Cycle,
) -> Result<()> {
    let open = conn.my_open_orders()?;
    let now = prices.now();
    let hangar: Vec<ItemRow> = conn
        .hangar_items()?
        .into_iter()
        .filter(|r| keep(r))
        .collect();
    let mut types: Vec<i64> = hangar.iter().map(|r| r.type_id).collect();
    types.extend(
        open.iter()
            .filter(|o| !o.order.bid && o.order.station_id == station)
            .map(|o| o.order.type_id),
    );
    let mut seen: Vec<i64> = Vec::new();
    for type_id in types {
        if seen.contains(&type_id) {
            continue;
        }
        seen.push(type_id);
        let Some(p_ref) = prices.p_ref(type_id) else {
            continue;
        };
        let mine = open
            .iter()
            .find(|o| !o.order.bid && o.order.type_id == type_id && o.order.station_id == station);
        let held: i64 = hangar
            .iter()
            .filter(|r| r.type_id == type_id)
            .map(|r| r.quantity)
            .sum();
        let p_mkt = prices.p_mkt(conn, type_id)?.unwrap_or(p_ref);
        // A reaction's product costs its formula's inputs (MN-G).
        let made_by = recipes.and_then(|r| {
            r.by_product(type_id)
                .or_else(|| r.reactions.iter().find(|b| b.product_type_id == type_id))
        });
        let cost = match made_by {
            Some(bp) => {
                let materials: Vec<i64> = bp.materials.iter().map(|&(t, _)| t).collect();
                pricing::unit_cost(bp, &prices.mkt_of(conn, &materials)?)
            }
            None => 0.0,
        };
        let steps = match mine {
            Some(own) => pricing::excess_steps(
                own.order.vol_remaining + held,
                prices.daily_volume(conn, type_id)?,
                (now - own.issue_date).div_euclid(pricing::DAY_TICKS),
            ),
            None => 0,
        };
        let book = conn.market_orders(type_id)?;
        let quote = Quote {
            p_ref,
            p_mkt,
            cost,
            ceiling: book.npc_ceiling(),
            steps,
            pressure: prices.pressure(conn, type_id)?,
            resting: mine.map(|o| o.order.price),
        };
        let Some(price) = pricing::ask(&quote) else {
            continue;
        };
        if let Some(own) = mine {
            settle(conn, own, price, now, cycle)?;
            continue;
        }
        let Some(row) = hangar.iter().find(|r| r.type_id == type_id) else {
            continue;
        };
        let line = SellLine {
            station_id: station,
            type_id: row.type_id,
            item_id: row.item_id,
            price,
            quantity: row.quantity,
        };
        if noted(conn.sell(&[line], ORDER_DAYS, BROKER_FEE), cycle, "list")?.is_some() {
            cycle.posted.push(line);
        }
    }
    Ok(())
}

/// Bring one of my orders to `target` by [`reprice`].
pub(crate) fn settle(
    conn: &mut Conn,
    own: &OwnOrder,
    target: f64,
    now: i64,
    cycle: &mut Cycle,
) -> Result<()> {
    // FE-VALUE: a rise is held to `FE_MAX_STEP` over the resting price.
    let target = pricing::capped_rise(Some(own.order.price), target);
    match reprice(own, target, now) {
        Reprice::Keep => {}
        Reprice::Wait => cycle.waiting.push(own.order.order_id),
        Reprice::Modify(to) => {
            if noted(conn.modify_order(own, to), cycle, "reprice")?.is_some() {
                cycle
                    .repriced
                    .push((own.order.order_id, own.order.price, to));
            }
        }
    }
    Ok(())
}

/// The hangar's units by type.
pub(crate) fn held_by_type(rows: &[ItemRow]) -> BTreeMap<i64, i64> {
    let mut held = BTreeMap::new();
    for r in rows.iter().filter(|r| !r.singleton) {
        *held.entry(r.type_id).or_insert(0) += r.quantity;
    }
    held
}

/// One blueprint the builder may work this cycle, appraised.
struct Pick<'a> {
    row: &'a BlueprintRow,
    bp: &'a Blueprint,
    bids: BTreeMap<i64, f64>,
}

/// Rest one standing bid per (type, station) for each of `short` that has
/// none, at `bids`; the placed ones are noted in `cycle`. `true` when any
/// was placed.
fn rest_bids(
    conn: &mut Conn,
    station: i64,
    short: &[(i64, i64)],
    bids: &BTreeMap<i64, f64>,
    open: &[OwnOrder],
    cycle: &mut Cycle,
) -> Result<bool> {
    let mut placed = false;
    for &(t, n) in short {
        let resting = open
            .iter()
            .any(|o| o.order.bid && o.order.type_id == t && o.order.station_id == station);
        let Some(&price) = bids.get(&t) else {
            continue;
        };
        if resting || n <= 0 {
            continue;
        }
        if noted(
            conn.buy(station, t, price, n, ORDER_DAYS, BROKER_FEE),
            cycle,
            "bid",
        )?
        .is_some()
        {
            cycle.bids.push((t, price, n));
            placed = true;
        }
    }
    Ok(placed)
}

/// Steps 3 and 4: climb the ladder, then **build by margin** (FE-BUILD):
/// every idle original the builder can feed is appraised at `V`
/// ([`crate::build_choice::appraise`]); the jobs that pay are ranked by ISK
/// per hour of the slot, the rest follow in ladder order (their bids sit
/// under `V`, so what they install is bought at a margin); and the builder
/// starts as many as it has free slots, bidding for the inputs of the next
/// while the slots are full ([`crate::build_choice::FE_INPUT_RUNS`]).
#[allow(clippy::too_many_arguments)]
fn build(
    conn: &mut Conn,
    station: i64,
    recipes: &industry::Table,
    prices: &mut Prices,
    trips: Option<crate::travel::Trips<'_>>,
    local: Option<Local<'_>>,
    sourcing: Option<&Sourcing<'_>>,
    parts: Option<&crate::doctrine_parts::Ctx<'_>>,
    cycle: &mut Cycle,
) -> Result<()> {
    use crate::build_choice as bc;
    // The products the leaders' doctrines want: a found copy of one is built.
    let wanted: std::collections::BTreeSet<i64> = parts
        .map(|ctx| ctx.board.wanted().into_keys().collect())
        .unwrap_or_default();
    // BP-COPIES: the hackers contract their data-site copies; take those of
    // a wanted product, so build_t2 below finds them in the hangar.
    if !wanted.is_empty() {
        let makes = |t: i64| {
            recipes
                .find(t)
                .is_some_and(|bp| wanted.contains(&bp.product_type_id))
        };
        match crate::contracts::collect_copies(conn, &makes) {
            Ok(0) => {}
            Ok(n) => cycle
                .notes
                .push(format!("took {n} blueprint copies by contract")),
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(e) => cycle.notes.push(format!("copies by contract: {e}")),
        }
    }
    let Some(mut blueprints) = noted(conn.blueprints(), cycle, "blueprints")? else {
        return Ok(());
    };
    let levels = noted(crate::acquire::skill_levels(conn), cycle, "skills")?;
    let slots = levels
        .as_ref()
        .map_or(MANUFACTURE_SLOTS as i64, |l| {
            industry::max_slots(|s| l.get(&s).copied().unwrap_or(0))
        })
        .max(1) as usize;
    climb(
        conn,
        station,
        recipes,
        prices,
        &blueprints,
        trips,
        local,
        sourcing,
        parts,
        levels.as_ref(),
        cycle,
    )?;
    if cycle.blueprint.is_some() {
        blueprints = conn.blueprints()?;
    }
    let mut running = conn
        .industry_jobs()?
        .iter()
        .filter(|j| !j.completed() && j.activity_id == industry::MANUFACTURING)
        .count();
    if running < slots
        && build_t2(
            conn,
            station,
            recipes,
            prices,
            &blueprints,
            &wanted,
            trips,
            cycle,
        )?
    {
        running += 1;
        if running >= slots {
            return Ok(());
        }
    }
    let mut open = conn.my_open_orders()?;
    let now = eve_server::pyutil::filetime_now();
    // The candidates this cycle, gated, appraised and ordered.
    let mut good: Vec<(Pick<'_>, bc::Appraisal, i64)> = Vec::new();
    let mut rest: Vec<(Pick<'_>, bc::Appraisal, i64)> = Vec::new();
    for bp_row in candidates(&blueprints, station, recipes) {
        let Some(bp) = recipes.find(bp_row.type_id) else {
            continue;
        };
        // A blueprint whose mineral no miner here brings would rest a bid
        // nobody fills and block every other (GB-FIX: the gate soak's seven
        // stalled builders).
        if let Some(l) = local {
            let missing = l.missing(bp);
            if !missing.is_empty() {
                cycle.notes.push(format!(
                    "blueprint {}: no miner here brings {missing:?}",
                    bp_row.type_id
                ));
                continue;
            }
        }
        // The same rule for an input no career makes (CAP-C).
        if let Some(s) = sourcing {
            let blocked = s.blocked(bp);
            if !blocked.is_empty() {
                cycle.notes.push(format!(
                    "blueprint {}: nobody makes {blocked:?}",
                    bp_row.type_id
                ));
                continue;
            }
        }
        let mut types: Vec<i64> = bp.materials.iter().map(|&(t, _)| t).collect();
        types.push(bp.product_type_id);
        let mkt = prices.mkt_of(conn, &types)?;
        let Some(bids) = pricing::margin_bids(bp, &mkt, prices.refs()) else {
            cycle
                .notes
                .push(format!("blueprint {}: no margin at P_mkt", bp_row.type_id));
            continue;
        };
        // The demand map (FE-DEMAND's `demand::map`, DOC-6's swap): other
        // people's bids against asks for the product, the bot's own resting
        // orders left out, and the units the leaders' doctrines want of it.
        let dm = crate::demand::map(conn, prices, &[bp.product_type_id])?;
        let (bid_u, ask_u) = dm
            .get(bp.product_type_id)
            .map_or((0, 0), |d| (d.bid_units, d.ask_units));
        let wanted = parts.map_or(0, |c| c.board.wanted_of(bp.product_type_id));
        let pressure = bc::pressure_of(bid_u + wanted, ask_u);
        let me = bp_row.material_efficiency;
        let appraisal = bc::appraise(
            bp,
            me,
            planned_runs(bp),
            &mkt,
            pressure,
            pricing::Rates::UNSKILLED,
        );
        let rank = rank(bp_row.type_id);
        let pick = Pick {
            row: bp_row,
            bp,
            bids,
        };
        match appraisal {
            Some(a) if a.profit_per_hour > 0.0 => good.push((pick, a, rank)),
            Some(a) => rest.push((pick, a, rank)),
            None => continue,
        }
    }
    // Those that pay, by ISK an hour; the others by the ladder's order.
    let mut order: Vec<(Pick<'_>, bc::Appraisal)> = bc::order_jobs(good);
    rest.sort_by_key(|(p, _, rank)| (std::cmp::Reverse(*rank), p.row.type_id, p.row.item_id));
    order.extend(rest.into_iter().map(|(p, a, _)| (p, a)));
    let mut bidders = 0;
    let mut started: Vec<i64> = Vec::new();
    for (pick, app) in order {
        let (bp_row, bp, bids) = (pick.row, pick.bp, &pick.bids);
        if started.contains(&app.product) {
            continue;
        }
        // Standing bids for this recipe follow its price, job or no job.
        for own in open
            .iter()
            .filter(|o| o.order.bid && o.order.station_id == station)
        {
            if let Some(&target) = bids.get(&own.order.type_id) {
                settle(conn, own, target, now, cycle)?;
            }
        }
        let me = bp_row.material_efficiency;
        let escrow: f64 = open
            .iter()
            .filter(|o| o.order.bid)
            .map(|o| o.order.price * o.order.vol_remaining as f64)
            .sum();
        let wallet = conn.cash_balance()?;
        let isk = ((wallet + escrow) * WALLET_SHARE - escrow).max(0.0);
        let mut held = held_by_type(&conn.hangar_items()?);
        if running >= slots {
            // Every slot is busy: bid for the inputs of the next job, so it
            // starts the cycle a slot frees, and stop.
            let planned = bc::buffered_runs(planned_runs(bp));
            let runs = affordable_runs(bp, planned, me, &held, bids, isk);
            if runs > 0 && bidders < bc::BID_PICKS {
                let short = shortfall(bp, runs, me, &held);
                if rest_bids(conn, station, &short, bids, &open, cycle)? {
                    cycle.picks.push((bp_row.type_id, app.profit_per_hour));
                }
            }
            return Ok(());
        }
        let planned = affordable_runs(bp, planned_runs(bp), me, &held, bids, isk)
            .max(runs_on_hand(bp, planned_runs(bp), me, &held));
        if planned == 0 {
            cycle.notes.push(format!(
                "blueprint {}: one run's materials exceed {isk:.2} ISK",
                bp_row.type_id
            ));
            continue;
        }
        // Buy what is short from asks here, at no more than the margin
        // rule's price.
        for (t, short) in shortfall(bp, planned, me, &held) {
            let book = conn.market_orders(t)?;
            let isk = conn.cash_balance()?;
            for (ask, n) in crate::acquire::plan(&book, station, Some(bids[&t]), isk, short) {
                let Some(row) = noted(
                    conn.buy(station, t, ask.price, n, 0, BROKER_FEE),
                    cycle,
                    "materials",
                )?
                else {
                    break;
                };
                let got = n - row.vol_remaining.clamp(0, n);
                if got > 0 {
                    cycle.bought.push((t, got, got as f64 * ask.price));
                }
            }
        }
        held = held_by_type(&conn.hangar_items()?);
        let runs = runs_on_hand(bp, planned, me, &held);
        if runs > 0 {
            let before = cycle.installed.len();
            install(conn, bp_row, runs, trips, cycle)?;
            if cycle.installed.len() == before {
                // Refused (a skill is being learned): the next cycle.
                return Ok(());
            }
            cycle.picks.push((bp_row.type_id, app.profit_per_hour));
            note_pace(conn, bp, runs, cycle)?;
            started.push(app.product);
            running += 1;
            if running >= slots {
                return Ok(());
            }
            continue;
        }
        // Short still: one standing bid per (type, station) for the rest.
        if bidders < bc::BID_PICKS
            && rest_bids(
                conn,
                station,
                &shortfall(bp, planned, me, &held),
                bids,
                &open,
                cycle,
            )?
        {
            bidders += 1;
            started.push(app.product);
            cycle.picks.push((bp_row.type_id, app.profit_per_hour));
            open = conn.my_open_orders()?;
        }
        if slots == 1 {
            return Ok(());
        }
    }
    Ok(())
}

/// Lacking rungs whose product's book the builder reads each cycle for the
/// demand pull **[A]**: a market read per rung, so bounded. Falsified by a
/// rung deep on the ladder whose demand never pulls it (too low) or builders
/// crowding the market calls (too high).
const PROBE_RUNGS: usize = 6;

/// A long job installed ([`LONG_JOB_SECS`]) is noted with its nominal game
/// days and the scale its end implies ([`pace_of`]): the soak's reader sees
/// at once that a hull is 20 game days and what that is on the wall clock.
fn note_pace(conn: &mut Conn, bp: &Blueprint, runs: i64, cycle: &mut Cycle) -> Result<()> {
    let nominal = nominal_secs(bp, runs);
    let Some(&(job_id, _, _)) = cycle.installed.last() else {
        return Ok(());
    };
    if nominal < LONG_JOB_SECS || cycle.installed.len() != 1 {
        return Ok(());
    }
    let now = eve_server::pyutil::filetime_now();
    let Some(job) = noted(conn.industry_jobs(), cycle, "jobs")?
        .and_then(|jobs| jobs.into_iter().find(|j| j.job_id == job_id))
    else {
        return Ok(());
    };
    let scale =
        pace_of(nominal, job.end_date, now).map_or("over".to_owned(), |s| format!("{s:.1}"));
    cycle.notes.push(format!(
        "long job {job_id}: {:.1} game days, scale about {scale}",
        nominal as f64 / 86_400.0
    ));
    Ok(())
}

/// Step 3: **climb the ladder** ([`BLUEPRINT_RUNGS`]). For each rung it
/// lacks, in order: the cheapest of the rung's blueprints asked here under
/// the consumer's cap ([`pricing::reservation`]), bought when [`may_climb`]
/// says so; with no ask here and `trips`, the nearest seller of any of them
/// within reach ([`fetch_rung`]). A rung that nothing in reach sells is
/// passed over for the next **[A]** (falsified by builders that hold a
/// cruiser blueprint and no frigate one a human two systems farther buys).
#[allow(clippy::too_many_arguments)]
fn climb(
    conn: &mut Conn,
    station: i64,
    recipes: &industry::Table,
    prices: &mut Prices,
    blueprints: &[BlueprintRow],
    trips: Option<crate::travel::Trips<'_>>,
    local: Option<Local<'_>>,
    sourcing: Option<&Sourcing<'_>>,
    parts: Option<&crate::doctrine_parts::Ctx<'_>>,
    levels: Option<&BTreeMap<i64, i64>>,
    cycle: &mut Cycle,
) -> Result<()> {
    let held: Vec<i64> = blueprints
        .iter()
        .filter(|b| b.is_original())
        .map(|b| b.type_id)
        .collect();
    let holds_any = !held.is_empty();
    let wallet = conn.cash_balance()?;
    let ok = |t: i64| makeable_with(local, sourcing, recipes, t);
    // PROG-MINE: the mining rungs (strip miners, barges, command ships)
    // before the capital ones.
    let capital = sourcing.map_or_else(Vec::new, |s| {
        let mut rungs = crate::hulls::mining_rungs(s.content, recipes);
        rungs.extend(crate::hulls::capital_rungs(s.content, recipes));
        rungs.extend(crate::hulls::t3_rungs(s.content, recipes));
        rungs
    });
    // PROG-PVE: the battlecruiser and battleship rungs go between the
    // starter and the capital ones (`hulls::warship_rungs`).
    let warships = crate::hulls::warship_rungs(recipes);
    // PROG-PVE2: then the guns, rounds and tank modules of the bots' weapon
    // systems (`hulls::arms_rungs`).
    let arms = crate::hulls::arms_rungs(recipes);
    // DOC-7: then the parts the leaders' doctrines want that no rung holds
    // yet (extenders, boosters, other sizes of gun), most wanted first.
    let mut doctrine = Vec::new();
    if let Some(ctx) = parts {
        let wanted = ctx.board.wanted();
        let src = ctx.sources(recipes, sourcing);
        let others: std::collections::BTreeSet<i64> = BLUEPRINT_RUNGS
            .iter()
            .flat_map(|r| r.iter().copied())
            .chain(warships.iter().flatten().copied())
            .chain(arms.iter().flatten().copied())
            .chain(capital.iter().flatten().copied())
            .collect();
        doctrine = crate::doctrine_parts::doctrine_rungs(recipes, &wanted, &src, &others);
        // The parts nothing can supply are logged, once each.
        let gaps = ctx
            .board
            .first_report(&crate::doctrine_parts::unsourced(&wanted, &src));
        if !gaps.is_empty() {
            cycle
                .notes
                .push(crate::doctrine_parts::unsourced_line(&gaps));
        }
    }
    let mut tail = warships;
    tail.extend(doctrine);
    tail.extend(arms);
    tail.extend(capital);
    let rungs = ladder_with(&tail);
    let mut lacking = lacking_rungs_by(&rungs, &held, recipes, &ok);
    // FE-BUILD: a rung whose product is dear -- a doctrine wants it, or bids
    // have outweighed asks for [`FE_DEMAND_CYCLES`] cycles -- goes ahead of
    // the ladder's order, when the builder has the skills for it.
    if let Some(ctx) = parts {
        use crate::build_choice as bc;
        let skilled = |t: i64| {
            recipes.find(t).is_some_and(|bp| {
                levels.is_none_or(|l| {
                    bp.skills
                        .iter()
                        .all(|&(s, lv)| l.get(&s).copied().unwrap_or(0) >= lv)
                })
            })
        };
        for choices in lacking.iter().take(PROBE_RUNGS) {
            let Some(product) = choices
                .first()
                .and_then(|&t| recipes.find(t))
                .map(|bp| bp.product_type_id)
            else {
                continue;
            };
            let dm = crate::demand::map(conn, prices, &[product])?;
            let hot = dm
                .get(product)
                .is_some_and(|d| bc::pressure_of(d.bid_units, d.ask_units) >= bc::FE_DEMAND_BUY);
            ctx.board.observe(product, hot);
        }
        lacking = bc::pull_demanded(lacking, &|choices: &[i64]| {
            choices.iter().any(|&t| {
                skilled(t)
                    && recipes.find(t).is_some_and(|bp| {
                        bc::pulls(
                            ctx.board.wanted_of(bp.product_type_id),
                            ctx.board.streak_of(bp.product_type_id),
                        )
                    })
            })
        });
    }
    for (rung, choices) in lacking.into_iter().enumerate() {
        let mut here: Option<(i64, BookOrder)> = None;
        let mut far: Option<(i64, crate::travel::Source, Option<f64>)> = None;
        // PROG-NULL: the best supply run when no near seller has it.
        let mut deep: Option<(i64, crate::supply_run::Candidate, Option<f64>)> = None;
        for &t in &choices {
            let book = conn.market_orders(t)?;
            let cap = prices.reservation(conn, t)?;
            if let Some((ask, _)) = crate::acquire::plan(&book, station, cap, wallet, 1)
                .into_iter()
                .next()
                && here.is_none_or(|(_, best)| ask.price < best.price)
            {
                here = Some((t, ask));
            }
            if let (Some(trips), Some(system)) = (trips, conn.session().solar_system_id2()) {
                let nearest = crate::travel::sources(&book, trips, system, station, cap, 1)
                    .into_iter()
                    .find(|s| s.price <= wallet);
                if let Some(src) = nearest
                    && far.is_none_or(|(_, f, _)| (src.jumps, src.price) < (f.jumps, f.price))
                {
                    far = Some((t, src, cap));
                }
                if let Some(c) = crate::supply_run::plan(
                    &book,
                    &crate::supply_run::Want {
                        map: trips.map,
                        from: system,
                        home: station,
                        cap,
                        wallet,
                        units: 1,
                        gain: 0.0,
                        worth_x: crate::supply_run::RUNG_WORTH_X,
                        exposure: crate::supply_run::BUILDER_EXPOSURE,
                        hazards: &crate::supply_run::Hazards::none(),
                        escorts: 0,
                    },
                ) && deep.is_none_or(|(_, d, _)| c.net > d.net)
                {
                    deep = Some((t, c, cap));
                }
            }
        }
        if let Some((t, ask)) = here {
            if may_climb(holds_any, wallet, ask.price)
                && let Some(row) = noted(
                    conn.buy(station, t, ask.price, 1, 0, BROKER_FEE),
                    cycle,
                    "blueprint",
                )?
                && row.vol_remaining < 1
            {
                cycle.bought.push((t, 1, ask.price));
                cycle.blueprint = Some(t);
            }
            return Ok(());
        }
        if let (Some(trips), Some((t, source, cap))) = (trips, far) {
            return fetch_rung(conn, trips, t, &source, cap, holds_any, cycle);
        }
        // PROG-NULL: nothing near sells it -- a supply run to the far seller
        // (docs/lanes/prog-null.md), over the safest floor that reaches it.
        if let (Some(trips), Some((t, c, cap))) = (trips, deep) {
            cycle.notes.push(format!(
                "supply run: rung {rung} from station {} ({} jumps, risk {:.3})",
                c.source.station, c.source.jumps, c.risk
            ));
            let trips = crate::supply_run::trips_for(trips.map, &c);
            return fetch_rung(conn, trips, t, &c.source, cap, holds_any, cycle);
        }
        cycle.notes.push(format!(
            "ladder: no seller in reach for lacking rung {rung}"
        ));
    }
    Ok(())
}

/// **A rung fetched** (the travel lane): from `source`, the nearest seller
/// within reach, when [`may_climb`] allows its price, there and back with
/// the blueprint in the hold. A trip is a note; a refused one too.
fn fetch_rung(
    conn: &mut Conn,
    trips: crate::travel::Trips<'_>,
    rung: i64,
    source: &crate::travel::Source,
    cap: Option<f64>,
    holds_any: bool,
    cycle: &mut Cycle,
) -> Result<()> {
    if !may_climb(holds_any, conn.cash_balance()?, source.price) {
        return Ok(());
    }
    let haul = crate::travel::Haul::Cargo;
    let Some(fetched) = noted(
        crate::travel::fetch(conn, trips, source, rung, 1, cap, haul),
        cycle,
        "blueprint trip",
    )?
    else {
        return Ok(());
    };
    cycle.bought.extend(fetched.bought.iter().copied());
    if fetched.home > 0 {
        cycle.blueprint = Some(rung);
    }
    cycle.notes.push(fetched.summary());
    Ok(())
}

/// **Tech II from an invented copy** (SOC-B7), ahead of the ladder: the
/// first idle T2 copy here ([`t2_candidates`]) whose materials the hangar
/// covers for a run, after buying what is short from asks here at the
/// margin rule's price -- but only when the asks here cover all of it, so
/// no half-set is bought. No standing bid is rested for a T2 material: its
/// components come from moon materials and reactions (stage D), which no
/// one makes yet, so a missing one is a note. `true`: a job installed.
#[allow(clippy::too_many_arguments)]
fn build_t2(
    conn: &mut Conn,
    station: i64,
    recipes: &industry::Table,
    prices: &mut Prices,
    blueprints: &[BlueprintRow],
    wanted: &std::collections::BTreeSet<i64>,
    trips: Option<crate::travel::Trips<'_>>,
    cycle: &mut Cycle,
) -> Result<bool> {
    for bp_row in t2_candidates(blueprints, station, recipes, wanted) {
        let Some(bp) = recipes.find(bp_row.type_id) else {
            continue;
        };
        let planned = bp_row.runs.min(planned_runs(bp));
        let me = bp_row.material_efficiency;
        let mut held = held_by_type(&conn.hangar_items()?);
        if runs_on_hand(bp, planned, me, &held) == 0 {
            let mut types: Vec<i64> = bp.materials.iter().map(|&(t, _)| t).collect();
            types.push(bp.product_type_id);
            let mkt = prices.mkt_of(conn, &types)?;
            if let Some(bids) = pricing::margin_bids(bp, &mkt, prices.refs()) {
                buy_whole_set(conn, station, &shortfall(bp, 1, me, &held), &bids, cycle)?;
                held = held_by_type(&conn.hangar_items()?);
            }
        }
        let runs = runs_on_hand(bp, planned, me, &held);
        if runs > 0 {
            install(conn, bp_row, runs, trips, cycle)?;
            return Ok(true);
        }
        cycle.notes.push(format!(
            "T2 copy {}: materials short (components need moon materials and reactions, stage D)",
            bp_row.type_id
        ));
    }
    Ok(false)
}

/// Buy every `(type, units)` of `short` from asks here at no more than
/// `bids`, or none of it when the asks here do not cover every line.
pub(crate) fn buy_whole_set(
    conn: &mut Conn,
    station: i64,
    short: &[(i64, i64)],
    bids: &BTreeMap<i64, f64>,
    cycle: &mut Cycle,
) -> Result<()> {
    let mut isk = conn.cash_balance()? * WALLET_SHARE;
    let mut plans = Vec::new();
    for &(t, n) in short {
        let book = conn.market_orders(t)?;
        let plan = crate::acquire::plan(&book, station, bids.get(&t).copied(), isk, n);
        if plan.iter().map(|&(_, k)| k).sum::<i64>() < n {
            return Ok(());
        }
        isk -= plan.iter().map(|&(a, k)| a.price * k as f64).sum::<f64>();
        plans.push((t, plan));
    }
    for (t, plan) in plans {
        for (ask, n) in plan {
            let Some(row) = noted(
                conn.buy(station, t, ask.price, n, 0, BROKER_FEE),
                cycle,
                "materials",
            )?
            else {
                break;
            };
            let got = n - row.vol_remaining.clamp(0, n);
            if got > 0 {
                cycle.bought.push((t, got, got as f64 * ask.price));
            }
        }
    }
    Ok(())
}

/// Step 5, **the laboratory** (SOC-B7): with a lab slot free
/// ([`LAB_SLOTS`]) and [`INVENTION_MIN_WALLET`] in the wallet, buy one
/// batch of datacores for the best inventable blueprint here from asks
/// here (players, hackers: no NPC sells a datacore) under the consumer's
/// cap, then start [`plan_lab`]'s job: an invention, or the copy one needs.
/// A refusal (a missing science or encryption skill, most often) is a note,
/// and an invention's learns its skills ([`learn_invention`], GAPS-B).
fn lab(
    conn: &mut Conn,
    station: i64,
    recipes: &industry::Table,
    prices: &mut Prices,
    cycle: &mut Cycle,
) -> Result<()> {
    let Some(jobs) = noted(conn.industry_jobs(), cycle, "jobs")? else {
        return Ok(());
    };
    let busy = jobs
        .iter()
        .filter(|j| !j.completed() && industry::is_science(j.activity_id))
        .count();
    if busy >= LAB_SLOTS || conn.cash_balance()? < INVENTION_MIN_WALLET {
        return Ok(());
    }
    let Some(blueprints) = noted(conn.blueprints(), cycle, "blueprints")? else {
        return Ok(());
    };
    let mut held = held_by_type(&conn.hangar_items()?);
    if buy_datacores(conn, station, recipes, prices, &blueprints, &held, cycle)? {
        held = held_by_type(&conn.hangar_items()?);
    }
    let (bp, activity, runs, licence, product) =
        match plan_lab(&blueprints, station, recipes, &held) {
            None => return Ok(()),
            Some(Lab::Copy {
                original,
                licensed_runs,
            }) => (original, industry::COPYING, 1, Some(licensed_runs), None),
            Some(Lab::Invent {
                copy,
                product,
                runs,
            }) => (copy, industry::INVENTION, runs, None, Some(product)),
        };
    match install_lab_job(conn, &bp, activity, runs, licence, product) {
        Ok(job_id) => cycle.lab.push((job_id, bp.type_id, activity, runs)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(e) => {
            cycle.notes.push(format!("lab {}: {e}", bp.type_id));
            if activity == industry::INVENTION {
                learn_invention(conn, recipes, bp.type_id, cycle)?;
            }
        }
    }
    Ok(())
}

/// **GAPS-B**: an invention refused (a missing science or encryption
/// skill, most often) learns the invention's own skills from its recipe:
/// each book bought at this station ([`crate::acquire::learn_skills`]),
/// injected and queued. A school station sells the encryption methods
/// (`econ_exchange::SCHOOL_INVENTION_BOOKS` **[A]**), so a builder there
/// can take up racial invention; one elsewhere notes what is missing.
fn learn_invention(
    conn: &mut Conn,
    recipes: &industry::Table,
    blueprint: i64,
    cycle: &mut Cycle,
) -> Result<()> {
    let Some(inv) = recipes
        .science(blueprint)
        .and_then(|s| s.invention.as_ref())
    else {
        return Ok(());
    };
    if let Some(got) = noted(
        crate::acquire::learn_skills(conn, &inv.skills, None),
        cycle,
        "invention skills",
    )? {
        cycle.notes.extend(got.missing);
        if !got.learned.is_empty() {
            cycle.notes.push(format!("queued {:?}", got.learned));
        }
    }
    Ok(())
}

/// One [`INVENTION_BATCH`] of datacores for the best-ranked blueprint here
/// with an invention, what the hangar lacks of it, from asks here under the
/// consumer's cap and inside [`WALLET_SHARE`]. `true`: something bought.
fn buy_datacores(
    conn: &mut Conn,
    station: i64,
    recipes: &industry::Table,
    prices: &mut Prices,
    blueprints: &[BlueprintRow],
    held: &BTreeMap<i64, i64>,
    cycle: &mut Cycle,
) -> Result<bool> {
    // T3-H: a relic is the Tech III laboratory's (`crate::t3::lab`).
    let invention = |t: i64| {
        recipes
            .science(t)
            .filter(|_| !recipes.is_relic(t))
            .and_then(|s| s.invention.as_ref())
    };
    let Some(inv) = blueprints
        .iter()
        .filter(|b| b.location_id == station && invention(b.type_id).is_some())
        .max_by_key(|b| (rank(b.type_id), std::cmp::Reverse(b.type_id)))
        .and_then(|b| invention(b.type_id))
    else {
        return Ok(false);
    };
    let mut bought = false;
    for &(t, per_run) in &inv.materials {
        let short = per_run * INVENTION_BATCH - held.get(&t).copied().unwrap_or(0);
        if short <= 0 {
            continue;
        }
        let cap = prices.reservation(conn, t)?;
        let book = conn.market_orders(t)?;
        let isk = conn.cash_balance()? * WALLET_SHARE;
        for (ask, n) in crate::acquire::plan(&book, station, cap, isk, short) {
            let Some(row) = noted(
                conn.buy(station, t, ask.price, n, 0, BROKER_FEE),
                cycle,
                "datacores",
            )?
            else {
                break;
            };
            let got = n - row.vol_remaining.clamp(0, n);
            if got > 0 {
                bought = true;
                cycle.bought.push((t, got, got as f64 * ask.price));
            }
        }
    }
    Ok(bought)
}

/// **Start a laboratory job**: `industryManager.InstallJob(job.dump())`
/// as [`Conn::install_job`] sends it, with the two keys only science reads
/// (`service/industry.rs` `install_request`) **[V]**: `licensedRuns`, the
/// runs each copy carries, for copying; `productTypeID`, which T2
/// blueprint an invention with several products aims at. Returns the job
/// id; an `IndustryValidationError` is a refusal.
pub fn install_lab_job(
    conn: &mut Conn,
    blueprint: &BlueprintRow,
    activity: i64,
    runs: i64,
    licensed_runs: Option<i64>,
    product_type_id: Option<i64>,
) -> Result<i64> {
    station_of(conn, "install lab job")?;
    let here = crate::verbs::job_location(blueprint.location_id, blueprint.flag_id);
    let mut dump = vec![
        ("activityID", PyValue::Int(activity)),
        ("blueprintID", PyValue::Int(blueprint.item_id)),
        ("runs", PyValue::Int(runs)),
        ("inputLocation", here.clone()),
        ("outputLocation", here),
    ];
    if let Some(n) = licensed_runs {
        dump.push(("licensedRuns", PyValue::Int(n)));
    }
    if let Some(t) = product_type_id {
        dump.push(("productTypeID", PyValue::Int(t)));
    }
    let dump = eve_server::pyutil::pydict(dump);
    let outcome = conn.call("industryManager", "InstallJob", vec![dump])?;
    match (outcome.refusal(), outcome.value()) {
        (None, Some(PyValue::Int(job_id))) => Ok(job_id),
        (Some(key), _) => Err(Error::Refused(format!("InstallJob: {key}"))),
        (None, other) => Err(Error::Refused(format!("InstallJob answered {other:?}"))),
    }
}

/// Install `runs` from `bp_row`; on a refusal, learn the builder's skills
/// the way [`crate::acquire`] does (a missing skill is the refusal a
/// fresh builder meets first) and note it.
pub(crate) fn install(
    conn: &mut Conn,
    bp_row: &BlueprintRow,
    runs: i64,
    trips: Option<crate::travel::Trips<'_>>,
    cycle: &mut Cycle,
) -> Result<()> {
    match conn.install_job(bp_row, runs) {
        Ok(job_id) => {
            cycle.installed.push((job_id, bp_row.type_id, runs));
            Ok(())
        }
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => {
            cycle.notes.push(format!("install {}: {e}", bp_row.type_id));
            let kit = crate::acquire::Kit {
                skills: BUILDER_SKILLS,
                ..crate::acquire::Kit::default()
            };
            let none = crate::acquire::Lacking {
                ship: false,
                module: false,
                ammo: false,
                in_capsule: false,
            };
            if let Some(got) = noted(
                crate::acquire::run_with(conn, &kit, none, trips),
                cycle,
                "skills",
            )? {
                cycle.notes.extend(got.missing);
                cycle.notes.extend(got.trips);
                if !got.learned.is_empty() {
                    cycle.notes.push(format!("queued {:?}", got.learned));
                }
            }
            Ok(())
        }
    }
}

/// **The miner's sale, docked**: refine the hangar's ore at the station
/// ([`Conn::reprocess`]; a refusal -- no refinery data, no money for the
/// tax -- is noted and the sale goes on), then sell every raw-material or
/// ore stack that some bid takes at [`sell_limit`] or better, at once in
/// one Sell-window call.
pub fn sell_goods(conn: &mut Conn) -> Result<Sold> {
    let station = station_of(conn, "sell_goods")?;
    let prices: BTreeMap<i64, f64> = conn.adjusted_prices()?.into_iter().collect();
    let before = conn.cash_balance()?;
    let ore: Vec<i64> = conn
        .hangar_items()?
        .iter()
        .filter(|r| !r.singleton && r.category_id == CATEGORY_ASTEROID)
        .map(|r| r.item_id)
        .collect();
    let (mut refined, mut not_refined) = (Vec::new(), None);
    if !ore.is_empty() {
        match conn.reprocess(&ore) {
            Ok(out) => refined = out,
            Err(Error::Refused(why) | Error::NotSent(why)) => not_refined = Some(why),
            Err(e) => return Err(e),
        }
    }
    let mut lines = Vec::new();
    for row in conn.hangar_items()?.iter().filter(|r| is_miner_goods(r)) {
        let book = conn.market_orders(row.type_id)?;
        let Some(limit) = sell_limit(prices.get(&row.type_id).copied(), &book) else {
            continue;
        };
        if book.best_bid().is_none_or(|b| b.price < limit) {
            continue;
        }
        lines.push(SellLine {
            station_id: station,
            type_id: row.type_id,
            item_id: row.item_id,
            price: limit,
            quantity: row.quantity,
        });
    }
    if !lines.is_empty() {
        conn.sell(&lines, 0, BROKER_FEE)?;
    }
    let after = conn.cash_balance()?;
    Ok(Sold {
        refined,
        not_refined,
        lines,
        before,
        after,
    })
}

/// **What a builder buys**: [`WANT_UNITS`] of every published mineral in
/// the client's own type table (the bundle's `typedata.tsv`), by type id.
pub fn wants_from(td: &eve_server::typedata::Table) -> Vec<(i64, i64)> {
    td.types()
        .filter(|t| t.published && t.group_id == GROUP_MINERAL)
        .map(|t| (t.type_id, WANT_UNITS))
        .collect()
}

/// [`wants_from`] the bundle at `dir` (`eve-bots --content DIR`); none when
/// it has no `typedata.tsv`.
pub fn wants_in(dir: &std::path::Path) -> std::result::Result<Vec<(i64, i64)>, String> {
    use eve_server::content::{Bundle, TYPEDATA};
    let (bundle, _) = Bundle::open(dir)?;
    match bundle.path(TYPEDATA) {
        Some(p) => Ok(wants_from(&eve_server::typedata::Table::load(&p)?)),
        None => Ok(Vec::new()),
    }
}

/// Unix seconds now.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::{BotConfig, Pace};
    use crate::pricing::BUY_CAP_OF_MKT;
    use eve_marshal::PyValue;
    use eve_server::econ_exchange::NPC_ORDER_BASE;
    use eve_server::item::{ItemType, SPAWNABLE};
    use eve_server::model::Character;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// An ask or bid; `npc` puts it in the exchange's id band.
    fn order(id: i64, price: f64, vol: i64, bid: bool, npc: bool) -> BookOrder {
        BookOrder {
            order_id: if npc { NPC_ORDER_BASE + id } else { id },
            type_id: 990_001,
            station_id: 69_000_001,
            price,
            vol_remaining: vol,
            bid,
        }
    }

    #[test]
    fn the_builder_is_a_civilian_row() {
        assert!(ROLE.civilian);
        assert_eq!(ROLE.name(), "builder");
        assert!(
            setup(Lacks {
                ship: true,
                module: true,
                ammo: true,
                isk: true
            })
            .is_empty()
        );
    }

    /// **FX6: the row docked runs the market, then waits its cadence**:
    /// one [`Act::Market`], and none again until the wait is over.
    #[test]
    fn the_builder_docked_runs_the_market_on_its_cadence() {
        let mut m = Memory::new("Builder1", brain::Policy::fast());
        let mut o = Observation::empty(Instant::now());
        o.place = brain::Place::Docked {
            station: 69_000_001,
        };
        let first = docked(&mut m, &o);
        assert_eq!(first[0], Act::Market);
        let Some(Act::Wait(wait)) = first.get(1).cloned() else {
            panic!("a wait after the market: {first:?}");
        };
        assert!(wait >= MARKET_EVERY.0 && wait <= MARKET_EVERY.1, "{wait:?}");
        // Woken early (a push): still waiting, no second cycle.
        o.now += wait / 2;
        assert!(matches!(docked(&mut m, &o)[..], [Act::Wait(_)]));
        // Its time again.
        o.now += wait;
        assert_eq!(docked(&mut m, &o)[0], Act::Market);
        // Repeated failures: a fresh connection, as every row.
        o.failures
            .insert(brain::ActKind::Market, brain::GIVE_UP_AFTER);
        assert_eq!(docked(&mut m, &o), vec![Act::Reconnect]);
    }

    /// The builder's wants are the published minerals of the type table
    /// (invented rows: two minerals, one unpublished, one module).
    #[test]
    fn a_builder_wants_the_published_minerals() {
        let row = |id: i64, group: i64, cat: i64, published: u8| {
            format!(
                "type\t{id}\t{group}\t{cat}\t{published}\t1\t0.01\t0\t0\t1\t0\t\t\tFixture {id}\n"
            )
        };
        let text = [
            row(990_001, GROUP_MINERAL, 4, 1),
            row(990_002, GROUP_MINERAL, 4, 1),
            row(990_003, GROUP_MINERAL, 4, 0),
            row(990_004, 53, 7, 1),
        ]
        .concat();
        let td = eve_server::typedata::Table::parse(&text).expect("the rows parse");
        assert_eq!(
            wants_from(&td),
            vec![(990_001, WANT_UNITS), (990_002, WANT_UNITS)]
        );
    }

    /// The ask (E10, on `P_ref` alone) is strictly under the ceiling and
    /// inside the clamp for every reference price and ceiling, the
    /// undercut anchor with no ceiling, and absent when there is no room.
    #[test]
    fn the_ask_is_under_the_ceiling_and_inside_the_clamp() {
        for p_ref in [0.37, 1.0, 12.5, 999.99, 250_000.0, 3.3e7] {
            for ceiling_of_ref in [0.3, 0.5, 1.0, 1.05, 1.10, 1.25, 1.6, 4.0] {
                let ceiling = p_ref * ceiling_of_ref;
                match ask_price(p_ref, Some(ceiling)) {
                    Some(ask) => {
                        assert!(ask < ceiling, "{ask} under {ceiling}");
                        assert!(ask + 1e-9 >= p_ref * pricing::BOT_PRICE_MIN);
                        assert!(ask <= anchor(p_ref), "{ask} at most the anchor");
                    }
                    None => assert!(
                        ceiling <= p_ref * pricing::BOT_PRICE_MIN + 2.0 * TICK,
                        "no ask under {ceiling} for {p_ref}"
                    ),
                }
            }
            assert_eq!(ask_price(p_ref, None), Some(anchor(p_ref)));
        }
        assert_eq!(ask_price(1.0, Some(TICK)), None);
        assert_eq!(ask_price(0.0, None), None);
    }

    fn anchor(p_ref: f64) -> f64 {
        pricing::up_to_cent(p_ref * (1.0 - pricing::UNDERCUT))
    }

    #[test]
    fn the_miner_takes_the_npc_floor_else_a_share_of_p_ref() {
        let book = Book {
            asks: vec![],
            bids: vec![
                order(1, 9.0, 10, true, false),
                order(2, 6.0, 10, true, true),
            ],
        };
        assert_eq!(sell_limit(Some(100.0), &book), Some(6.0));
        let human_only = Book {
            asks: vec![],
            bids: vec![order(1, 9.0, 10, true, false)],
        };
        assert_eq!(
            sell_limit(Some(100.0), &human_only),
            Some(down_to_cent(100.0 * SELL_FLOOR_OF_REF))
        );
        assert_eq!(sell_limit(None, &human_only), None);
    }

    /// **The row's test**, since E11 moved the demand budget into the
    /// server (SOC-A2): rounds of buys against a deep player ask and an NPC
    /// ask spend at most [`WALLET_SHARE`] of the wallet each, the NPC ask is
    /// never taken, and the wallet is never overdrawn.
    #[test]
    fn a_wants_round_spends_at_most_the_wallet_share_and_never_an_npc_ask() {
        let p_ref = 12.5;
        let book = Book {
            asks: vec![
                order(1, p_ref * 0.5, 1_000_000_000, false, true),
                order(2, p_ref * 0.9, 1_000_000_000, false, false),
            ],
            bids: vec![],
        };
        let mut wallet = 1e7;
        for _ in 0..10 {
            let isk = wallet * WALLET_SHARE;
            let mut spent = 0.0;
            for (ask, units) in buy_plan(&book, p_ref * pricing::BUY_CAP_OF_MKT, isk, 50_000) {
                assert!(!ask.is_npc(), "an NPC ask was bought");
                spent += units as f64 * ask.price;
            }
            assert!(spent > 0.0, "a round buys something");
            assert!(spent <= isk + 1e-6, "spent {spent} of {isk}");
            wallet -= spent;
            assert!(wallet >= 0.0);
        }
    }

    #[test]
    fn the_plan_buys_cheapest_first_and_never_above_the_cap() {
        let p_ref = 10.0;
        let book = Book {
            asks: vec![
                order(1, p_ref * BUY_CAP_OF_MKT * 2.0, 100, false, false),
                order(2, p_ref * BUY_CAP_OF_MKT * 0.8, 3, false, false),
                order(3, p_ref * BUY_CAP_OF_MKT * 0.5, 4, false, false),
            ],
            bids: vec![],
        };
        let plan = buy_plan(&book, p_ref * BUY_CAP_OF_MKT, 1e9, 5);
        let ids: Vec<i64> = plan.iter().map(|(o, _)| o.order_id).collect();
        assert_eq!(ids, vec![3, 2]);
        assert_eq!(plan.iter().map(|(_, u)| u).sum::<i64>(), 5);
        assert!(buy_plan(&book, p_ref * BUY_CAP_OF_MKT, 0.0, 5).is_empty());
    }

    // ---- over a real socket ------------------------------------------------------

    const MINER1: i64 = 90_000_001;
    const BUILDER1: i64 = 90_000_002;
    const ALICE: i64 = 90_000_003;
    /// Invented reference prices.
    const MINERAL_REF: f64 = 5.0;
    const MODULE_REF: f64 = 20_000.0;
    /// Invented: the builder's starting wallet, for the broker's fee.
    const BUILDER_WALLET: f64 = 10_000_000.0;
    /// Invented: Alice's, for her asks' and bids' broker fees and escrow.
    const ALICE_WALLET: f64 = 10_000_000.0;

    fn spawnable(name: &str) -> &'static ItemType {
        SPAWNABLE
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name} is spawnable"))
    }

    /// A mineral and a T1 module from the server's spawn table.
    fn goods() -> (&'static ItemType, &'static ItemType) {
        let mineral = spawnable("Tritanium");
        let module = spawnable("Miner I");
        assert!(
            FLOOR_GROUPS.contains(&mineral.group_id),
            "a floor-bid group"
        );
        assert!(CEILING_CATEGORIES.contains(&module.category_id));
        (mineral, module)
    }

    /// Prices for the two goods, and no NPC order for either: under O5 no
    /// NPC buys a mineral or sells a module (E5). Invented prices.
    fn content() -> eve_server::content::Content {
        let (mineral, module) = goods();
        let prices = eve_server::econ_prices::Table::parse(&[
            format!(
                "{}\t{MINERAL_REF}\tbase\t{MINERAL_REF}\t{}\t{}\t\t1",
                mineral.type_id, mineral.category_id, mineral.group_id
            ),
            format!(
                "{}\t{MODULE_REF}\trecipe\t{MODULE_REF}\t{}\t{}\t1\t1",
                module.type_id, module.category_id, module.group_id
            ),
        ])
        .expect("the rows parse");
        let mut c = eve_server::content::Content::fixture();
        c.insert("econ_prices", prices);
        c
    }

    fn start() -> SocketAddr {
        let pilot = |id, name: &str, ship, balance| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            balance,
            ..Character::default()
        };
        let mut config = eve_server::server::Config::new("127.0.0.1:0".parse().expect("literal"));
        config.sandbox.characters = vec![
            pilot(MINER1, "Miner1", 100_000_001, 0.0),
            pilot(BUILDER1, "Builder1", 100_000_002, BUILDER_WALLET),
            pilot(ALICE, "Alice", 100_000_003, ALICE_WALLET),
        ];
        config.content = Some(Arc::new(content()));
        let server = eve_server::server::Server::bind(config).expect("bind");
        let addr = server.local_addr().expect("local addr");
        std::thread::spawn(move || {
            let _ = server.run();
        });
        addr
    }

    /// **E10: the history read** answers over the wire: a type nobody has
    /// traded has no days, so its `P_mkt` is its `P_ref`.
    #[test]
    fn the_price_history_reads_over_the_wire_and_falls_back_to_p_ref() {
        let (mineral, _) = goods();
        let addr = start();
        let mut c = login(addr, "Builder1");
        let days = c
            .price_history(mineral.type_id)
            .expect("GetOldPriceHistory answers");
        assert!(days.is_empty(), "{days:?}");
        let mut prices = Prices::read(&mut c).expect("the prices read");
        let p_ref = prices.p_ref(mineral.type_id);
        assert!(p_ref.is_some(), "the fixture prices the mineral");
        assert_eq!(prices.p_mkt(&mut c, mineral.type_id).expect("read"), p_ref);
        assert_eq!(
            prices.daily_volume(&mut c, mineral.type_id).expect("read"),
            0.0
        );
    }

    /// Logged in the way the driver does it: with `BotConfig`'s token,
    /// which is never the name (CA19, ruling 2), then selected by name.
    fn login(addr: SocketAddr, name: &str) -> Conn {
        let config = BotConfig::new(name, addr, Pace::Fast);
        assert_ne!(config.token, config.name, "the name is never the token");
        let mut c = Conn::connect(addr, Duration::from_secs(10)).expect("connects");
        c.handshake_as(&config.token).expect("handshake");
        c.select_character_named(name).expect("selects by name");
        c
    }

    /// Setup only: a GM slash, named at the call site.
    fn gm(c: &mut Conn, line: &str) {
        c.call(
            "slash",
            "SlashCmd",
            vec![PyValue::Bytes(line.as_bytes().to_vec())],
        )
        .expect("the slash is answered");
    }

    /// Alice's standing bid for `units` of `type_id` at `price`: the buyer a
    /// miner sells into once no NPC buys minerals (O5).
    fn alice_bids(addr: SocketAddr, type_id: i64, price: f64, units: i64) {
        let mut alice = login(addr, "Alice");
        let station = alice.session().station_id().expect("docked");
        let row = alice
            .buy(station, type_id, price, units, ORDER_DAYS, BROKER_FEE)
            .expect("Alice bids");
        assert_eq!(row.vol_remaining, units, "the bid rests");
    }

    /// Alice's ask for `quantity` of `type_id` at `price`.
    fn alice_asks(addr: SocketAddr, type_id: i64, quantity: i64, price: f64) {
        let mut alice = login(addr, "Alice");
        gm(&mut alice, &format!("/createitem {type_id} {quantity}"));
        let station = alice.session().station_id().expect("docked");
        let stack = alice
            .hangar_items()
            .expect("the hangar lists")
            .into_iter()
            .find(|r| r.type_id == type_id)
            .expect("the stack");
        let line = SellLine {
            station_id: station,
            type_id,
            item_id: stack.item_id,
            price,
            quantity: stack.quantity,
        };
        alice
            .sell(&[line], ORDER_DAYS, BROKER_FEE)
            .expect("Alice lists");
    }

    /// **The row's test** (rewritten for O5): a miner with minerals in its
    /// hangar sells them into a player's bid at its floor or better, and its
    /// wallet rises, net of the sales tax.
    #[test]
    fn the_miner_sells_into_a_player_bid_and_its_wallet_rises() {
        let (mineral, _) = goods();
        let addr = start();
        alice_bids(addr, mineral.type_id, MINERAL_REF, 1_000);
        let mut c = login(addr, "Miner1");
        gm(&mut c, &format!("/createitem {} 500", mineral.type_id));
        let book = c.market_orders(mineral.type_id).expect("the book reads");
        assert!(book.npc_floor().is_none(), "no NPC buys a mineral (O5)");

        let sold = sell_goods(&mut c).expect("sells");
        assert!(
            sold.lines.iter().any(|l| l.type_id == mineral.type_id),
            "the mineral was offered: {sold:?}"
        );
        assert!(sold.after > sold.before, "the wallet rose: {sold:?}");
        let left: i64 = c
            .hangar_items()
            .expect("the hangar lists")
            .iter()
            .filter(|r| r.type_id == mineral.type_id)
            .map(|r| r.quantity)
            .sum();
        assert!(left < 500, "the stack went to the bid");
    }

    /// Run one driver bot as `name` in `role` until its log has an entry of
    /// `kind`, then stop it; that entry.
    fn drive_until(
        addr: SocketAddr,
        name: &str,
        role: crate::brain::RoleRef,
        wants: Vec<(i64, i64)>,
        kind: brain::ActKind,
    ) -> crate::driver::Entry {
        let mut config = BotConfig::new(name, addr, Pace::Fast).with_role(role);
        config.chat = false;
        config.policy.gm_setup = false;
        config.policy.wants = Arc::new(wants);
        let (handle, thread) = crate::driver::spawn(config);
        let deadline = Instant::now() + Duration::from_secs(30);
        let entry = loop {
            if let Some(e) = handle.log().into_iter().find(|e| e.kind == kind) {
                break e;
            }
            assert!(
                Instant::now() < deadline,
                "no {kind:?} entry: {:#?}",
                handle.log()
            );
            std::thread::sleep(Duration::from_millis(100));
        };
        handle.stop();
        let _ = thread.join();
        entry
    }

    /// **FX6, (a)** (rewritten for O5): a miner *bot* docked with goods in
    /// its hangar sells them into a player's bid through its own act
    /// ([`Act::Sell`]) and its wallet rises.
    #[test]
    fn a_docked_miner_bot_sells_through_its_act_and_its_wallet_rises() {
        let (mineral, _) = goods();
        let addr = start();
        alice_bids(addr, mineral.type_id, MINERAL_REF, 1_000);
        {
            let mut c = login(addr, "Miner1");
            gm(&mut c, &format!("/createitem {} 500", mineral.type_id));
            assert_eq!(c.cash_balance().expect("a wallet"), 0.0);
        }
        let sold = drive_until(
            addr,
            "Miner1",
            &crate::roles::MINE,
            vec![],
            brain::ActKind::Sell,
        );
        assert!(sold.verdict.is_ok(), "{sold:?}");
        assert!(
            sold.what.contains("wallet"),
            "the act line says what it did: {sold:?}"
        );

        let mut c = login(addr, "Miner1");
        assert!(c.cash_balance().expect("a wallet") > 0.0, "the wallet rose");
    }

    /// **FX6, (b)** (rewritten for O5): a `--loop builder` bot runs [`run`]
    /// docked through its own act ([`Act::Market`]): its module is listed at
    /// its markup and a player's mineral bought.
    #[test]
    fn a_builder_bot_runs_its_market_cycle_through_its_act() {
        let (mineral, module) = goods();
        let addr = start();
        alice_asks(
            addr,
            mineral.type_id,
            300,
            down_to_cent(MINERAL_REF * BUY_CAP_OF_MKT),
        );
        {
            let mut b = login(addr, "Builder1");
            gm(&mut b, &format!("/createitem {} 4", module.type_id));
        }
        let cycle = drive_until(
            addr,
            "Builder1",
            &ROLE,
            vec![(mineral.type_id, 100)],
            brain::ActKind::Market,
        );
        assert!(cycle.verdict.is_ok(), "{cycle:?}");

        let mut b = login(addr, "Builder1");
        let ask = ask_price(MODULE_REF, None).expect("an ask price");
        assert!(
            b.my_orders()
                .expect("my orders")
                .iter()
                .any(|o| !o.bid && o.type_id == module.type_id && o.price == ask),
            "a resting ask at the markup"
        );
        assert!(
            b.hangar_items()
                .expect("the hangar lists")
                .iter()
                .any(|r| r.type_id == mineral.type_id),
            "the mineral was bought"
        );
    }

    /// **The row's test** (rewritten for O5): the builder posts its module
    /// at its markup, one ask per (type, station), and buys a player's
    /// mineral with at most [`WALLET_SHARE`] of its wallet (the demand
    /// budget is the server's since E11).
    #[test]
    fn the_builder_posts_at_its_markup_and_buys_inside_its_wallet_share() {
        let (mineral, module) = goods();
        let addr = start();
        alice_asks(
            addr,
            mineral.type_id,
            300,
            down_to_cent(MINERAL_REF * BUY_CAP_OF_MKT),
        );

        let mut b = login(addr, "Builder1");
        gm(&mut b, &format!("/createitem {} 4", module.type_id));
        let wallet = b.cash_balance().expect("a wallet");
        let cycle = run(&mut b, &[(mineral.type_id, 100)], None).expect("a cycle");

        let ask = ask_price(MODULE_REF, None).expect("an ask price");
        let posted = cycle
            .posted
            .iter()
            .find(|l| l.type_id == module.type_id)
            .unwrap_or_else(|| panic!("the module was posted: {cycle:?}"));
        assert_eq!(posted.price, ask);

        let spent: f64 = cycle.bought.iter().map(|&(_, _, isk)| isk).sum();
        assert!(
            cycle
                .bought
                .iter()
                .any(|&(t, n, _)| t == mineral.type_id && n > 0),
            "the mineral was bought: {cycle:?}"
        );
        assert!(spent <= wallet * WALLET_SHARE + 1e-6, "{spent} of {wallet}");

        // A second cycle: the ask is not stacked, and not touched.
        gm(&mut b, &format!("/createitem {} 2", module.type_id));
        let again = run(&mut b, &[], None).expect("a cycle");
        assert!(again.posted.is_empty(), "{again:?}");
        assert!(
            again.repriced.is_empty() && again.waiting.is_empty(),
            "{again:?}"
        );
        let station = b.session().station_id().expect("docked");
        let asks = b
            .my_orders()
            .expect("my orders")
            .iter()
            .filter(|o| !o.bid && o.type_id == module.type_id && o.station_id == station)
            .count();
        assert_eq!(asks, 1, "one ask per (type, station)");
    }

    // ---- the industry act's pure rules (E9) -------------------------------------

    /// A recipe of invented ids, `materials` per run.
    fn recipe(time: i64, product_quantity: i64, materials: &[(i64, i64)]) -> Blueprint {
        Blueprint {
            blueprint_type_id: 990_500_001,
            product_type_id: 990_500_002,
            product_quantity,
            time,
            max_production_limit: 10,
            materials: materials.to_vec(),
            skills: vec![(industry::SKILL_INDUSTRY, 1)],
        }
    }

    /// A small deterministic stream (no dependency for a property test).
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }
        fn range(&mut self, lo: u64, hi: u64) -> u64 {
            lo + self.next() % (hi - lo + 1)
        }
    }

    /// **E9: the standing bid never exceeds the margin rule**, over 2,000
    /// generated recipes and prices: one run's materials at the bids plus
    /// the broker's fee cost no more than [`material_budget`], and no bid is
    /// above [`BUY_CAP_OF_MKT`] x its `P_ref` (here its `P_mkt`) nor under the
    /// clamp.
    #[test]
    fn the_margin_bids_never_exceed_the_margin_rule() {
        let mut r = Lcg(0x5eed);
        let mut some = 0;
        for _ in 0..2_000 {
            let n = r.range(1, 6) as usize;
            let materials: Vec<(i64, i64)> = (0..n)
                .map(|i| (990_600_000 + i as i64, r.range(1, 40_000) as i64))
                .collect();
            let bp = recipe(
                r.range(1, 10_000) as i64,
                r.range(1, 200) as i64,
                &materials,
            );
            let mut prices = BTreeMap::new();
            prices.insert(bp.product_type_id, r.range(1, 50_000_000) as f64 / 100.0);
            for &(t, _) in &materials {
                prices.insert(t, r.range(1, 100_000) as f64 / 100.0);
            }
            let Some(bids) = margin_bids(&bp, &prices) else {
                continue;
            };
            some += 1;
            let cost: f64 = bp
                .materials
                .iter()
                .map(|&(t, q)| q as f64 * bids[&t] * (1.0 + BROKER_FEE))
                .sum();
            let budget = material_budget(&bp, prices[&bp.product_type_id]);
            assert!(
                cost <= budget + 1e-6,
                "{cost} over {budget}: {bp:?} {prices:?}"
            );
            for (t, bid) in &bids {
                assert!(*bid >= TICK && *bid <= prices[t] * BUY_CAP_OF_MKT + 1e-9);
                assert!(
                    *bid + 1e-9 >= prices[t] * pricing::BOT_PRICE_MIN,
                    "inside the clamp"
                );
            }
        }
        assert!(some > 100, "the generator reaches the rule: {some}");
        // No price, no bid.
        let bp = recipe(10, 1, &[(990_600_000, 5)]);
        assert_eq!(margin_bids(&bp, &BTreeMap::new()), None);
    }

    /// **E9: one order per (type, station), repriced with modify, never
    /// inside the server's modification delay.**
    #[test]
    fn an_order_is_repriced_only_after_the_modification_delay() {
        let issued = 133_000_000_000_000_000;
        let own = OwnOrder {
            order: order(1, 10.0, 5, true, false),
            solar_system_id: 30_000_001,
            range: RANGE_STATION,
            issue_date: issued,
            last_state_change: None,
        };
        let delay =
            crate::verbs::MODIFICATION_DELAY.as_secs() as i64 * crate::verbs::FILETIME_SECOND;
        assert_eq!(reprice(&own, 10.0, issued), Reprice::Keep);
        assert_eq!(reprice(&own, 11.0, issued + delay - 1), Reprice::Wait);
        assert_eq!(reprice(&own, 11.0, issued + delay), Reprice::Modify(11.0));
        // After a modify, the delay runs from it.
        let modified = OwnOrder {
            last_state_change: Some(issued + delay),
            ..own
        };
        assert_eq!(reprice(&modified, 12.0, issued + delay + 1), Reprice::Wait);
        assert_eq!(
            reprice(&modified, 12.0, issued + 2 * delay),
            Reprice::Modify(12.0)
        );
    }

    /// The runs: [`JOB_TARGET`] over the time, then what the wallet buys at
    /// the bids, and what the hangar holds.
    #[test]
    fn the_runs_are_what_the_target_the_wallet_and_the_hangar_allow() {
        let bp = recipe(JOB_TARGET.as_secs() as i64 / 4, 1, &[(1, 10), (2, 3)]);
        assert_eq!(planned_runs(&bp), 4);
        assert_eq!(
            planned_runs(&recipe(JOB_TARGET.as_secs() as i64 * 3, 1, &[])),
            1
        );
        let bids: BTreeMap<i64, f64> = [(1, 1.0), (2, 2.0)].into_iter().collect();
        let per_run = (10.0 * 1.0 + 3.0 * 2.0) * (1.0 + BROKER_FEE);
        let empty = BTreeMap::new();
        assert_eq!(affordable_runs(&bp, 4, 0, &empty, &bids, per_run * 2.5), 2);
        assert_eq!(affordable_runs(&bp, 4, 0, &empty, &bids, per_run * 99.0), 4);
        assert_eq!(affordable_runs(&bp, 4, 0, &empty, &bids, per_run * 0.5), 0);
        let held: BTreeMap<i64, i64> = [(1, 25), (2, 9)].into_iter().collect();
        assert_eq!(runs_on_hand(&bp, 4, 0, &held), 2);
        assert_eq!(shortfall(&bp, 3, 0, &held), vec![(1, 5)]);
        // Two runs on hand cost nothing more.
        assert_eq!(affordable_runs(&bp, 4, 0, &held, &bids, 0.0), 2);
    }

    /// The ladder: the first rung with a recipe not held, as the rung's
    /// blueprints that have one (any one of them holds the rung); the first
    /// with any wallet that pays for it, the next with a cushion; the best
    /// idle original here built first.
    #[test]
    fn the_ladder_is_climbed_in_order_and_the_best_rung_is_built_first() {
        let (r0, r1) = (BLUEPRINT_RUNGS[0][0], BLUEPRINT_RUNGS[1][0]);
        let (r2, r2b) = (BLUEPRINT_RUNGS[2][0], BLUEPRINT_RUNGS[2][1]);
        let rows: Vec<String> = [r0, r1, r2, r2b]
            .iter()
            .flat_map(|bp| {
                [
                    format!("{bp}\t1\tmaterial\t990600001\t10\t\t60\t10"),
                    format!("{bp}\t1\tproduct\t990600002\t1\t\t60\t10"),
                ]
            })
            .collect();
        let recipes = industry::Table::parse(&rows, &eve_server::econ_prices::Table::default())
            .expect("the rows parse");
        assert_eq!(next_rung(&BLUEPRINT_RUNGS, &[], &recipes), Some(vec![r0]));
        assert_eq!(next_rung(&BLUEPRINT_RUNGS, &[r0], &recipes), Some(vec![r1]));
        assert_eq!(
            next_rung(&BLUEPRINT_RUNGS, &[r0, r1], &recipes),
            Some(vec![r2, r2b]),
            "a rung is its blueprints with a recipe"
        );
        assert_eq!(
            next_rung(&BLUEPRINT_RUNGS, &[r0, r1, r2b], &recipes),
            None,
            "one blueprint holds its rung, and the rungs above have no recipe"
        );
        assert_eq!(
            lacking_rungs(&BLUEPRINT_RUNGS, &[r1], &recipes),
            vec![vec![r0], vec![r2, r2b]],
            "a higher rung held never hides a lower one"
        );
        assert!(may_climb(false, 100.0, 100.0));
        assert!(!may_climb(true, 100.0, 100.0));
        assert!(may_climb(true, 100.0 * CLIMB_CUSHION, 100.0));

        const HERE: i64 = 60_000_001;
        let held = [
            bp_row(1, r0, HERE, -1, None),
            bp_row(2, r2b, HERE, -1, None),
            bp_row(3, r1, HERE, -1, Some(7)),
            bp_row(4, r2, 60_000_002, -1, None),
            bp_row(5, r1, HERE, 5, None),
            bp_row(6, 990_600_009, HERE, -1, None),
        ];
        let order: Vec<i64> = candidates(&held, HERE, &recipes)
            .iter()
            .map(|b| b.item_id)
            .collect();
        assert_eq!(
            order,
            [2, 1],
            "idle originals here with a recipe, best rung first"
        );
    }

    fn bp_row(
        item: i64,
        type_id: i64,
        location_id: i64,
        runs: i64,
        job_id: Option<i64>,
    ) -> BlueprintRow {
        BlueprintRow {
            item_id: item,
            type_id,
            location_id,
            flag_id: industry::FLAG_HANGAR,
            runs,
            material_efficiency: 0,
            job_id,
        }
    }

    /// PROG-NULL: a mineral only the deep belts yield makes the rung
    /// makeable exactly while a miner out there reports it.
    #[test]
    fn a_deep_miners_report_makes_its_mineral_obtainable() {
        const RARE: i64 = 990_630_001;
        const HOME: i64 = 39_400_001;
        let bp = BLUEPRINT_RUNGS[0][0];
        let rows = [
            format!("{bp}\t1\tmaterial\t{RARE}\t10\t\t60\t10"),
            format!("{bp}\t1\tproduct\t990630009\t1\t\t60\t10"),
        ];
        let recipes = industry::Table::parse(&rows, &eve_server::econ_prices::Table::default())
            .expect("the rows parse");
        // The mineral is yielded in a system that is not home.
        let supply = crate::supply::Supply::from_rocks([(HOME + 1, 1)], |_| vec![RARE]);
        let board = crate::rare_ore::RareBoard::new();
        let local = Some(Local {
            supply: &supply,
            system: HOME,
            rare: Some(&board),
        });
        assert!(!makeable(local, &recipes, bp), "nobody mines it yet");
        board.report("Deep1", HOME + 1, vec![RARE]);
        assert!(makeable(local, &recipes, bp), "a miner out there cuts it");
        board.withdraw("Deep1");
        assert!(!makeable(local, &recipes, bp), "and not once it is gone");
    }

    /// GB-FIX: the gate soak's seven builders each held a blueprint whose
    /// mineral no miner in their system brought, and never looked past it.
    /// A blueprint the supply rules out holds no rung and is no choice for
    /// one; the starter rung (Tritanium and Pyerite only) is what is left,
    /// and holding it satisfies it.
    #[test]
    fn a_blueprint_its_miners_cannot_feed_holds_no_rung_and_is_no_choice() {
        const MINERAL_X: i64 = 990_620_001;
        const MINERAL_Y: i64 = 990_620_002;
        const HOME: i64 = 39_300_001;
        let (r0, r1) = (BLUEPRINT_RUNGS[0][0], BLUEPRINT_RUNGS[1][0]);
        let starter = STARTER_AMMO[0];
        // r0 and r1 need Y; the starter needs only X.
        let rows: Vec<String> = [(r0, MINERAL_Y), (r1, MINERAL_Y), (starter, MINERAL_X)]
            .iter()
            .flat_map(|&(bp, m)| {
                [
                    format!("{bp}\t1\tmaterial\t{m}\t10\t\t60\t10"),
                    format!("{bp}\t1\tproduct\t990620009\t1\t\t60\t10"),
                ]
            })
            .collect();
        let recipes = industry::Table::parse(&rows, &eve_server::econ_prices::Table::default())
            .expect("the rows parse");
        // One ore here yields X; Y is yielded by an ore somewhere else.
        let supply = crate::supply::Supply::from_rocks([(HOME, 1), (HOME + 1, 2)], |ore| {
            vec![[MINERAL_X, MINERAL_Y][ore as usize - 1]]
        });
        let local = Some(Local {
            supply: &supply,
            system: HOME,
            rare: None,
        });
        assert!(!makeable(local, &recipes, r0), "Y is not brought here");
        assert!(makeable(local, &recipes, starter));
        assert!(makeable(None, &recipes, r0), "no supply table: no filter");
        assert!(!makeable(local, &recipes, 990_620_099), "no recipe");
        let ok = |t: i64| makeable(local, &recipes, t);

        assert_eq!(
            lacking_rungs_by(&BLUEPRINT_RUNGS, &[], &recipes, &ok),
            vec![vec![starter]],
            "every rung but the starter is ruled out"
        );
        assert_eq!(
            lacking_rungs_by(&BLUEPRINT_RUNGS, &[r0, r1], &recipes, &ok),
            vec![vec![starter]],
            "a held blueprint nobody can feed holds nothing"
        );
        assert!(lacking_rungs_by(&BLUEPRINT_RUNGS, &[starter], &recipes, &ok).is_empty());
        assert_eq!(
            lacking_rungs_by(&BLUEPRINT_RUNGS, &[], &recipes, &|_| true),
            lacking_rungs(&BLUEPRINT_RUNGS, &[], &recipes),
            "everything makeable is the old ladder"
        );

        // The starter is the last rung and the last built.
        assert_eq!(rung_of(starter), Some(STARTER_RUNG));
        const HERE: i64 = 60_000_001;
        let held = [
            bp_row(1, starter, HERE, -1, None),
            bp_row(2, r1, HERE, -1, None),
        ];
        let order: Vec<i64> = candidates(&held, HERE, &recipes)
            .iter()
            .map(|b| b.item_id)
            .collect();
        assert_eq!(order, [2, 1], "a higher rung first, the starter last");
    }

    /// The flat ladder is the rungs in order, every blueprint once, and the
    /// first rung is one blueprint (the one the bot tests name).
    #[test]
    fn the_flat_ladder_is_the_rungs_in_order() {
        let flat: Vec<i64> = BLUEPRINT_RUNGS
            .iter()
            .flat_map(|r| r.iter().copied())
            .collect();
        assert_eq!(flat, BLUEPRINT_LADDER);
        let mut seen = std::collections::BTreeSet::new();
        assert!(flat.iter().all(|t| seen.insert(*t)), "no blueprint twice");
        assert_eq!(BLUEPRINT_RUNGS[0], &[BLUEPRINT_LADDER[0]]);
        for (i, rung) in BLUEPRINT_RUNGS.iter().enumerate() {
            assert!(!rung.is_empty());
            assert!(rung.iter().all(|&t| rung_of(t) == Some(i)));
        }
    }

    // Invented ids for the invention rules.
    const T1_BP: i64 = 990_610_001;
    const T1_PRODUCT: i64 = 990_610_002;
    const T2_BP: i64 = 990_610_003;
    const T2_BP_UNBUILT: i64 = 990_610_004;
    const T2_PRODUCT: i64 = 990_610_005;
    const CORE_A: i64 = 990_610_011;
    const CORE_B: i64 = 990_610_012;
    const COMPONENT: i64 = 990_610_021;
    const MINERAL: i64 = 990_610_031;
    const HERE: i64 = 60_000_011;
    /// Datacores per attempt, invented.
    const CORES_A: i64 = 2;
    const CORES_B: i64 = 3;

    /// A T1 blueprint inventing two T2 blueprints, only the second of
    /// which has a recipe; the T2 recipe eats a component.
    fn invention_recipes() -> industry::Table {
        let rows = [
            format!("{T1_BP}\t1\tactivity\t0\t0\t\t60\t30"),
            format!("{T1_BP}\t1\tmaterial\t{MINERAL}\t10\t\t60\t30"),
            format!("{T1_BP}\t1\tproduct\t{T1_PRODUCT}\t1\t\t60\t30"),
            format!("{T1_BP}\t5\tactivity\t0\t0\t\t60\t30"),
            format!("{T1_BP}\t8\tactivity\t0\t0\t\t60\t30"),
            format!("{T1_BP}\t8\tmaterial\t{CORE_A}\t{CORES_A}\t\t60\t30"),
            format!("{T1_BP}\t8\tmaterial\t{CORE_B}\t{CORES_B}\t\t60\t30"),
            format!("{T1_BP}\t8\tproduct\t{T2_BP_UNBUILT}\t1\t0.5\t60\t30"),
            format!("{T1_BP}\t8\tproduct\t{T2_BP}\t1\t0.5\t60\t30"),
            format!("{T2_BP}\t1\tactivity\t0\t0\t\t60\t10"),
            format!("{T2_BP}\t1\tmaterial\t{COMPONENT}\t4\t\t60\t10"),
            format!("{T2_BP}\t1\tproduct\t{T2_PRODUCT}\t1\t\t60\t10"),
        ];
        industry::Table::parse(&rows, &eve_server::econ_prices::Table::default())
            .expect("the rows parse")
    }

    /// **SOC-B7, the laboratory's plan**: invent from a copy when the
    /// datacores cover it, as many attempts as they, the copy and the batch
    /// allow; else copy the original, once, and only with datacores for an
    /// attempt; the product is the T2 blueprint that has a recipe.
    #[test]
    fn the_lab_copies_then_invents_as_far_as_the_datacores_go() {
        let recipes = invention_recipes();
        let inv = recipes
            .science(T1_BP)
            .and_then(|s| s.invention.as_ref())
            .expect("an invention");
        assert_eq!(invention_product(inv, &recipes), Some(T2_BP));
        assert_eq!(invented_types(&recipes).len(), 2);
        let cores = |sets: i64| -> BTreeMap<i64, i64> {
            [(CORE_A, CORES_A * sets), (CORE_B, CORES_B * sets + 1)]
                .into_iter()
                .collect()
        };
        assert_eq!(datacore_sets(inv, &cores(0), 99), 0);
        assert_eq!(datacore_sets(inv, &cores(4), 99), 4);
        assert_eq!(datacore_sets(inv, &cores(4), 3), 3);

        let original = bp_row(1, T1_BP, HERE, -1, None);
        // No datacores: nothing, not even a copy.
        assert_eq!(plan_lab(&[original], HERE, &recipes, &cores(0)), None);
        // Datacores and no copy: copy, once, with a batch's licence.
        assert_eq!(
            plan_lab(&[original], HERE, &recipes, &cores(1)),
            Some(Lab::Copy {
                original,
                licensed_runs: INVENTION_BATCH
            })
        );
        // A copy here: invent, the runs what the cores and the copy allow.
        let copy = bp_row(2, T1_BP, HERE, 3, None);
        assert_eq!(
            plan_lab(&[original, copy], HERE, &recipes, &cores(5)),
            Some(Lab::Invent {
                copy,
                product: T2_BP,
                runs: 3
            })
        );
        assert_eq!(
            plan_lab(&[original, copy], HERE, &recipes, &cores(2)),
            Some(Lab::Invent {
                copy,
                product: T2_BP,
                runs: 2
            })
        );
        // A copy busy in a job: no second copy, no invention.
        let busy = bp_row(2, T1_BP, HERE, 3, Some(9));
        assert_eq!(plan_lab(&[original, busy], HERE, &recipes, &cores(5)), None);
        // Elsewhere: nothing.
        let away = bp_row(1, T1_BP, HERE + 1, -1, None);
        assert_eq!(plan_lab(&[away], HERE, &recipes, &cores(5)), None);
        // A T2 copy is built, never invented from or copied.
        let t2 = bp_row(3, T2_BP, HERE, 1, None);
        let t2_unbuilt = bp_row(4, T2_BP_UNBUILT, HERE, 1, None);
        assert_eq!(plan_lab(&[t2], HERE, &recipes, &cores(5)), None);
        let none = std::collections::BTreeSet::new();
        let built: Vec<i64> =
            t2_candidates(&[original, copy, t2, t2_unbuilt], HERE, &recipes, &none)
                .iter()
                .map(|b| b.item_id)
                .collect();
        assert_eq!(built, [3], "only an invented copy with a recipe");
    }

    // ---- CAP-C: the capital ladder, its inputs and its run times -------------

    use crate::hulls::world::*;

    /// The world before X-PI3: the farmers stop at P2, nobody makes P3 or
    /// P4 (the CAP-C tests below pin what that blocks).
    const NO_PI_HIGH: Made = Made {
        pi_p3: false,
        pi_p4: false,
        ..MADE_TODAY
    };

    /// A world where only `made` is made.
    fn made(pi_p4: bool, reactions: bool) -> Made {
        Made {
            pi_p4,
            reactions,
            ..MADE_TODAY
        }
    }

    #[test]
    fn a_part_nobody_makes_blocks_everything_built_from_it() {
        let (c, recipes) = capital();
        let blocked = |m: Made, product: i64| {
            Sourcing::with(&c, &recipes, m).blocked(recipes.find(bp(product)).expect("a recipe"))
        };
        // A world with no reactor yet, as when CAP-C was written.
        let today = made(false, false);
        // A mineral and a P2 good: made (the miners, the farmers).
        assert!(blocked(today, COMPONENT_OK).is_empty());
        // Loot and a P2 good.
        assert!(blocked(today, CONDUIT).is_empty());
        // A component that takes a P4 good or a composite: nobody makes either.
        assert_eq!(blocked(today, COMPONENT_P4), vec![P4]);
        assert_eq!(blocked(today, COMPONENT_RCF), vec![COMPOSITE]);
        assert_eq!(blocked(today, ADVANCED), vec![COMPOSITE]);
        // A hull is as blocked as its parts, and names the leaf.
        assert!(blocked(today, CARRIER).is_empty(), "OK component, conduit");
        assert!(blocked(today, FREIGHTER).is_empty());
        // A source appears and the blocked recipe is free.
        assert!(blocked(made(true, false), COMPONENT_P4).is_empty());
        assert_eq!(blocked(made(true, false), COMPONENT_RCF), vec![COMPOSITE]);
        assert!(blocked(made(false, true), COMPONENT_RCF).is_empty());
        assert!(blocked(made(false, true), ADVANCED).is_empty());
    }

    #[test]
    fn a_built_part_is_as_makeable_as_its_own_recipe() {
        let (c, recipes) = capital();
        // A hull of the P4 component: invented, over the world's recipes.
        let mut rows: Vec<String> = Vec::new();
        for (hull, parts) in [
            (1, &[COMPONENT_P4][..]),
            (2, &[COMPONENT_OK, GHOST_PART][..]),
        ] {
            let product = 990_710_300 + hull;
            rows.push(format!(
                "{}\t1\tproduct\t{product}\t1\t\t1000\t1",
                bp(product)
            ));
            for m in parts {
                rows.push(format!("{}\t1\tmaterial\t{m}\t5\t\t1000\t1", bp(product)));
            }
        }
        let mut all: Vec<_> = recipes.blueprints.to_vec();
        let extra = industry::Table::parse(&rows, &eve_server::econ_prices::Table::default())
            .expect("the rows parse");
        all.extend(extra.blueprints.iter().cloned());
        all.sort_by_key(|b| b.blueprint_type_id);
        let both = industry::Table {
            blueprints: all,
            ..recipes.clone()
        };
        let s = Sourcing::with(&c, &both, NO_PI_HIGH);
        let blocked = |product: i64| s.blocked(both.find(bp(product)).expect("a recipe"));
        assert_eq!(
            blocked(990_710_301),
            vec![P4],
            "the leaf, not the component"
        );
        assert_eq!(
            blocked(990_710_302),
            vec![GHOST_PART],
            "a built part with no recipe is nobody's"
        );
        assert_eq!(
            Sourcing::with(
                &c,
                &both,
                Made {
                    built: false,
                    ..MADE_TODAY
                }
            )
            .blocked(both.find(bp(CARRIER)).expect("a recipe")),
            vec![COMPONENT_OK, CONDUIT],
            "no builder makes parts: every part is blocked"
        );
    }

    #[test]
    fn zero_point_condensate_blocks_whatever_takes_it_in_any_world() {
        let (c, _) = capital();
        let rows = [
            format!(
                "{}\t1\tproduct\t{COMPONENT_OK}\t1\t\t16000\t1",
                bp(COMPONENT_OK)
            ),
            format!(
                "{}\t1\tmaterial\t{ZERO_POINT}\t90\t\t16000\t1",
                bp(COMPONENT_OK)
            ),
        ];
        let recipes = industry::Table::parse(&rows, &eve_server::econ_prices::Table::default())
            .expect("the rows parse");
        let everything = Made {
            pi_basic: true,
            pi_p3: true,
            pi_p4: true,
            reactions: true,
            loot: true,
            built: true,
        };
        let s = Sourcing::with(&c, &recipes, everything);
        assert_eq!(
            s.blocked(recipes.find(bp(COMPONENT_OK)).expect("a recipe")),
            vec![ZERO_POINT]
        );
    }

    #[test]
    fn a_capital_blueprint_its_inputs_cannot_reach_holds_no_rung() {
        let (c, recipes) = capital();
        let capital_rungs = crate::hulls::capital_rungs(&c, &recipes);
        let rungs = ladder_with(&capital_rungs);
        // A world with no reactor yet: composites have no maker.
        let no_reactor = Made {
            reactions: false,
            ..NO_PI_HIGH
        };
        let sourcing = Sourcing::with(&c, &recipes, no_reactor);
        let ok = |t: i64| makeable_with(None, Some(&sourcing), &recipes, t);
        let lacking = lacking_rungs_by(&rungs, &[], &recipes, &ok);
        // The T1 ladder has no recipe here; what is left is capital.
        assert_eq!(
            lacking,
            vec![
                vec![bp(COMPONENT_OK)],
                vec![bp(CONDUIT)],
                vec![bp(CARRIER), bp(FREIGHTER)],
            ],
            "the P4 and composite components, and the advanced one, are no choice"
        );
        // Holding a blocked blueprint holds nothing.
        let held = [bp(COMPONENT_P4), bp(ADVANCED)];
        assert_eq!(lacking_rungs_by(&rungs, &held, &recipes, &ok), lacking);
        // Holding an unblocked one holds its rung.
        let held = [bp(COMPONENT_OK), bp(CARRIER)];
        assert_eq!(
            lacking_rungs_by(&rungs, &held, &recipes, &ok),
            vec![vec![bp(CONDUIT)]]
        );
        // A reactor and a P4 farmer appear: the blocked ones are choices.
        let sourcing = Sourcing::with(&c, &recipes, made(true, true));
        let ok = |t: i64| makeable_with(None, Some(&sourcing), &recipes, t);
        assert_eq!(
            lacking_rungs_by(&rungs, &[], &recipes, &ok),
            vec![
                vec![bp(COMPONENT_OK), bp(COMPONENT_P4), bp(COMPONENT_RCF)],
                vec![bp(CONDUIT)],
                vec![bp(CARRIER), bp(FREIGHTER)],
                vec![bp(ADVANCED)],
            ]
        );
        // No sourcing at all (no bundle): the old rule, nothing ruled out.
        assert!(makeable_with(None, None, &recipes, bp(COMPONENT_RCF)));
    }

    #[test]
    fn the_capital_rungs_follow_the_starter_and_the_t1_ladder_is_unchanged() {
        let (c, recipes) = capital();
        let capital_rungs = crate::hulls::capital_rungs(&c, &recipes);
        let rungs = ladder_with(&capital_rungs);
        assert_eq!(rungs.len(), BLUEPRINT_RUNGS.len() + capital_rungs.len());
        assert_eq!(rungs[..BLUEPRINT_RUNGS.len()], BLUEPRINT_RUNGS[..]);
        assert_eq!(
            rungs[STARTER_RUNG], STARTER_AMMO,
            "the starter stays first of the tail"
        );
        assert_eq!(
            rungs[BLUEPRINT_RUNGS.len()],
            &[bp(COMPONENT_OK), bp(COMPONENT_P4), bp(COMPONENT_RCF)]
        );
        assert_eq!(ladder_with(&[]), BLUEPRINT_RUNGS.to_vec());
        assert_eq!(
            rung_of(bp(CARRIER)),
            None,
            "capital blueprints are off the T1 ladder"
        );
    }

    /// A capital blueprint ranks with the starter, under every T1 rung; a
    /// hull builder's standing bids for a component move the component to
    /// the front (`promote_demanded`), which is how a hull gets its parts.
    #[test]
    fn capital_blueprints_are_built_after_the_ladder_and_when_demanded() {
        let (_, recipes) = capital();
        let (r0, r1) = (BLUEPRINT_RUNGS[0][0], BLUEPRINT_RUNGS[1][0]);
        let mut rows: Vec<String> = Vec::new();
        for bp_id in [r0, r1] {
            rows.push(format!("{bp_id}\t1\tproduct\t990710401\t1\t\t60\t10"));
            rows.push(format!("{bp_id}\t1\tmaterial\t{MINERAL}\t10\t\t60\t10"));
        }
        let t1 = industry::Table::parse(&rows, &eve_server::econ_prices::Table::default())
            .expect("the rows parse");
        let mut all = recipes.blueprints.clone();
        all.extend(t1.blueprints.iter().cloned());
        all.sort_by_key(|b| b.blueprint_type_id);
        let both = industry::Table {
            blueprints: all,
            ..recipes.clone()
        };
        const HERE: i64 = 60_000_001;
        let held = [
            bp_row(1, bp(COMPONENT_OK), HERE, -1, None),
            bp_row(2, r0, HERE, -1, None),
            bp_row(3, r1, HERE, -1, None),
        ];
        let order = |demand: &BTreeMap<i64, i64>| -> Vec<i64> {
            promote_demanded(candidates(&held, HERE, &both), demand)
                .iter()
                .map(|b| b.item_id)
                .collect()
        };
        assert_eq!(order(&BTreeMap::new()), [3, 2, 1], "the ladder first");
        let demand = BTreeMap::from([(bp(COMPONENT_OK), 20)]);
        assert_eq!(order(&demand), [1, 3, 2], "a demanded component first");
    }

    /// Run times are game seconds: the server ends a job `clock.span(nominal)`
    /// after its start, so the scale read off the end is the clock's, at any
    /// scale, and the runs planned are the same at every one.
    #[test]
    fn run_times_are_game_seconds_and_the_pace_reads_the_scale() {
        let (_, recipes) = capital();
        let hull = recipes.find(bp(CARRIER)).expect("a recipe");
        let part = recipes.find(bp(COMPONENT_OK)).expect("a recipe");
        // A hull is one run, a component one run (2 h over 4.4 h, at least 1).
        assert_eq!((planned_runs(hull), planned_runs(part)), (1, 1));
        assert_eq!(nominal_secs(hull, 1), hull.time);
        assert!(
            nominal_secs(hull, 1) >= LONG_JOB_SECS,
            "a hull holds the slot for days"
        );
        assert!(
            nominal_secs(part, 1) < LONG_JOB_SECS,
            "a component does not"
        );
        let now = 1_000_000_000_000_i64;
        for scale in [1_u32, 8, 60] {
            let clock = eve_server::game_clock::GameClock::new(scale).expect("a legal scale");
            let nominal = nominal_secs(hull, 1);
            let end = now + clock.span(nominal * industry::FILETIME_SECOND);
            let pace = pace_of(nominal, end, now).expect("a job still running");
            assert!(
                (pace - f64::from(scale)).abs() < 1e-6,
                "scale {scale} read as {pace}"
            );
            // Half-way through, the pace is the same.
            let half = now + (end - now) / 2;
            let pace = pace_of(nominal, end, half).expect("still running");
            assert!(
                pace > f64::from(scale),
                "the nominal is the whole job, the wall time is half"
            );
        }
        assert_eq!(pace_of(hull.time, now, now), None, "a job that is over");
        assert_eq!(pace_of(hull.time, now - 1, now), None);
    }

    /// With what the bots make today the PI farmer's P3 and P4 count as
    /// made (X-PI3; `pi_p3.rs` founds the colonies), so a hull whose recipe
    /// needs only those, minerals, P2 and loot holds a rung.
    #[test]
    fn today_the_bots_make_pi_p3_and_p4_goods() {
        const { assert!(MADE_TODAY.pi_basic && MADE_TODAY.loot && MADE_TODAY.built) };
        // The reactor career (MN-G) makes reaction products.
        const { assert!(MADE_TODAY.reactions) };
        const { assert!(MADE_TODAY.pi_p3 && MADE_TODAY.pi_p4) };
    }

    // ---- T3-F: the Tech 3 chain ----------------------------------------------

    // Invented ids (no client value). Blueprints are `product + 1_000`.
    const HYBRID: i64 = 990_730_001;
    const SALVAGE: i64 = 990_730_002;
    const T3_HULL: i64 = 990_730_003;
    const T3_SUB: i64 = 990_730_004;
    const T2_HULL: i64 = 990_730_005;
    const RELIC: i64 = 990_730_101;
    const T2_SEED: i64 = 990_730_102;
    const SHEET_MINERAL: i64 = 990_730_201;

    /// `(content, recipes)`: a hybrid component made of salvage and a
    /// mineral; a T3 hull and subsystem (ids *below* the T2 hull's, so the
    /// order under test is not the id order) both made of the component and
    /// invented from a relic; a T2 hull invented from an ordinary seed.
    fn t3_world() -> (eve_server::content::Content, industry::Table) {
        use crate::hulls::{GROUP_ANCIENT_SALVAGE, GROUP_HYBRID_COMPONENT};
        let mut text = String::new();
        for (id, group, cat) in [
            (HYBRID, GROUP_HYBRID_COMPONENT, 17),
            (SALVAGE, GROUP_ANCIENT_SALVAGE, 17),
            (SHEET_MINERAL, GROUP_MINERAL, 4),
        ] {
            text.push_str(&format!(
                "type\t{id}\t{group}\t{cat}\t1\t1\t1.0\t0.0\t1.0\t1.0\t0\t\t\tinvented\n"
            ));
        }
        let mut c = eve_server::content::Content::fixture();
        c.insert(
            "typedata",
            eve_server::typedata::Installed::build(
                eve_server::typedata::Table::parse(&text).expect("rows parse"),
            ),
        );
        let b = |p: i64| p + 1_000;
        let mut rows = vec![
            format!("{}\t1\tproduct\t{HYBRID}\t1\t\t600\t1", b(HYBRID)),
            format!("{}\t1\tmaterial\t{SALVAGE}\t7\t\t600\t1", b(HYBRID)),
            format!("{}\t1\tmaterial\t{SHEET_MINERAL}\t9\t\t600\t1", b(HYBRID)),
        ];
        for p in [T3_HULL, T3_SUB, T2_HULL] {
            rows.push(format!("{}\t1\tproduct\t{p}\t1\t\t6000\t1", b(p)));
            rows.push(format!("{}\t1\tmaterial\t{HYBRID}\t5\t\t6000\t1", b(p)));
        }
        // The relic: invention only, two T3 products.
        rows.push(format!("{RELIC}\t8\tactivity\t0\t0\t\t3600\t300"));
        for p in [T3_HULL, T3_SUB] {
            rows.push(format!("{RELIC}\t8\tproduct\t{}\t1\t0.3\t3600\t300", b(p)));
        }
        // An ordinary T1 seed inventing the T2 hull.
        rows.push(format!("{T2_SEED}\t1\tactivity\t0\t0\t\t60\t30"));
        rows.push(format!(
            "{T2_SEED}\t1\tproduct\t{SHEET_MINERAL}\t1\t\t60\t30"
        ));
        rows.push(format!("{T2_SEED}\t8\tactivity\t0\t0\t\t60\t30"));
        rows.push(format!(
            "{T2_SEED}\t8\tproduct\t{}\t1\t0.3\t60\t30",
            b(T2_HULL)
        ));
        let recipes = industry::Table::parse(&rows, &eve_server::econ_prices::Table::default())
            .expect("the rows parse");
        (c, recipes)
    }

    #[test]
    fn t3_inputs_are_classed_and_a_world_without_loot_cannot_feed_them() {
        use crate::hulls::{Input, input_of};
        let (c, recipes) = t3_world();
        assert_eq!(input_of(&c, HYBRID), Input::Built);
        assert_eq!(input_of(&c, SALVAGE), Input::Loot);
        let hull = recipes.find(T3_HULL + 1_000).expect("a hull recipe");
        assert!(Sourcing::new(&c, &recipes).blocked(hull).is_empty());
        let no_loot = Made {
            loot: false,
            ..MADE_TODAY
        };
        assert_eq!(
            Sourcing::with(&c, &recipes, no_loot).blocked(hull),
            vec![SALVAGE],
            "no explorer, no salvage, no component, no hull"
        );
    }

    /// The ladder: T1 rungs, then capital rungs, then the Tech 3 rung (the
    /// hybrid components' originals); the hull, subsystem and destroyer
    /// blueprints are on no rung, because no corporation sells them. Built
    /// copies: Tech II first, Tech III after, whatever the ids.
    #[test]
    fn t3_rungs_come_after_the_battlecruiser_rung() {
        let (c, recipes) = t3_world();
        let t3 = crate::hulls::t3_rungs(&c, &recipes);
        assert_eq!(t3, vec![vec![HYBRID + 1_000]]);
        let capital = crate::hulls::capital_rungs(&c, &recipes);
        let mut all = capital.clone();
        all.extend(t3.clone());
        let ladder = ladder_with(&all);
        assert_eq!(ladder.len(), BLUEPRINT_RUNGS.len() + all.len());
        assert_eq!(
            ladder.last().copied(),
            Some(&t3[0][..]),
            "after the T1 ladder and the capital rungs"
        );
        let flat: Vec<i64> = ladder.concat();
        for bp in [T3_HULL, T3_SUB, T2_HULL] {
            assert!(!flat.contains(&(bp + 1_000)), "{bp} is no rung");
        }

        let at = |item, bp: i64| bp_row(item, bp + 1_000, HERE, 1, None);
        let copies = [at(1, T3_HULL), at(2, T3_SUB), at(3, T2_HULL)];
        let none = std::collections::BTreeSet::new();
        let order: Vec<i64> = t2_candidates(&copies, HERE, &recipes, &none)
            .iter()
            .map(|b| b.item_id)
            .collect();
        assert_eq!(order, [3, 1, 2], "the T2 copy, then the T3 ones by id");
        assert_eq!(
            t3_types(&recipes),
            [T3_HULL + 1_000, T3_SUB + 1_000].into_iter().collect()
        );
    }
}
