//! **Demand with a size, through the market** (FE-DEMAND, R-FLUIDECON §2-3.4):
//! every need a bot has -- a doctrine's hulls and parts, a lost ship, the
//! rounds a fight burns, a tier up, the fuel a hub eats -- becomes **a
//! standing bid with units** through one channel, [`post`]; and the bids on a
//! region's books are read back as a **demand map** ([`map`], [`DemandMap`])
//! that builders and gatherers act on.
//!
//! # The channel is the market, nothing else
//!
//! No bot hands a need to another. A bot that wants twelve plates rests a bid
//! for twelve; a builder reads the books (`GetOrders`, the same call a human's
//! client makes) and sees twelve unmet. [`DemandMap`] is built from
//! [`Spot`]s, the player half of a book, and from nothing else, so what a
//! builder "knows" is what a human reading the market window would know.
//! Corp orders are not supported by the server (`isCorp` is discarded), so a
//! need is always a *member's* personal bid.
//!
//! # Posting ([`post`])
//!
//! One type, one station, `units` wanted. The pilot's resting bids for the
//! type are summed; a shortfall is topped up with a new order of the
//! difference (an order cannot be resized in place, `ModifyCharOrder` moves
//! the price only); the price is the consumer's reservation, `RESERVATION x V`
//! ([`crate::pricing::Prices::reservation`]), never above it, so a resting
//! bid is **repriced toward the cap** each visit but a *rise* is held to
//! [`crate::pricing::FE_MAX_STEP`] a repricing ([`crate::pricing::capped_rise`])
//! and a fall is not. The size is clamped to [`FE_BID_MAX_UNITS`] and to what
//! [`crate::hull_choice::SPEND_SHARE`] of the wallet pays for. Several needs
//! for one type (a loss and a doctrine) are summed by the caller into one
//! `units`.
//!
//! # What each need becomes ([`Need`])
//!
//! | need | units | where it is called from |
//! |---|---|---|
//! | doctrine hull / fit modules | missing count | [`crate::hull_choice::want_parts`], `obtain_from` |
//! | a loss | hull 1 + the fit's modules + [`FE_AMMO_DAYS`] of rounds ([`replenish`]) | [`crate::hull_choice::replenish`] |
//! | a spare hull when rich | 1 ([`spare_hull_wanted`]) | [`crate::hull_choice::replenish`] |
//! | a tier up | 1 hull of the next class | [`crate::hull_choice::upgrade_bid`] |
//! | fuel | [`fuel_units`] | a sov bot's turn |
//!
//! # Reading it ([`DemandMap`], the seam for FE-BUILD)
//!
//! `demand::map(conn, prices, &types) -> DemandMap` reads each type's book
//! once (cached in [`Prices`]) and keeps, per type, the player bid and ask
//! units, the best bid and the pressure. A builder asks
//! [`DemandMap::unmet`] (bids less asks, units), [`DemandMap::pull`]
//! (`0..=1`, how hard the bids outweigh the asks) and [`DemandMap::weight`]
//! (`V x (1 + FE_PULL_K x pull)`, the value a producer or gatherer ranks by).
//! `builder.rs` today builds a `BTreeMap<blueprint, unmet>` for
//! `promote_demanded` from `Conn::market_orders`; FE-BUILD swaps that for
//! `DemandMap::unmet` of the blueprint's product and may use `weight` in its
//! profit/hour.
//!
//! # Gatherers ([`ore_values`], [`by_value`])
//!
//! A miner's rock is chosen by what its ore is *worth per m3 refined at
//! today's mineral values*, each mineral's value lifted by the bid pull on
//! it. [`by_value`] drops a free ordinary rock whose ore pays more than
//! [`FE_ORE_EDGE`] less than the best free rock's, as
//! [`crate::moon_pref::scanned`] drops a lesser moon ore; a rock whose ore is
//! unknown stays. [`weigh`] is the same rule for any gatherer's choice among
//! goods (PI, gas, explorer loot).
//!
//! Every constant here is **[A]**, with its falsifier on the constant.

use crate::acquire::Order;
use crate::pricing::{self, Prices, Spot};
use crate::{Conn, Error, Result, Tracked};
use std::collections::BTreeMap;

