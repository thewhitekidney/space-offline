//! **The tape** (lane FE-MEASURE, R-FLUIDECON §4 and §5 G2): what prices did,
//! read off the fills, the open book, the items and the industry jobs, and
//! the detector for manipulation loops. A pure read of the world, like the
//! rest of [`super`]; nothing here writes a delta or changes a price.
//!
//! - **Price rows** per watched type and region: the window's VWAP, `P_ref`,
//!   the trading index (`mktidx`, which the wash filter protects) and the
//!   spot (best ask and bid of the live book, NPC orders included).
//! - **Baskets** ([`PRICE_BASKETS`]): a value-weighted index of history
//!   (`Σ vwap·units / Σ P_ref·units`), of the trading index and of spot, with
//!   the book's depth, the bots' stock, the builders' share of job-hours.
//! - **Churn** (destroyed over minted, per goods class) and the **level**
//!   (the closed-loop price index with the bots' and humans' money).
//! - **Audit** ([`Audit`]): the top bot's ISK/h against the median, buyer and
//!   seller pairs with too many fills of one type, round trips inside
//!   [`ROUND_TRIP_HOURS`], types whose index is pinned to the band, and the
//!   wash fills the index ignores.
//!
//! **Basket membership.** A type is in a basket by its group
//! ([`Basket::groups`], ids the repository already carries), or by its
//! category and a word of its name ([`Basket::words`], game vocabulary, not a
//! table). Hulls are ranked by `rigSize` ([`super::tier`]). **[A]** The
//! word lists are a guess at the retail names; falsified by `/econ
//! price-tape` listing a module in the wrong basket or `other`.

use super::{Report, tier};
use crate::damage::BlueTime;
use crate::econ_exchange::{self as exchange, Fill, Side};
use crate::econ_ledger::{self as ledger, Goods};
use crate::econ_prices as prices;
use crate::service::Registry;
use std::collections::{BTreeMap, BTreeSet};

/// How many types the tape watches by day value (`FE_WATCH_N`). **[A]**:
/// falsified by a doctrine soak that names a type the tape never shows (then
/// use [`WATCH_ENV`]).
pub const WATCH_N: usize = 12;

/// A comma-separated list of type ids the tape always watches.
pub const WATCH_ENV: &str = "EVE_ECON_WATCH";

/// A bot earning more than this many times the median is an outlier
/// (`FE_OUTLIER`). **[A]**: falsified by a soak whose careers differ by more
/// than this honestly (the table of `econ-report.py` then shows it).
pub const OUTLIER: f64 = 8.0;

/// More fills than this of one type between one buyer and seller in the
/// window is a pair worth a look (`FE_PAIR_MAX`). **[A]**: falsified by a
/// supply route that honestly fills more often.
pub const PAIR_MAX: usize = 12;

/// Opposite fills of one type between one pair within this many game hours
/// are a round trip. **[A]**
pub const ROUND_TRIP_HOURS: i64 = 6;

/// A group (ids) or a category with a word of the name.
#[derive(Debug, Clone, Copy)]
pub struct Basket {
    pub name: &'static str,
    pub groups: &'static [i64],
    pub categories: &'static [i64],
    /// Lower-case fragments; an empty list takes the whole category.
    pub words: &'static [&'static str],
}

const CATEGORY_MODULE: i64 = 7;
const CATEGORY_CHARGE: i64 = 8;
const CATEGORY_DRONE: i64 = 18;

/// The named baskets of the tape (R-FLUIDECON §4). The hull baskets
/// (`hull_s`, `hull_m`, `hull_l`, `hull_c`) are by `rigSize`, not listed.
pub const PRICE_BASKETS: [Basket; 14] = [
    Basket {
        name: "mineral",
        groups: &[18],
        categories: &[],
        words: &[],
    },
    Basket {
        name: "ice",
        groups: &[crate::item::GROUP_ICE_PRODUCT],
        categories: &[],
        words: &[],
    },
    Basket {
        name: "moon",
        groups: &[
            427,
            crate::moon::GROUP_UBIQUITOUS,
            crate::moon::GROUP_COMMON,
            crate::moon::GROUP_UNCOMMON,
            crate::moon::GROUP_RARE,
            crate::moon::GROUP_EXCEPTIONAL,
        ],
        categories: &[],
        words: &[],
    },
    Basket {
        name: "pi",
        groups: &[1032, 1033, 1034, 1035, 1040, 1041, 1042],
        categories: &[],
        words: &[],
    },
    Basket {
        name: "fuel",
        groups: &[crate::sov_hub::GROUP_FUEL_BLOCK],
        categories: &[],
        words: &[],
    },
    Basket {
        name: "loot",
        groups: &[crate::salvage::GROUP_SALVAGE],
        categories: &[],
        words: &[],
    },
    Basket {
        name: "ammo",
        groups: &[],
        categories: &[CATEGORY_CHARGE],
        words: &[],
    },
    Basket {
        name: "armour",
        groups: &[],
        categories: &[CATEGORY_MODULE],
        words: &["armor repair", "armor hardener", "plate"],
    },
    Basket {
        name: "shield",
        groups: &[],
        categories: &[CATEGORY_MODULE],
        words: &[
            "shield extender",
            "shield booster",
            "shield hardener",
            "shield amplifier",
        ],
    },
    Basket {
        name: "weapon_projectile",
        groups: &[],
        categories: &[CATEGORY_MODULE],
        words: &["autocannon", "artillery"],
    },
    Basket {
        name: "weapon_hybrid",
        groups: &[],
        categories: &[CATEGORY_MODULE],
        words: &["blaster", "railgun"],
    },
    Basket {
        name: "weapon_laser",
        groups: &[],
        categories: &[CATEGORY_MODULE],
        words: &["pulse laser", "beam laser"],
    },
    Basket {
        name: "weapon_missile",
        groups: &[],
        categories: &[CATEGORY_MODULE],
        words: &["missile launcher"],
    },
    Basket {
        name: "drone",
        groups: &[],
        categories: &[CATEGORY_DRONE],
        words: &[],
    },
];