/// The most units one bid asks **[A]**. Falsified by a doctrine switch whose
/// bids never reach the doctrine's size (too low), or one bidder's order
/// swamping a thin book's mid (too high).
pub const FE_BID_MAX_UNITS: i64 = 200;
/// Days of rounds a combat bot holds and re-buys after a loss **[A]**
/// (R-FLUIDECON §3.4). Falsified by fighters out of ammo mid-fight (too low)
/// or ammo stacks that never run down (too high).
pub const FE_AMMO_DAYS: f64 = 3.0;
/// A bot with this many times a hull's value in its wallet keeps one spare
/// hull **[A]** (§3.4). Falsified by rich bots flying a loss at a time.
pub const FE_SPARE_RICH: f64 = 6.0;
/// A resting bid whose price is within this fraction of the target is left
/// alone **[A]**: a repricing costs a broker fee and a delay.
pub const FE_REPRICE_EPS: f64 = 0.02;
/// How far full bid pressure lifts a gatherer's weight **[A]**.
/// Falsified by every miner on the pulled ore while the rest sits full
/// (too high), or the pull never moving a target (too low).
pub const FE_PULL_K: f64 = 0.5;
/// How much less than the best a free rock's ore may pay and still be
/// mined **[A]** (§7 FE-CAREER). Falsified by miners on poor ore while a
/// near rock of richer ore sits free (too high), or all on one rock (too low).
pub const FE_ORE_EDGE: f64 = 0.15;
/// Days of fuel a sov hub's bot keeps stocked **[A]**. Falsified by a hub
/// that runs dry (too low) or fuel stacks piling up (too high).
pub const FE_FUEL_DAYS: f64 = 3.0;

/// What a need is for. The kind decides nothing about the bid; it is the
/// `[demand]` log's label and the test's handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Hull,
    Module,
    Ammo,
    Loss,
    Upgrade,
    Spare,
    Fuel,
    Sov,
}

/// One need: `units` of `type_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Need {
    pub type_id: i64,
    pub units: i64,
    pub kind: Kind,
}

// ---- posting ------------------------------------------------------------------

/// One of the pilot's resting bids for the type, as [`plan_post`] reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rest {
    pub order_id: i64,
    pub price: f64,
    pub units: i64,
}

/// What [`post`] decides to do (pure: [`plan_post`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PostPlan {
    /// Units of a new order, 0 for none.
    pub top_up: i64,
    /// The price of the new order, and the target the old ones move toward.
    pub price: f64,
    /// `(order id, new price)` for each resting bid worth moving.
    pub reprice: Vec<(i64, f64)>,
}

/// **The size of a bid**: `want` clamped to `1..=`[`FE_BID_MAX_UNITS`], then
/// cut to what `budget` ISK pays for at `price`. 0 when not even one unit is
/// affordable or `want` is not positive.
pub fn size(want: i64, price: f64, budget: f64) -> i64 {
    if want <= 0 || price.is_nan() || price <= 0.0 || !price.is_finite() {
        return 0;
    }
    let affordable = (budget / price).floor();
    if affordable.is_nan() || affordable < 1.0 {
        return 0;
    }
    want.clamp(1, FE_BID_MAX_UNITS).min(affordable as i64)
}

/// **What to do about one type** (pure): bid for `want` units at `target`
/// given the pilot's `resting` bids. The shortfall is a new order at
/// `target`; each resting bid more than [`FE_REPRICE_EPS`] from `target` is
/// moved toward it, a rise held to [`pricing::FE_MAX_STEP`], and **never
/// above `target`**.
pub fn plan_post(resting: &[Rest], want: i64, target: f64) -> PostPlan {
    let have: i64 = resting.iter().map(|r| r.units.max(0)).sum();
    let reprice = resting
        .iter()
        .filter_map(|r| {
            let next = pricing::capped_rise(Some(r.price), target).min(target);
            ((next - r.price).abs() > r.price * FE_REPRICE_EPS).then_some((r.order_id, next))
        })
        .collect();
    PostPlan {
        top_up: (want - have).max(0),
        price: target,
        reprice,
    }
}

/// What a [`post`] did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Posted {
    /// Units of a new order placed.
    pub placed: i64,
    /// Units the pilot's bids for the type rest for now (before the new one).
    pub resting: i64,
    /// Resting bids moved.
    pub repriced: usize,
    /// The target price, 0 when no bid was possible.
    pub price: f64,
}

fn soft<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(_) => Ok(None),
    }
}

/// **Post a need** (docked): the channel every demand goes through. Rests
/// bids for `units` of `type_id` at this station, sized and priced as in the
/// module docs. A type with no price, an unaffordable one, or an undocked
/// pilot posts nothing (`Posted::default()`).
pub fn post(
    conn: &mut Conn,
    prices: &mut Prices,
    type_id: i64,
    units: i64,
    wallet: f64,
) -> Result<Posted> {
    post_as(conn, prices, type_id, units, wallet, false)
}

/// [`post`] for a corporation's wallet when `use_corp` (CORP-BUY): the bids
/// are placed with `useCorp`, and the resting ones it reprices and counts are
/// the corporation's, read from `GetCorporationOrders`. `wallet` is then the
/// division's budget. Only a member with the accountant and trader roles
/// (`doctrine_stock`'s procurement member) may.
pub fn post_as(
    conn: &mut Conn,
    prices: &mut Prices,
    type_id: i64,
    units: i64,
    wallet: f64,
    use_corp: bool,
) -> Result<Posted> {
    let Some(station) = conn.session().station_id() else {
        return Ok(Posted::default());
    };
    let Some(target) = soft(prices.reservation(conn, type_id))?.flatten() else {
        return Ok(Posted::default());
    };
    let want = size(units, target, wallet * crate::hull_choice::SPEND_SHARE);
    if want == 0 {
        return Ok(Posted::default());
    }
    let open = if use_corp {
        conn.corp_open_orders()?
    } else {
        conn.my_open_orders()?
    };
    let mine: Vec<&crate::verbs::OwnOrder> = open
        .iter()
        .filter(|o| o.order.bid && o.order.type_id == type_id && o.order.station_id == station)
        .collect();
    let rests: Vec<Rest> = mine
        .iter()
        .map(|o| Rest {
            order_id: o.order.order_id,
            price: o.order.price,
            units: o.order.vol_remaining,
        })
        .collect();
    let plan = plan_post(&rests, want, target);
    let mut done = Posted {
        resting: rests.iter().map(|r| r.units).sum(),
        price: target,
        ..Posted::default()
    };
    for (id, price) in &plan.reprice {
        if let Some(own) = mine.iter().find(|o| o.order.order_id == *id)
            && soft(conn.modify_order(own, *price))?.is_some()
        {
            done.repriced += 1;
        }
    }
    if plan.top_up > 0
        && soft(conn.buy_with(
            use_corp,
            station,
            type_id,
            plan.price,
            plan.top_up,
            crate::hull_choice::BID_DAYS,
            crate::builder::BROKER_FEE,
        ))?
        .is_some()
    {
        done.placed = plan.top_up;
    }
    Ok(done)
}

// ---- what a need becomes ------------------------------------------------------

/// **The replacement of a lost ship** (pure): a bid for the hull (when none
/// is held), for each module of `order` the pilot lacks and for the rounds,
/// at least [`FE_AMMO_DAYS`] of `rounds_per_day` (the order's own count when
/// larger). `held` is what the hangar holds of a type.
pub fn replenish(
    order: &Order,
    held: &dyn Fn(i64) -> i64,
    rounds_per_day: &dyn Fn(i64) -> f64,
) -> Vec<Need> {
    let mut out = Vec::new();
    if let Some(h) = order.hull
        && held(h) < 1
    {
        out.push(Need {
            type_id: h,
            units: 1,
            kind: Kind::Loss,
        });
    }
    for &(t, n) in &order.modules {
        let short = n - held(t);
        if short > 0 {
            out.push(Need {
                type_id: t,
                units: short,
                kind: Kind::Module,
            });
        }
    }
    for &(t, n) in &order.charges {
        let days = (FE_AMMO_DAYS * rounds_per_day(t)).ceil() as i64;
        let short = n.max(days) - held(t);
        if short > 0 {
            out.push(Need {
                type_id: t,
                units: short,
                kind: Kind::Ammo,
            });
        }
    }
    out
}

/// **A spare hull** (pure): a bot with [`FE_SPARE_RICH`] hull-values in the
/// wallet and no spare in the hangar wants one more.
pub fn spare_hull_wanted(wallet: f64, hull_value: f64, spares_held: i64) -> bool {
    hull_value > 0.0 && spares_held < 1 && wallet >= FE_SPARE_RICH * hull_value
}