/// Does a type of `(category, group, name)` belong to `b`?
pub fn in_basket(b: &Basket, category: i64, group: i64, name: &str) -> bool {
    if b.groups.contains(&group) {
        return true;
    }
    if !b.categories.contains(&category) {
        return false;
    }
    let lower = name.to_lowercase();
    b.words.is_empty() || b.words.iter().any(|w| lower.contains(w))
}

/// The basket of `type_id` among `defs`, or the hull basket of a ship by
/// `rigSize`, or `None`. The first basket in `defs` that holds it wins.
pub fn basket_of(reg: &Registry, type_id: i64, defs: &[Basket]) -> Option<String> {
    let row = reg.content().get::<prices::Table>().prices.get(&type_id)?;
    let name = crate::item::find_in(reg.content(), type_id).map_or("", |t| t.name);
    if let Some(b) = defs
        .iter()
        .find(|b| in_basket(b, row.category_id, row.group_id, name))
    {
        return Some(b.name.to_owned());
    }
    if row.category_id == crate::model::CATEGORY_SHIP {
        return match tier(reg, type_id) {
            "S" => Some("hull_s".to_owned()),
            "M" => Some("hull_m".to_owned()),
            "L" => Some("hull_l".to_owned()),
            "C" => Some("hull_c".to_owned()),
            _ => None,
        };
    }
    None
}

/// The scope key of "all regions" in the maps below.
pub const ALL: i64 = 0;

/// One type in one region over the window.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceRow {
    pub region: i64,
    pub type_id: i64,
    pub basket: String,
    pub fills: usize,
    pub units: i64,
    /// Volume-weighted mean of the window's fills; `None` with no fill.
    pub vwap: Option<f64>,
    pub p_ref: f64,
    /// The trading index (`mktidx`), which the wash filter protects.
    pub index: f64,
    pub best_ask: Option<f64>,
    pub best_bid: Option<f64>,
}

impl PriceRow {
    /// Mid of the live book, or the one side there is.
    pub fn spot(&self) -> Option<f64> {
        match (self.best_ask, self.best_bid) {
            (Some(a), Some(b)) => Some((a + b) / 2.0),
            (a, b) => a.or(b),
        }
    }
}

/// One basket in one scope (`ALL` or a region).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BasketRow {
    /// Types with a price row (traded, or resting in the book).
    pub types: usize,
    pub fills: usize,
    pub units: i64,
    hist: (f64, f64),
    idx: (f64, f64),
    spot: (f64, f64),
    /// Player and bot orders resting, units.
    pub bid_units: i64,
    pub ask_units: i64,
}

impl BasketRow {
    fn ratio((num, den): (f64, f64)) -> Option<f64> {
        (den > 0.0).then(|| num / den)
    }
    /// `Σ vwap·units / Σ P_ref·units`: `None` with no trade (a thin basket
    /// is reported, not failed).
    pub fn history_index(&self) -> Option<f64> {
        Self::ratio(self.hist)
    }
    /// The trading index over the traded types, same weights.
    pub fn trading_index(&self) -> Option<f64> {
        Self::ratio(self.idx)
    }
    /// The live book's spot over `P_ref`, weighted by the window's units
    /// (one unit for a type that did not trade).
    pub fn spot_index(&self) -> Option<f64> {
        Self::ratio(self.spot)
    }
    /// `(bids - asks) / (bids + asks)`, in `[-1, 1]`; `None` with no book.
    pub fn pressure(&self) -> Option<f64> {
        let total = self.bid_units + self.ask_units;
        (total > 0).then(|| (self.bid_units - self.ask_units) as f64 / total as f64)
    }
}

/// A basket's stock: held and for sale, in units, over the window's daily
/// volume.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stock {
    pub held_by_bots: i64,
    pub on_sale_by_bots: i64,
    pub daily_units: f64,
}

impl Stock {
    /// Days of the window's volume the bots hold.
    pub fn days(&self) -> Option<f64> {
        (self.daily_units > 0.0).then(|| self.held_by_bots as f64 / self.daily_units)
    }
}

/// What the detector found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Audit {
    /// `(bot, ISK/h, median ISK/h)` of a bot above [`OUTLIER`] x median.
    pub outlier: Option<(i64, f64, f64)>,
    /// `(buyer, seller, type, fills)` above [`PAIR_MAX`].
    pub pairs: Vec<(i64, i64, i64, usize)>,
    /// `(a, b, type, trips)`: opposite fills of one pair inside the window.
    pub round_trips: Vec<(i64, i64, i64, usize)>,
    /// `(type, "floor" | "ceiling")`: the index sits on its clamp.
    pub pinned: Vec<(i64, &'static str)>,
    /// Fills between members of one corporation or alliance (the index
    /// ignores them).
    pub washes: usize,
}

impl Audit {
    pub fn clean(&self) -> bool {
        self.outlier.is_none()
            && self.pairs.is_empty()
            && self.round_trips.is_empty()
            && self.pinned.is_empty()
            && self.washes == 0
    }
}