/// **Fuel units to bid for** (pure): [`FE_FUEL_DAYS`] of `per_hub_day` for
/// `hubs` hubs, less what is held.
pub fn fuel_units(hubs: i64, per_hub_day: i64, held: i64) -> i64 {
    ((FE_FUEL_DAYS * (hubs.max(0) * per_hub_day.max(0)) as f64).ceil() as i64 - held).max(0)
}

/// **Post every need** of `needs` (summed per type), in type order. The
/// types posted for, with the units placed.
pub fn post_all(
    conn: &mut Conn,
    prices: &mut Prices,
    needs: &[Need],
    wallet: f64,
) -> Result<Vec<(i64, i64)>> {
    let mut sum: BTreeMap<i64, i64> = BTreeMap::new();
    for n in needs {
        *sum.entry(n.type_id).or_default() += n.units.max(0);
    }
    let mut placed = Vec::new();
    for (t, units) in sum {
        let p = post(conn, prices, t, units, wallet)?;
        if p.placed > 0 {
            placed.push((t, p.placed));
        }
    }
    Ok(placed)
}

// ---- reading it back: the demand map ------------------------------------------

/// One type's demand in the region, from its player book.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Demand {
    pub bid_units: i64,
    pub ask_units: i64,
    pub best_bid: Option<f64>,
    /// `-1..=1`, [`Spot::pressure`].
    pub pressure: f64,
}

/// **The demand map of a region**: type to [`Demand`]. The seam builders and
/// gatherers read; built only from books.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DemandMap {
    by: BTreeMap<i64, Demand>,
}

impl DemandMap {
    /// From `(type, spot, daily volume)` rows (pure).
    pub fn from_spots(rows: impl IntoIterator<Item = (i64, Spot, f64)>) -> DemandMap {
        DemandMap {
            by: rows
                .into_iter()
                .map(|(t, s, volume)| {
                    (
                        t,
                        Demand {
                            bid_units: s.bid_units,
                            ask_units: s.ask_units,
                            best_bid: s.best_bid,
                            pressure: s.pressure(volume),
                        },
                    )
                })
                .collect(),
        }
    }

    /// From `(type, bid units, ask units)` when only the depth is known (the
    /// career signals' quotes), at the volume floor.
    pub fn from_units(rows: impl IntoIterator<Item = (i64, i64, i64)>) -> DemandMap {
        DemandMap::from_spots(rows.into_iter().map(|(t, b, a)| {
            (
                t,
                Spot {
                    bid_units: b,
                    ask_units: a,
                    ..Spot::default()
                },
                0.0,
            )
        }))
    }

    pub fn get(&self, type_id: i64) -> Option<&Demand> {
        self.by.get(&type_id)
    }

    /// Units wanted beyond what is offered: bids less asks, never below 0.
    pub fn unmet(&self, type_id: i64) -> i64 {
        self.get(type_id)
            .map_or(0, |d| (d.bid_units - d.ask_units).max(0))
    }

    /// How hard the bids outweigh the asks, `0..=1`.
    pub fn pull(&self, type_id: i64) -> f64 {
        self.get(type_id).map_or(0.0, |d| d.pressure.max(0.0))
    }

    /// `value x (1 + FE_PULL_K x pull)`: what a producer or gatherer ranks by.
    pub fn weight(&self, type_id: i64, value: f64) -> f64 {
        weigh(value, self.pull(type_id))
    }

    /// The types with any unmet demand, most pulled first (ties by type id).
    pub fn ranked(&self) -> Vec<(i64, f64)> {
        let mut v: Vec<(i64, f64)> = self
            .by
            .keys()
            .map(|&t| (t, self.pull(t)))
            .filter(|&(_, p)| p > 0.0)
            .collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }

    pub fn is_empty(&self) -> bool {
        self.by.is_empty()
    }
}

/// `value x (1 + FE_PULL_K x pull)` with `pull` kept in `0..=1`.
pub fn weigh(value: f64, pull: f64) -> f64 {
    value * (1.0 + FE_PULL_K * pull.clamp(0.0, 1.0))
}