/// The tape of one window.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tape {
    pub hours: f64,
    /// Every `(region, type)` that traded or rests in the book.
    pub rows: Vec<PriceRow>,
    /// The watched types, in order.
    pub watched: Vec<i64>,
    pub baskets: BTreeMap<(i64, String), BasketRow>,
    pub stock: BTreeMap<String, Stock>,
    /// Basket -> (jobs started, job-hours).
    pub build: BTreeMap<String, (usize, f64)>,
    /// Goods class -> (minted, destroyed).
    pub churn: BTreeMap<&'static str, (i64, i64)>,
    pub level: Option<f64>,
    pub m_bots: f64,
    pub m_humans: f64,
    pub audit: Audit,
}

/// The ids named by [`WATCH_ENV`].
pub fn watch_env() -> Vec<i64> {
    std::env::var(WATCH_ENV)
        .map(|v| v.split(',').filter_map(|t| t.trim().parse().ok()).collect())
        .unwrap_or_default()
}

fn region_of(reg: &Registry, f: &Fill) -> i64 {
    exchange::place(reg, f.station_id)
        .map(|(_, r)| r)
        .or_else(|| {
            [f.buyer_id, f.seller_id]
                .into_iter()
                .find_map(|c| reg.character(c).map(|c| c.region_id))
        })
        .unwrap_or(ALL)
}

impl Tape {
    /// The tape over the `hours` game hours before `now`, with `goods` for
    /// the churn and `report` for the audit; `extra_watch` are type ids to
    /// show beside the top [`WATCH_N`] by value.
    pub fn of(
        reg: &Registry,
        now: BlueTime,
        hours: f64,
        goods: &Goods,
        report: &Report,
        defs: &[Basket],
        extra_watch: &[i64],
    ) -> Tape {
        let from = now - crate::game_clock::span(reg, (hours * crate::seed::HOUR as f64) as i64);
        let fills: Vec<Fill> = reg
            .market_fills_since(from)
            .into_iter()
            .filter(|f| f.date <= now)
            .collect();
        let orders = reg.market_open_orders();
        let mut t = Tape {
            hours,
            ..Tape::default()
        };

        // (region, type) -> (value, units, fills)
        let mut traded: BTreeMap<(i64, i64), (f64, i64, usize)> = BTreeMap::new();
        let mut by_value: BTreeMap<i64, f64> = BTreeMap::new();
        for f in &fills {
            let e = traded.entry((region_of(reg, f), f.type_id)).or_default();
            e.0 += f.price * f.quantity as f64;
            e.1 += f.quantity;
            e.2 += 1;
            *by_value.entry(f.type_id).or_default() += f.price * f.quantity as f64;
        }
        let mut keys: BTreeSet<(i64, i64)> = traded.keys().copied().collect();
        for o in &orders {
            keys.insert((o.region_id, o.type_id));
        }
        let mut cache: BTreeMap<i64, Option<String>> = BTreeMap::new();
        let mut basket = |reg: &Registry, ty: i64| -> Option<String> {
            cache
                .entry(ty)
                .or_insert_with(|| basket_of(reg, ty, defs))
                .clone()
        };

        for &(region, ty) in &keys {
            let Some(p_ref) = prices::p_ref(reg, ty) else {
                continue;
            };
            let (value, units, n) = traded.get(&(region, ty)).copied().unwrap_or_default();
            let mut ask: Option<f64> = None;
            let mut bid: Option<f64> = None;
            for o in orders
                .iter()
                .filter(|o| o.region_id == region && o.type_id == ty)
            {
                let slot = if o.bid { &mut bid } else { &mut ask };
                *slot = Some(match (*slot, o.bid) {
                    (None, _) => o.price,
                    (Some(b), true) => b.max(o.price),
                    (Some(a), false) => a.min(o.price),
                });
            }
            if region != ALL {
                for o in exchange::orders(reg, region, Some(ty), now) {
                    let slot = if o.side == Side::Bid {
                        &mut bid
                    } else {
                        &mut ask
                    };
                    *slot = Some(match (*slot, o.side) {
                        (None, _) => o.price,
                        (Some(b), Side::Bid) => b.max(o.price),
                        (Some(a), Side::Ask) => a.min(o.price),
                    });
                }
            }
            t.rows.push(PriceRow {
                region,
                type_id: ty,
                basket: basket(reg, ty).unwrap_or_else(|| "other".to_owned()),
                fills: n,
                units,
                vwap: (units > 0).then(|| value / units as f64),
                p_ref,
                index: prices::average(reg, ty).unwrap_or(p_ref),
                best_ask: ask,
                best_bid: bid,
            });
        }

        // Baskets, per region and in total.
        for r in &t.rows {
            for scope in [r.region, ALL] {
                let b = t.baskets.entry((scope, r.basket.clone())).or_default();
                b.types += 1;
                b.fills += r.fills;
                b.units += r.units;
                let w = r.units as f64;
                if let Some(v) = r.vwap {
                    b.hist.0 += v * w;
                    b.hist.1 += r.p_ref * w;
                    b.idx.0 += r.index * w;
                    b.idx.1 += r.p_ref * w;
                }
                if let Some(s) = r.spot() {
                    let w = w.max(1.0);
                    b.spot.0 += s * w;
                    b.spot.1 += r.p_ref * w;
                }
            }
        }
        let mut daily: BTreeMap<String, f64> = BTreeMap::new();
        let days = (hours / 24.0).max(f64::EPSILON);
        for r in &t.rows {
            *daily.entry(r.basket.clone()).or_default() += r.units as f64 / days;
        }
        for o in &orders {
            let Some(name) = basket(reg, o.type_id) else {
                continue;
            };
            for scope in [o.region_id, ALL] {
                let b = t.baskets.entry((scope, name.clone())).or_default();
                if o.bid {
                    b.bid_units += o.remaining;
                } else {
                    b.ask_units += o.remaining;
                }
            }
            if !o.bid && reg.character(o.char_id).is_some_and(|c| c.is_bot) {
                t.stock.entry(name).or_default().on_sale_by_bots += o.remaining;
            }
        }

        // Stock: what the bots hold, except hulls (they sit in them).
        for i in reg.store().items() {
            if i.category_id == crate::model::CATEGORY_SHIP
                || !reg.character(i.owner_id).is_some_and(|c| c.is_bot)
            {
                continue;
            }
            if let Some(name) = basket(reg, i.type_id) {
                t.stock.entry(name).or_default().held_by_bots += i.quantity;
            }
        }
        for (name, s) in &mut t.stock {
            s.daily_units = daily.get(name).copied().unwrap_or(0.0);
        }

        // Build share: jobs a bot started in the window, by product.
        for j in reg.jobs().all() {
            if j.start_date < from
                || j.start_date > now
                || j.product_type_id <= 0
                || !reg.character(j.installer_id).is_some_and(|c| c.is_bot)
            {
                continue;
            }
            let Some(name) = basket(reg, j.product_type_id) else {
                continue;
            };
            let e = t.build.entry(name).or_default();
            e.0 += 1;
            e.1 += j.time_in_seconds as f64 / 3_600.0;
        }

        // Watched: the top types by value, then the named ones.
        let mut top: Vec<(i64, f64)> = by_value.into_iter().collect();
        top.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        for ty in top
            .into_iter()
            .take(WATCH_N)
            .map(|x| x.0)
            .chain(extra_watch.iter().copied())
        {
            if !t.watched.contains(&ty) {
                t.watched.push(ty);
            }
        }

        t.churn = goods.0.iter().map(|(k, v)| (*k, *v)).collect();
        t.level = ledger::Closed::of(reg, from, Goods::default()).index;
        for c in &reg.config().characters {
            if c.is_bot {
                t.m_bots += c.balance;
            } else {
                t.m_humans += c.balance;
            }
        }
        t.audit = audit_of(reg, &fills, report);
        t
    }
}