/// **Read the demand map** for `types` (docked): each book once through
/// `prices`; a type with no readable book is left out.
pub fn map(conn: &mut Conn, prices: &mut Prices, types: &[i64]) -> Result<DemandMap> {
    let mut rows = Vec::new();
    for &t in types {
        if let Some(spot) = prices.spot(conn, t)? {
            let volume = prices.daily_volume(conn, t)?;
            rows.push((t, spot, volume));
        }
    }
    Ok(DemandMap::from_spots(rows))
}

// ---- gatherers ----------------------------------------------------------------

/// **ISK per m3 of an ore** (pure): one portion refines to `yields`
/// (`(mineral, quantity)`), each mineral worth `value` (its weight, if the
/// caller lifts it by the pull), over `portion` units of `volume` m3 each.
/// `None` when the ore yields nothing priced.
pub fn ore_isk_per_m3(
    yields: &[(i64, i64)],
    portion: i64,
    volume: f64,
    value: &dyn Fn(i64) -> Option<f64>,
) -> Option<f64> {
    if volume.is_nan() || volume <= 0.0 {
        return None;
    }
    let total: f64 = yields
        .iter()
        .filter_map(|&(m, q)| value(m).map(|v| v * q as f64))
        .sum();
    (total > 0.0).then(|| total / portion.max(1) as f64 / volume)
}

/// **The ISK/m3 of each ore in `ores`** from the bundle's refine yields and
/// volumes, minerals valued at `value` lifted by `demand`'s pull. Ores the
/// bundle cannot price are left out.
pub fn ore_values(
    content: &eve_server::content::Content,
    ores: impl IntoIterator<Item = i64>,
    value: &BTreeMap<i64, f64>,
    demand: &DemandMap,
) -> BTreeMap<i64, f64> {
    let (Some(refine), Some(types)) = (
        content.try_get::<eve_server::reprocess::Table>(),
        content.try_get::<eve_server::typedata::Installed>(),
    ) else {
        return BTreeMap::new();
    };
    let weighted = |m: i64| value.get(&m).map(|&v| demand.weight(m, v));
    ores.into_iter()
        .filter_map(|ore| {
            let volume = types.table().get(ore)?.volume;
            let v = ore_isk_per_m3(
                refine.materials(ore),
                refine.portion_size(ore),
                volume,
                &weighted,
            )?;
            Some((ore, v))
        })
        .collect()
}

/// [`ore_values`] over every reprocessable asteroid type of the bundle.
pub fn ore_values_all(
    content: &eve_server::content::Content,
    value: &BTreeMap<i64, f64>,
    demand: &DemandMap,
) -> BTreeMap<i64, f64> {
    let (Some(refine), Some(types)) = (
        content.try_get::<eve_server::reprocess::Table>(),
        content.try_get::<eve_server::typedata::Installed>(),
    ) else {
        return BTreeMap::new();
    };
    let ores: Vec<i64> = refine
        .reprocessable_types()
        .filter(|&t| {
            types
                .table()
                .get(t)
                .is_some_and(|r| r.category_id == crate::brain::CATEGORY_ASTEROID)
        })
        .collect();
    ore_values(content, ores, value, demand)
}

/// **The rocks to leave alone** (pure): from `(rock id, ISK/m3 if known)`,
/// the ids whose ore pays more than [`FE_ORE_EDGE`] less than the best. An
/// unknown ore is never dropped, and with nothing known nothing is.
pub fn below_the_edge(free: &[(i64, Option<f64>)]) -> Vec<i64> {
    let best = free
        .iter()
        .filter_map(|&(_, v)| v)
        .fold(f64::NEG_INFINITY, f64::max);
    if !best.is_finite() {
        return Vec::new();
    }
    free.iter()
        .filter(|&&(_, v)| v.is_some_and(|v| v < best * (1.0 - FE_ORE_EDGE)))
        .map(|&(id, _)| id)
        .collect()
}