/// The detector over the window's `fills` and the bots' report.
pub fn audit_of(reg: &Registry, fills: &[Fill], report: &Report) -> Audit {
    let mut a = Audit::default();
    let mut rates: Vec<(i64, f64)> = report
        .bots
        .iter()
        .filter(|b| b.hours > 0)
        .map(|b| (b.id, b.net / f64::from(b.hours)))
        .collect();
    rates.sort_by(|x, y| x.1.total_cmp(&y.1).then(x.0.cmp(&y.0)));
    if let (Some(&(id, top)), Some(&(_, median))) = (rates.last(), rates.get(rates.len() / 2))
        && median > 0.0
        && top > OUTLIER * median
    {
        a.outlier = Some((id, top, median));
    }

    let people =
        |f: &Fill| reg.character(f.buyer_id).is_some() && reg.character(f.seller_id).is_some();
    let mut pairs: BTreeMap<(i64, i64, i64), usize> = BTreeMap::new();
    let mut trips: BTreeMap<(i64, i64, i64), usize> = BTreeMap::new();
    let mut ordered: Vec<&Fill> = fills.iter().filter(|f| people(f)).collect();
    ordered.sort_by_key(|f| (f.date, f.buyer_id, f.seller_id));
    let gap = crate::game_clock::span(reg, ROUND_TRIP_HOURS * crate::seed::HOUR);
    let mut last: BTreeMap<(i64, i64, i64), (i64, i64)> = BTreeMap::new();
    for f in &ordered {
        if prices::same_group(reg, f.buyer_id, f.seller_id) {
            a.washes += 1;
        }
        *pairs
            .entry((f.buyer_id, f.seller_id, f.type_id))
            .or_default() += 1;
        let (lo, hi) = (f.buyer_id.min(f.seller_id), f.buyer_id.max(f.seller_id));
        let key = (lo, hi, f.type_id);
        if let Some(&(date, buyer)) = last.get(&key)
            && buyer != f.buyer_id
            && f.date - date <= gap
        {
            *trips.entry(key).or_default() += 1;
        }
        last.insert(key, (f.date, f.buyer_id));
    }
    a.pairs = pairs
        .into_iter()
        .filter(|(_, n)| *n > PAIR_MAX)
        .map(|((b, s, t), n)| (b, s, t, n))
        .collect();
    a.round_trips = trips
        .into_iter()
        .map(|((x, y, t), n)| (x, y, t, n))
        .collect();

    for (k, v) in reg.deltas().of(prices::KIND) {
        let (Some(ty), [avg, _]) = (k.first().and_then(|t| t.parse::<i64>().ok()), &v[..]) else {
            continue;
        };
        let (Some(p), Ok(avg)) = (prices::p_ref(reg, ty), avg.parse::<f64>()) else {
            continue;
        };
        let (lo, hi) = (prices::CLAMP.0 * p, prices::CLAMP.1 * p);
        let eps = p * 1e-9;
        if (avg - lo).abs() <= eps {
            a.pinned.push((ty, "floor"));
        } else if (avg - hi).abs() <= eps {
            a.pinned.push((ty, "ceiling"));
        }
    }
    a
}

/// `Option<f64>` as a TSV number (`0` when absent), times `scale`.
fn num(x: Option<f64>, scale: f64) -> String {
    format!("{:.0}", x.unwrap_or(0.0) * scale)
}

impl Tape {
    /// The tape as TSV rows (no header) for day `day`, on the report's
    /// nine columns. Kinds: `price`, `spot`, `pbasket`, `pindex`, `book`,
    /// `stock`, `build`, `level`, `churn`, `audit`.
    pub fn tsv_rows(&self, day: u32) -> Vec<String> {
        let scope = |r: i64| {
            if r == ALL {
                "all".to_owned()
            } else {
                r.to_string()
            }
        };
        let h = self.hours;
        let mut v = Vec::new();
        for r in self
            .rows
            .iter()
            .filter(|r| self.watched.contains(&r.type_id))
        {
            let key = format!("{}:{}", r.basket, r.type_id);
            let sc = scope(r.region);
            v.push(format!(
                "{day}\t{sc}\tprice\t{key}\t{}\t{h:.0}\t{:.2}\t{:.2}\t{:.0}",
                r.fills,
                r.vwap.unwrap_or(0.0),
                r.p_ref,
                r.index / r.p_ref * 1000.0
            ));
            if let Some(s) = r.spot() {
                v.push(format!(
                    "{day}\t{sc}\tspot\t{key}\t0\t{h:.0}\t{:.2}\t{:.2}\t{:.0}",
                    r.best_ask.unwrap_or(0.0),
                    r.best_bid.unwrap_or(0.0),
                    s / r.p_ref * 1000.0
                ));
            }
        }
        for ((region, name), b) in &self.baskets {
            let sc = scope(*region);
            v.push(format!(
                "{day}\t{sc}\tpbasket\t{name}\t{}\t{h:.0}\t{}\t{}\t{}",
                b.types,
                num(b.history_index(), 1000.0),
                num(b.spot_index(), 1000.0),
                b.units
            ));
            v.push(format!(
                "{day}\t{sc}\tpindex\t{name}\t{}\t{h:.0}\t{}\t0\t0",
                b.types,
                num(b.trading_index(), 1000.0)
            ));
            v.push(format!(
                "{day}\t{sc}\tbook\t{name}\t{}\t{h:.0}\t{}\t0\t{}",
                b.bid_units,
                b.ask_units,
                num(b.pressure(), 1000.0)
            ));
        }
        for (name, s) in &self.stock {
            v.push(format!(
                "{day}\tall\tstock\t{name}\t{}\t{h:.0}\t{}\t0\t{}",
                s.held_by_bots,
                s.on_sale_by_bots,
                num(s.days(), 100.0)
            ));
        }
        let total: f64 = self.build.values().map(|b| b.1).sum();
        for (name, (jobs, hours)) in &self.build {
            let share = if total > 0.0 { hours / total } else { 0.0 };
            v.push(format!(
                "{day}\tall\tbuild\t{name}\t{jobs}\t{hours:.0}\t0\t0\t{:.0}",
                share * 1000.0
            ));
        }
        v.push(format!(
            "{day}\tall\tlevel\tall\t0\t{h:.0}\t{}\t{:.0}\t{:.0}",
            num(self.level, 1000.0),
            self.m_bots,
            if self.m_humans > 0.0 {
                self.m_bots / self.m_humans * 1000.0
            } else {
                0.0
            }
        ));
        for (class, (minted, destroyed)) in &self.churn {
            let ratio = if *minted > 0 {
                *destroyed as f64 / *minted as f64 * 100.0
            } else {
                0.0
            };
            v.push(format!(
                "{day}\tall\tchurn\t{class}\t{destroyed}\t{h:.0}\t{minted}\t0\t{ratio:.0}"
            ));
        }
        let a = &self.audit;
        let n = |x: usize| x.to_string();
        for (name, count) in [
            ("outlier", n(usize::from(a.outlier.is_some()))),
            ("pairs", n(a.pairs.len())),
            ("round_trips", n(a.round_trips.len())),
            ("pinned", n(a.pinned.len())),
            ("washes", n(a.washes)),
        ] {
            v.push(format!(
                "{day}\tall\taudit\t{name}\t{count}\t{h:.0}\t0\t0\t0"
            ));
        }
        v
    }