/// **The rocks a miner is shown**, by value: the free ordinary rocks of an
/// ore that pays [`FE_ORE_EDGE`] less than the best free one are taken out.
/// Moon rocks (`crate::moon_pref`'s choice) and rocks under a laser stay.
pub fn by_value<'a>(
    rocks: Vec<&'a Tracked>,
    taken: &BTreeMap<i64, i64>,
    values: &BTreeMap<i64, f64>,
) -> Vec<&'a Tracked> {
    if values.is_empty() {
        return rocks;
    }
    let free: Vec<(i64, Option<f64>)> = rocks
        .iter()
        .filter(|r| {
            !eve_server::moon_belt::is_moon_id(r.ball_id) && !taken.contains_key(&r.ball_id)
        })
        .map(|r| (r.ball_id, r.type_id().and_then(|t| values.get(&t).copied())))
        .collect();
    let drop = below_the_edge(&free);
    rocks
        .into_iter()
        .filter(|r| !drop.contains(&r.ball_id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids.
    const HULL: i64 = 990_100_001;
    const GUN: i64 = 990_100_002;
    const ROUND: i64 = 990_100_003;
    const MIN_A: i64 = 990_300_001;
    const MIN_B: i64 = 990_300_002;

    fn order() -> Order {
        Order {
            hull: Some(HULL),
            modules: vec![(GUN, 4)],
            charges: vec![(ROUND, 100)],
            skills: vec![],
        }
    }

    #[test]
    fn loss_posts_bids_for_hull_modules_and_ammo() {
        let held = |_t: i64| 0;
        let per_day = |t: i64| if t == ROUND { 80.0 } else { 0.0 };
        let needs = replenish(&order(), &held, &per_day);
        let get = |t| needs.iter().find(|n| n.type_id == t).copied();
        assert_eq!(get(HULL).map(|n| (n.units, n.kind)), Some((1, Kind::Loss)));
        assert_eq!(get(GUN).map(|n| (n.units, n.kind)), Some((4, Kind::Module)));
        let days = (FE_AMMO_DAYS * 80.0).ceil() as i64;
        assert_eq!(
            get(ROUND).map(|n| (n.units, n.kind)),
            Some((days.max(100), Kind::Ammo)),
            "the larger of the order's count and the days of fire"
        );
        // What is held is not bid for again.
        let some = |t: i64| match t {
            HULL => 1,
            GUN => 3,
            _ => 0,
        };
        let needs = replenish(&order(), &some, &per_day);
        assert!(needs.iter().all(|n| n.type_id != HULL), "hull held");
        assert_eq!(
            needs.iter().find(|n| n.type_id == GUN).map(|n| n.units),
            Some(1)
        );
    }

    #[test]
    fn bid_depth_matches_missing_units() {
        // 12 wanted, 5 resting: a top-up of 7 and nothing more.
        let rest = [Rest {
            order_id: 1,
            price: 100.0,
            units: 5,
        }];
        let plan = plan_post(&rest, 12, 100.0);
        assert_eq!(plan.top_up, 7);
        assert!(plan.reprice.is_empty());
        // Enough resting: no new order.
        assert_eq!(plan_post(&rest, 5, 100.0).top_up, 0);
        assert_eq!(plan_post(&rest, 3, 100.0).top_up, 0);
        // Sized: clamped to the cap, then to what the budget pays.
        assert_eq!(size(10_000, 1.0, 1e12), FE_BID_MAX_UNITS);
        assert_eq!(size(12, 100.0, 450.0), 4);
        assert_eq!(size(12, 100.0, 99.0), 0, "not even one");
        assert_eq!(size(0, 100.0, 1e9), 0);
    }

    #[test]
    fn bid_repriced_toward_cap_not_above() {
        let at = |price: f64| {
            [Rest {
                order_id: 7,
                price,
                units: 1,
            }]
        };
        // Far below the target: a rise, held to FE_MAX_STEP, never over target.
        let plan = plan_post(&at(100.0), 1, 1_000.0);
        let (_, next) = plan.reprice[0];
        assert!(next > 100.0 && next <= 100.0 * (1.0 + pricing::FE_MAX_STEP) + 0.01);
        assert!(next <= 1_000.0);
        // Above the target (the market fell): down to it in one move.
        let plan = plan_post(&at(500.0), 1, 300.0);
        assert_eq!(plan.reprice, vec![(7, 300.0)]);
        // Within the epsilon: left alone.
        assert!(
            plan_post(&at(100.0), 1, 100.0 * (1.0 + FE_REPRICE_EPS / 2.0))
                .reprice
                .is_empty()
        );
        // Close below the target, a step that overshoots is cut to target.
        let plan = plan_post(&at(100.0), 1, 110.0);
        assert!(plan.reprice.iter().all(|&(_, p)| p <= 110.0));
    }

    #[test]
    fn spare_hull_when_rich() {
        assert!(spare_hull_wanted(FE_SPARE_RICH * 100.0, 100.0, 0));
        assert!(!spare_hull_wanted(FE_SPARE_RICH * 100.0 - 1.0, 100.0, 0));
        assert!(!spare_hull_wanted(1e12, 100.0, 1), "one spare is enough");
        assert!(!spare_hull_wanted(1e12, 0.0, 0), "no value, no spare");
    }

    #[test]
    fn fuel_is_days_of_burn_less_stock() {
        assert_eq!(fuel_units(2, 100, 0), (FE_FUEL_DAYS * 200.0) as i64);
        assert_eq!(fuel_units(2, 100, 10_000), 0);
        assert_eq!(fuel_units(0, 100, 0), 0);
    }

    fn spot(bid: i64, ask: i64) -> Spot {
        Spot {
            best_bid: Some(10.0),
            best_ask: Some(12.0),
            bid_units: bid,
            ask_units: ask,
        }
    }

    #[test]
    fn the_map_reads_unmet_and_pull_from_the_books() {
        let m = DemandMap::from_spots([
            (HULL, spot(40, 10), 10.0),
            (GUN, spot(0, 30), 10.0),
            (ROUND, spot(5, 5), 10.0),
        ]);
        assert_eq!(m.unmet(HULL), 30);
        assert_eq!(m.unmet(GUN), 0);
        assert_eq!(m.unmet(ROUND), 0);
        assert_eq!(m.unmet(MIN_A), 0, "unknown: none");
        assert!(m.pull(HULL) > 0.5);
        assert_eq!(m.pull(GUN), 0.0, "an ask glut pulls nothing");
        assert_eq!(m.ranked(), vec![(HULL, m.pull(HULL))]);
        assert!(m.weight(HULL, 100.0) > 100.0);
        assert!(m.weight(HULL, 100.0) <= 100.0 * (1.0 + FE_PULL_K));
        assert_eq!(m.weight(GUN, 100.0), 100.0);
    }

    #[test]
    fn miner_prefers_ore_with_higher_isk_per_m3() {
        let value = |m: i64| match m {
            MIN_A => Some(10.0),
            MIN_B => Some(2.0),
            _ => None,
        };
        // Same volume: rich ore yields the dear mineral.
        let rich = ore_isk_per_m3(&[(MIN_A, 100)], 100, 1.0, &value).unwrap();
        let poor = ore_isk_per_m3(&[(MIN_B, 100)], 100, 1.0, &value).unwrap();
        assert!(rich > poor);
        // A price move flips the order.
        let flipped = |m: i64| match m {
            MIN_A => Some(1.0),
            MIN_B => Some(5.0),
            _ => None,
        };
        let rich2 = ore_isk_per_m3(&[(MIN_A, 100)], 100, 1.0, &flipped).unwrap();
        let poor2 = ore_isk_per_m3(&[(MIN_B, 100)], 100, 1.0, &flipped).unwrap();
        assert!(poor2 > rich2);
        // Unpriced yields: no value, and a bad volume is none.
        assert_eq!(ore_isk_per_m3(&[(999, 5)], 1, 1.0, &value), None);
        assert_eq!(ore_isk_per_m3(&[(MIN_A, 5)], 1, 0.0, &value), None);
        // The edge: only the poor one is dropped, a near one stays.
        let free = [
            (1, Some(rich)),
            (2, Some(poor)),
            (3, Some(rich * (1.0 - FE_ORE_EDGE / 2.0))),
            (4, None),
        ];
        assert_eq!(below_the_edge(&free), vec![2]);
        assert!(below_the_edge(&[(1, None), (2, None)]).is_empty());
        assert!(below_the_edge(&[]).is_empty());
    }

    #[test]
    fn pull_on_a_mineral_lifts_its_ore() {
        let value = BTreeMap::from([(MIN_A, 10.0), (MIN_B, 10.0)]);
        let flat = DemandMap::default();
        let pulled = DemandMap::from_units([(MIN_B, 1_000, 0)]);
        let isk = |d: &DemandMap, m: i64| {
            ore_isk_per_m3(&[(m, 100)], 100, 1.0, &|x| {
                value.get(&x).map(|&v| d.weight(x, v))
            })
            .unwrap()
        };
        assert_eq!(isk(&flat, MIN_A), isk(&flat, MIN_B));
        assert!(isk(&pulled, MIN_B) > isk(&pulled, MIN_A));
    }
}