    /// `/econ price-tape [region] [days]`.
    pub fn text(&self, region: Option<i64>) -> String {
        let scope = region.unwrap_or(ALL);
        let fx = |x: Option<f64>| x.map_or("-".to_owned(), |x| format!("{x:.2}"));
        let mut out = vec![format!(
            "[econ] price tape {} over the last {:.0} game hours (x = price / P_ref; history = \
             window VWAP, index = mktidx, spot = live book; bids/asks are player and bot orders)",
            region.map_or("total".to_owned(), |r| format!("region {r}")),
            self.hours
        )];
        for ((s, name), b) in self.baskets.iter().filter(|((s, _), _)| *s == scope) {
            let _ = s;
            let thin = if b.fills == 0 {
                " (thin: no trade)"
            } else {
                ""
            };
            out.push(format!(
                "basket {name}: {} types, {} units, history x{} index x{} spot x{} | book {} bid / {} ask, pressure {}{thin}",
                b.types,
                b.units,
                fx(b.history_index()),
                fx(b.trading_index()),
                fx(b.spot_index()),
                b.bid_units,
                b.ask_units,
                fx(b.pressure())
            ));
        }
        if self.baskets.keys().all(|(s, _)| *s != scope) {
            out.push("no basket traded or rests in the book in this scope".to_owned());
        }
        for r in self
            .rows
            .iter()
            .filter(|r| r.region == scope || (scope == ALL && r.region != ALL))
            .filter(|r| self.watched.contains(&r.type_id))
        {
            out.push(format!(
                "type {} [{}] region {}: {} fills, vwap {} P_ref {:.2} index {:.2} ask {} bid {}",
                r.type_id,
                r.basket,
                r.region,
                r.fills,
                fx(r.vwap),
                r.p_ref,
                r.index,
                fx(r.best_ask),
                fx(r.best_bid)
            ));
        }
        let list = self
            .stock
            .iter()
            .map(|(n, s)| {
                format!(
                    "{n} {} held, {} on sale, {} d",
                    s.held_by_bots,
                    s.on_sale_by_bots,
                    s.days().map_or("-".to_owned(), |d| format!("{d:.1}"))
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        out.push(format!(
            "bot stock: {}",
            if list.is_empty() { "none" } else { &list }
        ));
        let total: f64 = self.build.values().map(|b| b.1).sum();
        let list = self
            .build
            .iter()
            .map(|(n, (jobs, h))| {
                format!(
                    "{n} {jobs} jobs {:.0}%",
                    h / total.max(f64::EPSILON) * 100.0
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        out.push(format!(
            "builder job-hours by product: {}",
            if list.is_empty() { "none" } else { &list }
        ));
        let list = self
            .churn
            .iter()
            .map(|(c, (m, d))| format!("{c} minted {m} destroyed {d}"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(format!(
            "churn: {}",
            if list.is_empty() { "none" } else { &list }
        ));
        out.push(format!(
            "level: closed-loop index {}; bots hold {:.0} ISK, humans {:.0}",
            fx(self.level),
            self.m_bots,
            self.m_humans
        ));
        out.join("\n")
    }

    /// `/econ audit`.
    pub fn audit_text(&self) -> String {
        let a = &self.audit;
        let mut out = vec![format!(
            "[econ] audit over the last {:.0} game hours: {}",
            self.hours,
            if a.clean() { "clean" } else { "findings" }
        )];
        if let Some((id, top, median)) = a.outlier {
            out.push(format!(
                "outlier: bot {id} nets {top:.0} ISK/h, {:.1}x the median {median:.0} (alarm at {OUTLIER}x)",
                top / median
            ));
        }
        for (b, s, t, n) in &a.pairs {
            out.push(format!(
                "pair: {b} bought type {t} from {s} {n} times (alarm above {PAIR_MAX})"
            ));
        }
        for (x, y, t, n) in &a.round_trips {
            out.push(format!(
                "round trip: {x} and {y} traded type {t} both ways {n} time(s) within {ROUND_TRIP_HOURS} game hours"
            ));
        }
        for (t, edge) in &a.pinned {
            out.push(format!("pinned: type {t}'s index sits on its {edge}"));
        }
        if a.washes > 0 {
            out.push(format!(
                "wash: {} fill(s) between members of one corporation or alliance (the index ignores them, mkthist counts them)",
                a.washes
            ));
        }
        out.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Character;

    const GROUP_A: i64 = 99_101;
    const GROUP_B: i64 = 99_102;
    const CAT: i64 = 99_201;
    const T_A1: i64 = 991_001;
    const T_A2: i64 = 991_002;
    const T_B1: i64 = 991_003;
    const T_OTHER: i64 = 991_004;
    const REGION: i64 = 10_000_991;
    const ALICE: i64 = 90_000_301;
    const BOB: i64 = 90_000_302;
    const CORP: i64 = 98_000_031;
    const NOW: i64 = 100 * crate::seed::DAY;

    const DEFS: [Basket; 2] = [
        Basket {
            name: "a",
            groups: &[GROUP_A],
            categories: &[],
            words: &[],
        },
        Basket {
            name: "b",
            groups: &[],
            categories: &[CAT],
            words: &["zorp"],
        },
    ];

    /// A priced world: two types in basket a (P_ref 100 and 10), one in b
    /// (by the category and a word of its name, which is the fixture's, so
    /// the word is tested apart), one in no basket; Alice and Bob bots.
    fn world() -> Registry {
        let table = prices::Table::parse(&[
            format!("{T_A1}\t100.0\trecipe\t50.0\t4\t{GROUP_A}\t\t1"),
            format!("{T_A2}\t10.0\trecipe\t5.0\t4\t{GROUP_A}\t\t1"),
            format!("{T_B1}\t1000.0\trecipe\t500.0\t{CAT}\t{GROUP_B}\t\t1"),
            format!("{T_OTHER}\t5.0\trecipe\t2.0\t4\t99999\t\t1"),
        ])
        .expect("parses");
        let mut content = crate::content::Content::fixture();
        content.insert("econ_prices", table);
        crate::testkit::registry_with(|c| {
            c.characters = ["Alice", "Bob"]
                .iter()
                .zip([ALICE, BOB])
                .map(|(n, id)| Character {
                    character_id: id,
                    ship_id: id + 10_000_000,
                    name: (*n).to_owned(),
                    is_bot: true,
                    region_id: REGION,
                    corporation_id: CORP + (id - ALICE),
                    balance: 1_000.0,
                    ..Character::default()
                })
                .collect();
        })
        .with_content(std::sync::Arc::new(content))
    }

    fn fill(type_id: i64, quantity: i64, price: f64, buyer: i64, seller: i64, at: i64) -> Fill {
        Fill {
            date: at,
            station_id: 0,
            type_id,
            quantity,
            price,
            buyer_id: buyer,
            seller_id: seller,
        }
    }

    fn tape(reg: &Registry) -> Tape {
        let now = NOW + crate::seed::DAY;
        let report = Report::of(reg, NOW, now);
        Tape::of(reg, now, 24.0, &Goods::default(), &report, &DEFS, &[])
    }

    fn row(t: &Tape, ty: i64) -> &PriceRow {
        t.rows.iter().find(|r| r.type_id == ty).expect("a row")
    }

    /// Membership: a group, a category with a word, the whole category with
    /// no words, and none.
    #[test]
    fn baskets_hold_by_group_or_category_and_word() {
        let [a, b] = DEFS;
        assert!(in_basket(&a, 4, GROUP_A, "Whatever"));
        assert!(!in_basket(&a, CAT, GROUP_B, "Zorp"));
        assert!(in_basket(&b, CAT, GROUP_B, "Mark II ZORP Launcher"));
        assert!(!in_basket(&b, CAT, GROUP_B, "Plain"));
        assert!(!in_basket(&b, 4, GROUP_B, "zorp"));
        let all = Basket {
            name: "all",
            groups: &[],
            categories: &[CAT],
            words: &[],
        };
        assert!(in_basket(&all, CAT, 1, ""));
        let reg = world();
        assert_eq!(basket_of(&reg, T_A1, &DEFS).as_deref(), Some("a"));
        assert_eq!(basket_of(&reg, T_OTHER, &DEFS), None);
        assert_eq!(basket_of(&reg, 1, &DEFS), None, "unpriced");
    }

    /// **price_rows_are_vwap_over_pref_per_region**: two fills of one type
    /// give their volume-weighted mean beside `P_ref` and the index.
    #[test]
    fn price_rows_are_vwap_over_pref_per_region() {
        let mut reg = world();
        reg.seed_market_fills(vec![
            fill(T_A1, 10, 110.0, ALICE, BOB, NOW + 10),
            fill(T_A1, 30, 130.0, ALICE, BOB, NOW + 20),
        ]);
        let t = tape(&reg);
        let r = row(&t, T_A1);
        assert_eq!((r.fills, r.units), (2, 40));
        assert_eq!(r.vwap, Some((10.0 * 110.0 + 30.0 * 130.0) / 40.0));
        assert_eq!(
            (r.p_ref, r.index),
            (100.0, 100.0),
            "the index folds at rollover"
        );
        assert_eq!(r.region, REGION, "placed by the trader's region");
        let rows = t.tsv_rows(7);
        assert!(
            t.watched.contains(&T_A1) && rows.iter().any(|l| l.contains("\tprice\ta:991001\t2\t")),
            "{rows:?}"
        );
    }

    /// **basket_index_weights_by_value**: the dear type counts for more.
    #[test]
    fn basket_index_weights_by_value() {
        let mut reg = world();
        reg.seed_market_fills(vec![
            fill(T_A1, 1, 200.0, ALICE, BOB, NOW + 1), // 2.0 x P_ref, value 200
            fill(T_A2, 5, 5.0, ALICE, BOB, NOW + 2),   // 0.5 x P_ref, value 25
        ]);
        let t = tape(&reg);
        let b = &t.baskets[&(ALL, "a".to_owned())];
        assert_eq!(b.types, 2);
        let want = (200.0 + 25.0) / (100.0 + 50.0);
        assert_eq!(b.history_index(), Some(want));
        assert_ne!(want, (2.0 + 0.5) / 2.0, "not the plain mean of ratios");
        assert_eq!(t.baskets[&(REGION, "a".to_owned())], *b);
    }

    /// **thin_basket_is_reported_not_failed**: a basket whose types only rest
    /// in the book has a spot and a book, no history; one nobody touches has
    /// no row; both print.
    #[test]
    fn thin_basket_is_reported_not_failed() {
        let mut reg = world();
        reg.seed_market_orders(vec![
            exchange::BookRow {
                char_id: ALICE,
                type_id: T_A2,
                region_id: REGION,
                price: 12.0,
                remaining: 30,
                bid: false,
            },
            exchange::BookRow {
                char_id: BOB,
                type_id: T_A2,
                region_id: REGION,
                price: 8.0,
                remaining: 10,
                bid: true,
            },
        ]);
        let t = tape(&reg);
        let b = &t.baskets[&(REGION, "a".to_owned())];
        assert_eq!(b.history_index(), None, "no trade, no history");
        assert_eq!(b.spot_index(), Some(10.0 / 10.0), "mid of 12 and 8 over 10");
        assert_eq!((b.bid_units, b.ask_units), (10, 30));
        assert_eq!(b.pressure(), Some(-0.5));
        let text = t.text(Some(REGION));
        assert!(
            text.contains("basket a:") && text.contains("(thin: no trade)"),
            "{text}"
        );
        assert!(!t.baskets.contains_key(&(REGION, "b".to_owned())));
        assert!(
            t.text(Some(1)).contains("no basket traded"),
            "an empty scope"
        );
        let rows = t.tsv_rows(3);
        assert!(
            rows.iter()
                .any(|l| l.contains("\tbook\ta\t10\t24\t30\t0\t-500")),
            "{rows:?}"
        );
    }

    /// **churn_counts_destroyed_over_minted**, and the level row.
    #[test]
    fn churn_counts_destroyed_over_minted() {
        let reg = world();
        let now = NOW + crate::seed::DAY;
        let mut goods = Goods::default();
        goods.0.insert("module", (50, 25));
        goods.0.insert("ore", (0, 4));
        let report = Report::of(&reg, NOW, now);
        let t = Tape::of(&reg, now, 24.0, &goods, &report, &DEFS, &[]);
        let rows = t.tsv_rows(9);
        assert!(
            rows.contains(&"9\tall\tchurn\tmodule\t25\t24\t50\t0\t50".to_owned()),
            "{rows:?}"
        );
        assert!(
            rows.contains(&"9\tall\tchurn\tore\t4\t24\t0\t0\t0".to_owned()),
            "no mint, no ratio"
        );
        let level = rows
            .iter()
            .find(|l| l.contains("\tlevel\t"))
            .expect("a level row");
        assert!(
            level.ends_with("\t2000\t0"),
            "two bots holding 1000 each, no human: {level}"
        );
    }

    /// **watch_list_adds_types**: the top types by value, then the named
    /// ones, once each.
    #[test]
    fn watch_list_adds_types() {
        let mut reg = world();
        reg.seed_market_fills(vec![
            fill(T_A1, 1, 100.0, ALICE, BOB, NOW + 1),
            fill(T_A2, 1, 10.0, ALICE, BOB, NOW + 2),
        ]);
        let now = NOW + crate::seed::DAY;
        let report = Report::of(&reg, NOW, now);
        let t = Tape::of(
            &reg,
            now,
            24.0,
            &Goods::default(),
            &report,
            &DEFS,
            &[T_B1, T_A1],
        );
        assert_eq!(t.watched, vec![T_A1, T_A2, T_B1]);
    }

    /// **audit_flags_outlier_and_pair_and_pinned_type**, plus the round trip
    /// and the wash.
    #[test]
    fn audit_flags_outlier_and_pair_and_pinned_type() {
        let mut reg = world();
        let clean = tape(&reg);
        assert!(clean.audit.clean(), "{:?}", clean.audit);
        assert!(clean.audit_text().contains("clean"));

        let mut fills: Vec<Fill> = (0..=PAIR_MAX as i64)
            .map(|i| fill(T_A1, 1, 100.0, ALICE, BOB, NOW + i * 10))
            .collect();
        // Bob sells back to Alice inside the round-trip gap.
        fills.push(fill(T_A2, 1, 10.0, BOB, ALICE, NOW + 5));
        fills.push(fill(T_A2, 1, 10.0, ALICE, BOB, NOW + 6));
        reg.seed_market_fills(fills);
        // The index of T_B1 sits on the ceiling.
        reg.deltas_mut().put(
            prices::KIND,
            vec![T_B1.to_string()],
            vec![(prices::CLAMP.1 * 1000.0).to_string(), "1".to_owned()],
        );
        let t = tape(&reg);
        assert_eq!(t.audit.pairs, vec![(ALICE, BOB, T_A1, PAIR_MAX + 1)]);
        assert_eq!(t.audit.round_trips, vec![(ALICE, BOB, T_A2, 1)]);
        assert_eq!(t.audit.pinned, vec![(T_B1, "ceiling")]);
        assert!(t.audit.outlier.is_none(), "no journal, no rates");
        let text = t.audit_text();
        for want in ["pair:", "round trip:", "pinned: type 991003"] {
            assert!(text.contains(want), "{text}");
        }

        // The outlier: a pure function of the report's bot rows.
        let bot = |id: i64, net: f64| super::super::BotRow {
            id,
            region: REGION,
            career: "pve".to_owned(),
            declared: false,
            tier: "S",
            hours: 1,
            net,
            gross: net,
        };
        let report = Report {
            bots: vec![
                bot(1, 10.0),
                bot(2, 12.0),
                bot(3, 11.0),
                bot(4, 12.0 * OUTLIER + 1.0),
            ],
            ..Report::default()
        };
        let a = audit_of(&reg, &[], &report);
        assert_eq!(a.outlier, Some((4, 12.0 * OUTLIER + 1.0, 12.0)));
        let report = Report {
            bots: vec![
                bot(1, 10.0),
                bot(2, 12.0),
                bot(3, 11.0),
                bot(4, 12.0 * OUTLIER),
            ],
            ..Report::default()
        };
        assert!(
            audit_of(&reg, &[], &report).outlier.is_none(),
            "at the line is not over it"
        );
    }

    /// The stock of the bots and the builders' share of job-hours.
    #[test]
    fn stock_is_held_and_on_sale_over_daily_volume() {
        let mut reg = world();
        reg.seed_market_fills(vec![fill(T_A1, 20, 100.0, ALICE, BOB, NOW + 1)]);
        reg.seed_market_orders(vec![exchange::BookRow {
            char_id: BOB,
            type_id: T_A1,
            region_id: REGION,
            price: 105.0,
            remaining: 15,
            bid: false,
        }]);
        let t = tape(&reg);
        let s = &t.stock["a"];
        assert_eq!(s.on_sale_by_bots, 15);
        assert_eq!(s.daily_units, 20.0);
        assert_eq!(s.days(), Some(0.0), "nothing held in a hangar");
        let rows = t.tsv_rows(1);
        assert!(
            rows.iter()
                .any(|l| l.starts_with("1\tall\tstock\ta\t0\t24\t15\t0\t0")),
            "{rows:?}"
        );
    }

    /// The header is the X-ECONREPORT one; every tape row has its nine
    /// columns, so `econ-report.py` and the old tables read them.
    #[test]
    fn tsv_header_unchanged_and_rows_roundtrip() {
        assert_eq!(
            super::super::TSV_HEADER,
            "day\tscope\tkind\tkey\tn\thours\tisk\tisk2\tper_h"
        );
        let mut reg = world();
        reg.seed_market_fills(vec![fill(T_A1, 4, 120.0, ALICE, BOB, NOW + 1)]);
        let t = tape(&reg);
        let rows = t.tsv_rows(2);
        assert!(!rows.is_empty());
        for l in &rows {
            let f: Vec<&str> = l.split('\t').collect();
            assert_eq!(f.len(), 9, "{l}");
            assert_eq!(f[0], "2");
            assert!(f[4..].iter().all(|x| x.parse::<f64>().is_ok()), "{l}");
        }
        let kinds: BTreeSet<&str> = rows.iter().filter_map(|l| l.split('\t').nth(2)).collect();
        for k in ["price", "pbasket", "pindex", "book", "level", "audit"] {
            assert!(kinds.contains(k), "{k} in {kinds:?}");
        }
    }
}
