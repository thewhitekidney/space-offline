//! **Career signals from the market** (SOC-SIG, R-SOCIETY §2.2 and §3.4):
//! what a bot reads at a docked decision point so that the career mix
//! follows prices and shortages.
//!
//! - **What each career sells** is a [`Basket`] ([`BASKETS`], by career
//!   name; a career not listed reads neutral): the miner its minerals, the
//!   builder its products' margin, the ratter and the site runner their
//!   loot, under a fixed bounty share.
//! - **The price index** of a basket is `P_mkt / P_ref`
//!   ([`crate::pricing::p_mkt`]) per type, clamped to the bot price band
//!   ([`crate::pricing::BOT_PRICE_MIN`] .. `MAX`) and averaged
//!   ([`ratio_index`]); the builder's is its margin at `P_mkt` over its
//!   margin at `P_ref` ([`margin_index`]).
//! - **Corp need** is a shortage signal **[A]**: player bids for minerals
//!   outweighing player asks means minerals are short, so the miner's need
//!   rises; a product with few player asks means there is nothing to buy,
//!   so the builder's need rises ([`shortage`], [`stock_need`]). Both stay
//!   inside `1 +- NEED_SWING`.
//! - **The server's `[econ]` index** (E11) is not readable by a bot: it is
//!   a server log line and a GM's `/econ closed`. The bot's index is its
//!   own, from the same history the Market History tab reads.
//!
//! Everything but [`read`] is pure.

use crate::career::{Career, Signals};
use crate::pricing::{BOT_PRICE_MAX, BOT_PRICE_MIN, BROKER_FEE, Prices, SALES_TAX};
use crate::{Conn, Error, ItemRow, Result};
use eve_server::econ_exchange::{LOOT_CATEGORIES, TAG_GROUPS};
use eve_server::industry;
use std::collections::BTreeMap;

/// What a career's income is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basket {
    /// The minerals it mines and sells (the builder's `wants`).
    Minerals,
    /// The builder's products, by their margin over the materials.
    Products,
    /// Loot under a fixed bounty: [`BOUNTY_SHARE`] of the income is ISK
    /// from the NPC, the rest is loot at market prices.
    Loot,
    /// The trader's trade goods (`trader::trade_goods`, merge-b): their
    /// price level, the goods a trader buys and sells.
    Goods,
    /// The huffer's gas (`huffer::GAS_TYPES`, merge-b): the clouds it
    /// harvests and sells.
    Gas,
    /// The ice miner's ice products (`ice::ICE_PRODUCTS`, SOC-B1): what
    /// its refined ice sells as.
    Ice,
    /// The PI farmer's P1 (`pi::P1_TYPES`, SOC-B6): what its colonies
    /// make and it sells.
    Planetary,
    /// The hacker's datacores (`hacker::LOOT_TYPES`, HK4): what its data
    /// cans give and the builders' invention buys.
    Exploration,
}

/// **Each career's basket**, by career name **[A]**. A stage-B lane adds a
/// row for a new career (or leaves it neutral). Falsified by a career whose
/// `[soc]` income ledger moves against its basket's index.
pub static BASKETS: &[(&str, Basket)] = &[
    ("miner", Basket::Minerals),
    ("builder", Basket::Products),
    ("ratter", Basket::Loot),
    ("site_runner", Basket::Loot),
    ("pirate", Basket::Loot),
    // merge-b: the mission runner earns as the ratter does (bounty, reward
    // and loot); the trader its goods; the huffer its gas.
    ("mission_runner", Basket::Loot),
    ("trader", Basket::Goods),
    ("gas_huffer", Basket::Gas),
    // Y-WH: the diver sells Fullerite, which is gas.
    ("wormhole_diver", Basket::Gas),
    // T3-SALVAGE: the raider sells Sleeper loot and salvage, as a ratter.
    ("wormhole_raider", Basket::Loot),
    // SOC-B1: the ice miner sells its refined ice's products.
    ("ice_miner", Basket::Ice),
    // SOC-B6: the PI farmer sells its colonies' P1.
    ("pi_farmer", Basket::Planetary),
    // HK4: the explorer hacks cans and sells their datacores.
    ("explorer", Basket::Exploration),
];

/// The share of a ratter's income that is bounty, which no market moves
/// **[A]**: falsified by a ratter's ledger that follows loot prices more
/// (too high) or less (too low) than this.
pub const BOUNTY_SHARE: f64 = 0.7;
/// How far corp need moves from 1 either way (`1 +- NEED_SWING`) **[A]**:
/// falsified by a shortage that moves no bot in a day (too small) or a
/// mix that swings from one career to another on every glut (too large).
pub const NEED_SWING: f64 = 0.5;
/// Units of one product the region's player asks should hold before the
/// builder's need falls under 1 **[A]**: falsified by bots that cannot buy
/// a hull while builders leave (too low), or hulls piling up while more
/// bots become builders (too high).
pub const PRODUCT_STOCK_TARGET: i64 = 20;
/// The most loot types a decision prices (one history read each) **[A]**:
/// a bound on the calls, not a rule. Trade goods and gas are bounded by it
/// too (merge-b).
pub const LOOT_TYPES_MAX: usize = 6;

/// One type's market at a decision: `P_ref`, `P_mkt`, and the units in the
/// region's player orders (NPC orders are pinned and left out).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Quote {
    pub type_id: i64,
    pub p_ref: Option<f64>,
    pub p_mkt: Option<f64>,
    pub ask_units: i64,
    pub bid_units: i64,
    /// REVIEW-FIX2: the stock target in units for this type
    /// ([`crate::pricing::product_stock_target`] of its daily volume);
    /// `None` reads the flat [`PRODUCT_STOCK_TARGET`].
    pub stock_target: Option<i64>,
    /// REVIEW-FIX2: the best `R_eff` among the friends who posted a
    /// want-to-buy for this type ([`crate::effects::friend_wants`]);
    /// [`crate::effects::friend_need`] lifts the need when it reaches a friend.
    pub buyer_r: Option<f64>,
}

impl Quote {
    /// `P_mkt / P_ref`, clamped to the bot price band; `None` without both.
    pub fn ratio(&self) -> Option<f64> {
        let (r, m) = (self.p_ref?, self.p_mkt?);
        (r > 0.0 && m.is_finite() && m > 0.0).then(|| (m / r).clamp(BOT_PRICE_MIN, BOT_PRICE_MAX))
    }
}

/// One builder product: its quote, the units a run yields, and a run's
/// materials.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Product {
    pub quote: Quote,
    pub quantity: i64,
    pub materials: Vec<(i64, i64)>,
}

/// What a decision read of the market.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Market {
    pub minerals: Vec<Quote>,
    pub products: Vec<Product>,
    pub loot: Vec<Quote>,
    /// Trade goods with a reference price, at most [`LOOT_TYPES_MAX`].
    pub goods: Vec<Quote>,
    /// Gas types with a reference price, at most [`LOOT_TYPES_MAX`].
    pub gas: Vec<Quote>,
    /// Ice products with a reference price, at most [`LOOT_TYPES_MAX`]
    /// (SOC-B1).
    pub ice: Vec<Quote>,
    /// P1 types with a reference price, at most [`LOOT_TYPES_MAX`]
    /// (SOC-B6).
    pub planetary: Vec<Quote>,
    /// Datacores with a reference price, at most [`LOOT_TYPES_MAX`] (HK4).
    pub exploration: Vec<Quote>,
    /// Every material's `(P_ref, P_mkt)`, by type, for the margins.
    pub materials: BTreeMap<i64, (Option<f64>, Option<f64>)>,
    /// The server's `P_ref`s (`GetAverageMarketPrices`), for the goals.
    pub refs: BTreeMap<i64, f64>,
}

/// The mean clamped ratio of `quotes` that have one; `None` when none has.
pub fn ratio_index<'a>(quotes: impl IntoIterator<Item = &'a Quote>) -> Option<f64> {
    let r: Vec<f64> = quotes.into_iter().filter_map(Quote::ratio).collect();
    (!r.is_empty()).then(|| r.iter().sum::<f64>() / r.len() as f64)
}

/// One run's margin: the product sold net of tax and fee, less the
/// materials, at the prices `pick` takes from `(P_ref, P_mkt)`.
fn margin(
    p: &Product,
    mats: &BTreeMap<i64, (Option<f64>, Option<f64>)>,
    pick: impl Fn((Option<f64>, Option<f64>)) -> Option<f64>,
) -> Option<f64> {
    let price = pick((p.quote.p_ref, p.quote.p_mkt))?;
    let mut cost = 0.0;
    for &(t, q) in &p.materials {
        cost += q as f64 * pick(*mats.get(&t)?)?;
    }
    Some(price * p.quantity as f64 * (1.0 - SALES_TAX - BROKER_FEE) - cost)
}

/// **The builder's index**: its products' margin at `P_mkt` over their
/// margin at `P_ref`, clamped to the bot price band. A product whose
/// reference margin is not positive is left out; with none left, the
/// products' plain [`ratio_index`].
pub fn margin_index(m: &Market) -> Option<f64> {
    let (mut at_mkt, mut at_ref) = (0.0, 0.0);
    for p in &m.products {
        let r = margin(p, &m.materials, |(r, _)| r);
        // A material with no history prices at its reference.
        let k = margin(p, &m.materials, |(r, k)| k.or(r));
        if let (Some(r), Some(k)) = (r, k)
            && r > 0.0
        {
            at_ref += r;
            at_mkt += k;
        }
    }
    if at_ref > 0.0 {
        Some((at_mkt / at_ref).clamp(BOT_PRICE_MIN, BOT_PRICE_MAX))
    } else {
        ratio_index(m.products.iter().map(|p| &p.quote))
    }
}

/// **Need from a shortage**: `1 + NEED_SWING x (bids - asks) / (bids +
/// asks)` in player units; 1 when the book is empty.
pub fn shortage(bid_units: i64, ask_units: i64) -> f64 {
    let (b, a) = (bid_units.max(0) as f64, ask_units.max(0) as f64);
    if b + a <= 0.0 {
        return 1.0;
    }
    1.0 + NEED_SWING * (b - a) / (b + a)
}

/// **Need from stock**: `1 + NEED_SWING x (target - asks) / (target +
/// asks)`, [`PRODUCT_STOCK_TARGET`] units the neutral point: nothing to buy
/// is the most need.
pub fn stock_need(ask_units: i64) -> f64 {
    stock_need_at(ask_units, PRODUCT_STOCK_TARGET)
}

/// [`stock_need`] against a given `target` in units (at least 1).
pub fn stock_need_at(ask_units: i64, target: i64) -> f64 {
    let (t, a) = (target.max(1) as f64, ask_units.max(0) as f64);
    1.0 + NEED_SWING * (t - a) / (t + a)
}

/// **A product's need** (ship-pool): its stock need, or, when more is bid
/// than asked (combat bots resting bids for a hull or a drone no one sells),
/// the shortage, whichever is higher.
pub fn product_need(q: &Quote) -> f64 {
    let need = stock_need_at(q.ask_units, q.stock_target.unwrap_or(PRODUCT_STOCK_TARGET))
        .max(shortage(q.bid_units, q.ask_units));
    crate::effects::friend_need(need, q.buyer_r.unwrap_or(0.0))
}

/// REVIEW-FIX2: hand each product the best friend's regard among the posters
/// of a want for it (`regard` by type, [`crate::effects::friend_wants`]).
pub fn apply_buyers(m: &mut Market, regard: &BTreeMap<i64, f64>) {
    for p in &mut m.products {
        p.quote.buyer_r = regard.get(&p.quote.type_id).copied();
    }
}

/// One basket's `(index, need)`; `None` when the market said nothing of it.
pub fn reading(b: Basket, m: &Market) -> (Option<f64>, Option<f64>) {
    match b {
        Basket::Minerals => {
            let bids = m.minerals.iter().map(|q| q.bid_units).sum();
            let asks = m.minerals.iter().map(|q| q.ask_units).sum();
            let need = (!m.minerals.is_empty()).then(|| shortage(bids, asks));
            (ratio_index(&m.minerals), need)
        }
        Basket::Products => {
            let need = (!m.products.is_empty()).then(|| {
                m.products
                    .iter()
                    .map(|p| product_need(&p.quote))
                    .sum::<f64>()
                    / m.products.len() as f64
            });
            (margin_index(m), need)
        }
        Basket::Loot => (
            ratio_index(&m.loot).map(|l| BOUNTY_SHARE + (1.0 - BOUNTY_SHARE) * l),
            None,
        ),
        Basket::Goods => (ratio_index(&m.goods), None),
        Basket::Gas => (ratio_index(&m.gas), None),
        Basket::Ice => (ratio_index(&m.ice), None),
        Basket::Planetary => (ratio_index(&m.planetary), None),
        Basket::Exploration => (ratio_index(&m.exploration), None),
    }
}

/// The basket of career `name`.
pub fn basket_of(name: &str) -> Option<Basket> {
    BASKETS.iter().find(|(n, _)| *n == name).map(|&(_, b)| b)
}

/// **The signals a decision uses**: `base` (what the run blocks) with each
/// basket career's index and need from `m`, for the careers of `table`.
pub fn signals(base: &Signals, table: &[Career], m: &Market) -> Signals {
    let mut s = base.clone();
    for c in table {
        let Some(b) = basket_of(c.name) else {
            continue;
        };
        let (index, need) = reading(b, m);
        if let Some(i) = index {
            s.price_index.insert(c.name.to_owned(), i);
        }
        if let Some(n) = need {
            s.corp_need.insert(c.name.to_owned(), n);
        }
    }
    s
}

/// The baskets the served careers of `table` need read.
pub fn wanted(table: &[Career]) -> Vec<Basket> {
    let mut v: Vec<Basket> = table
        .iter()
        .filter(|c| c.served())
        .filter_map(|c| basket_of(c.name))
        .collect();
    v.sort_by_key(|b| *b as u8);
    v.dedup();
    v
}

/// The whole market's index: every quote read, minerals, products and
/// loot ([`ratio_index`]).
pub fn market_index(m: &Market) -> Option<f64> {
    ratio_index(
        m.minerals
            .iter()
            .chain(m.products.iter().map(|p| &p.quote))
            .chain(&m.loot)
            .chain(&m.goods)
            .chain(&m.gas)
            .chain(&m.ice)
            .chain(&m.planetary)
            .chain(&m.exploration),
    )
}

/// The `[soc]` line for a decision's signals: the market's index, then
/// each career with a reading as `name=index x need`.
pub fn signals_line(name: &str, s: &Signals, m: &Market) -> String {
    let fmt = |o: Option<f64>| o.map_or("-".to_owned(), |v| format!("{v:.2}"));
    let mut names: Vec<&String> = s.price_index.keys().chain(s.corp_need.keys()).collect();
    names.sort();
    names.dedup();
    let parts: Vec<String> = names
        .iter()
        .map(|n| {
            format!(
                "{n}={}x{}",
                fmt(s.price_index.get(*n).copied()),
                fmt(s.corp_need.get(*n).copied())
            )
        })
        .collect();
    format!(
        "[soc] {name} signals index {} {}",
        fmt(market_index(m)),
        parts.join(",")
    )
}

/// Loot in a hangar: packaged stacks of a module, a charge or a tag
/// (`econ_exchange::LOOT_CATEGORIES`, `TAG_GROUPS`, E10b's rule), at most
/// [`LOOT_TYPES_MAX`] types, lowest type id first.
pub fn loot_types(hangar: &[ItemRow]) -> Vec<i64> {
    let mut v: Vec<i64> = hangar
        .iter()
        .filter(|r| {
            !r.singleton
                && (LOOT_CATEGORIES.contains(&r.category_id) || TAG_GROUPS.contains(&r.group_id))
        })
        .map(|r| r.type_id)
        .collect();
    v.sort_unstable();
    v.dedup();
    v.truncate(LOOT_TYPES_MAX);
    v
}

// ---- over the wire -----------------------------------------------------------------

fn soft<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(_) => Ok(None),
    }
}

/// `type_id`'s quote: its `P_mkt` and, with `book`, its player orders.
fn quote(conn: &mut Conn, prices: &mut Prices, type_id: i64, book: bool) -> Result<Quote> {
    let p_mkt = soft(prices.p_mkt(conn, type_id))?.flatten();
    let mut q = Quote {
        type_id,
        p_ref: prices.p_ref(type_id),
        p_mkt,
        ..Quote::default()
    };
    if book && let Some(b) = soft(conn.market_orders(type_id))? {
        q.ask_units = b
            .asks
            .iter()
            .filter(|o| !o.is_npc())
            .map(|o| o.vol_remaining)
            .sum();
        q.bid_units = b
            .bids
            .iter()
            .filter(|o| !o.is_npc())
            .map(|o| o.vol_remaining)
            .sum();
        // REVIEW-FIX2: the stock target follows the region's daily volume.
        if let Some(v) = soft(prices.daily_volume(conn, type_id))? {
            q.stock_target = Some(crate::pricing::product_stock_target(v));
        }
    }
    Ok(q)
}

/// **Read the market for `baskets`** at a docked decision: the server's
/// `P_ref`s once, then per type its history (the Market History tab's
/// call) and, for minerals and products, its book (`GetOrders`). Minerals
/// are `wants` (the bundle's), products the builder's ladder in `recipes`,
/// loot what `hangar` holds, trade goods `goods` and gas
/// `crate::huffer::GAS_TYPES`, each only where the server has a `P_ref`
/// ([`priced`]). A refusal leaves that type out; only a broken connection
/// is an error.
pub fn read(
    conn: &mut Conn,
    baskets: &[Basket],
    wants: &[(i64, i64)],
    recipes: Option<&industry::Table>,
    hangar: &[ItemRow],
    goods: &[i64],
) -> Result<Market> {
    let mut m = Market::default();
    if baskets.is_empty() {
        return Ok(m);
    }
    let mut prices = match soft(Prices::read(conn))? {
        Some(p) => p,
        None => Prices::new(BTreeMap::new(), eve_server::pyutil::filetime_now()),
    };
    m.refs = prices.refs().clone();
    if baskets.contains(&Basket::Minerals) {
        for &(t, _) in wants {
            m.minerals.push(quote(conn, &mut prices, t, true)?);
        }
    }
    if baskets.contains(&Basket::Products)
        && let Some(recipes) = recipes
    {
        for &bp in &crate::builder::BLUEPRINT_LADDER {
            let Some(b) = recipes.find(bp) else {
                continue;
            };
            for &(t, _) in &b.materials {
                if let std::collections::btree_map::Entry::Vacant(e) = m.materials.entry(t) {
                    let k = soft(prices.p_mkt(conn, t))?.flatten();
                    e.insert((prices.p_ref(t), k));
                }
            }
            m.products.push(Product {
                quote: quote(conn, &mut prices, b.product_type_id, true)?,
                quantity: b.product_quantity.max(1),
                materials: b.materials.clone(),
            });
        }
    }
    if baskets.contains(&Basket::Loot) {
        for t in loot_types(hangar) {
            m.loot.push(quote(conn, &mut prices, t, false)?);
        }
    }
    if baskets.contains(&Basket::Goods) {
        for t in priced(goods, &m.refs) {
            m.goods.push(quote(conn, &mut prices, t, false)?);
        }
    }
    if baskets.contains(&Basket::Gas) {
        for t in priced(crate::huffer::GAS_TYPES, &m.refs) {
            m.gas.push(quote(conn, &mut prices, t, false)?);
        }
    }
    if baskets.contains(&Basket::Ice) {
        for t in priced(crate::ice::ICE_PRODUCTS, &m.refs) {
            m.ice.push(quote(conn, &mut prices, t, false)?);
        }
    }
    if baskets.contains(&Basket::Planetary) {
        for t in priced(crate::pi::P1_TYPES, &m.refs) {
            m.planetary.push(quote(conn, &mut prices, t, false)?);
        }
    }
    if baskets.contains(&Basket::Exploration) {
        for t in priced(crate::hacker::LOOT_TYPES, &m.refs) {
            m.exploration.push(quote(conn, &mut prices, t, false)?);
        }
    }
    Ok(m)
}

/// The first [`LOOT_TYPES_MAX`] of `types` the server prices (`refs`, the
/// `P_ref`s): a type without one has no index to read.
pub fn priced(types: &[i64], refs: &BTreeMap<i64, f64>) -> Vec<i64> {
    types
        .iter()
        .copied()
        .filter(|t| refs.get(t).is_some_and(|&p| p > 0.0))
        .take(LOOT_TYPES_MAX)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::career::CAREERS;
    use crate::mind::{DAY, FLAP_MAX, Mind};

    /// An invented quote at `ratio x` its reference, with player orders.
    fn q(type_id: i64, ratio: f64, asks: i64, bids: i64) -> Quote {
        Quote {
            type_id,
            p_ref: Some(10.0),
            p_mkt: Some(10.0 * ratio),
            ask_units: asks,
            bid_units: bids,
            ..Default::default()
        }
    }

    /// Invented minerals: two types at `ratio`, the book `bids` / `asks`.
    fn minerals(ratio: f64, asks: i64, bids: i64) -> Market {
        Market {
            minerals: vec![
                q(1, ratio, asks / 2, bids / 2),
                q(2, ratio, asks / 2, bids / 2),
            ],
            ..Market::default()
        }
    }

    #[test]
    fn the_indices_clamp_to_the_band_and_need_stays_in_its_swing() {
        assert_eq!(ratio_index(&[q(1, 1.2, 0, 0), q(2, 0.8, 0, 0)]), Some(1.0));
        assert_eq!(ratio_index(&[q(1, 100.0, 0, 0)]), Some(BOT_PRICE_MAX));
        assert_eq!(ratio_index(&[Quote::default()]), None);
        assert_eq!(shortage(0, 0), 1.0);
        assert_eq!(shortage(10, 0), 1.0 + NEED_SWING);
        assert_eq!(shortage(0, 10), 1.0 - NEED_SWING);
        assert_eq!(stock_need(PRODUCT_STOCK_TARGET), 1.0);
        assert_eq!(stock_need(0), 1.0 + NEED_SWING);
        assert!(stock_need(1_000_000) > 1.0 - NEED_SWING);
    }

    /// The builder's index is its margin's move: dearer minerals at an
    /// unchanged product price shrink it, a dearer product grows it.
    #[test]
    fn the_builder_reads_its_margin_not_its_price() {
        let product = |ratio: f64| Product {
            quote: q(100, ratio, 0, 0),
            quantity: 1,
            materials: vec![(1, 2), (2, 1)],
        };
        let mats = |k: f64| {
            [
                (1, (Some(2.0), Some(2.0 * k))),
                (2, (Some(1.0), Some(1.0 * k))),
            ]
            .into_iter()
            .collect()
        };
        let m = |p: f64, k: f64| Market {
            products: vec![product(p)],
            materials: mats(k),
            ..Market::default()
        };
        assert!((margin_index(&m(1.0, 1.0)).unwrap() - 1.0).abs() < 1e-9);
        assert!(margin_index(&m(1.0, 1.5)).unwrap() < 1.0);
        assert!(margin_index(&m(1.3, 1.0)).unwrap() > 1.3);
        // No reference margin: the product's own ratio.
        let mut thin = m(1.2, 1.0);
        thin.materials = mats(1.0)
            .into_iter()
            .map(|(t, (r, _))| (t, (r.map(|r| r * 100.0), None)))
            .collect();
        assert!((margin_index(&thin).unwrap() - 1.2).abs() < 1e-9);
    }

    #[test]
    fn signals_land_on_the_basket_careers_and_keep_the_blocks() {
        let mut base = Signals::default();
        base.blocked.insert("mission_runner".to_owned());
        let mut m = minerals(1.4, 0, 1_000);
        m.loot = vec![q(7, 2.0, 0, 0)];
        let s = signals(&base, CAREERS, &m);
        assert!(s.blocked.contains("mission_runner"));
        assert!((s.price_index["miner"] - 1.4).abs() < 1e-9);
        assert_eq!(s.corp_need["miner"], 1.0 + NEED_SWING);
        let loot = BOUNTY_SHARE + (1.0 - BOUNTY_SHARE) * 2.0;
        assert!((s.price_index["ratter"] - loot).abs() < 1e-9);
        assert!(!s.price_index.contains_key("builder"), "no products read");
        assert!(!s.corp_need.contains_key("ratter"));
        let line = signals_line("Invented Pilot", &s, &m);
        for f in [
            "[soc] Invented Pilot signals index",
            "miner=1.40x1.50",
            "ratter=",
        ] {
            assert!(line.contains(f), "{line} lacks {f}");
        }
        assert_eq!(
            signals(&base, CAREERS, &Market::default())
                .price_index
                .len(),
            0
        );
        assert!(wanted(CAREERS).contains(&Basket::Minerals));
    }

    /// SOC-B1's row: the ice miner reads its ice products, and nothing
    /// else moves it.
    #[test]
    fn the_ice_miner_reads_its_ice_products() {
        let m = Market {
            ice: vec![q(10, 1.2, 0, 0), q(11, 0.6, 0, 0)],
            ..Market::default()
        };
        let s = signals(&Signals::default(), CAREERS, &m);
        assert!((s.price_index["ice_miner"] - 0.9).abs() < 1e-9);
        assert!(!s.price_index.contains_key("miner"));
        assert!(!s.corp_need.contains_key("ice_miner"));
        assert!(wanted(CAREERS).contains(&Basket::Ice));
    }

    /// merge-b's rows: the trader reads its goods, the huffer its gas, the
    /// mission runner the ratter's loot; only priced types are read.
    #[test]
    fn the_trader_the_huffer_and_the_mission_runner_read_their_baskets() {
        let m = Market {
            loot: vec![q(7, 2.0, 0, 0)],
            goods: vec![q(8, 1.2, 0, 0)],
            gas: vec![q(9, 0.6, 0, 0)],
            ..Market::default()
        };
        let s = signals(&Signals::default(), CAREERS, &m);
        assert!((s.price_index["trader"] - 1.2).abs() < 1e-9);
        assert!((s.price_index["gas_huffer"] - 0.6).abs() < 1e-9);
        assert_eq!(s.price_index["mission_runner"], s.price_index["ratter"]);
        assert!(!s.corp_need.contains_key("trader"));
        let refs: BTreeMap<i64, f64> = [(2, 5.0), (3, 0.0), (4, 1.0)].into_iter().collect();
        assert_eq!(priced(&[1, 2, 3, 4], &refs), vec![2, 4]);
        let many: Vec<i64> = (1..=20).collect();
        let all: BTreeMap<i64, f64> = many.iter().map(|&t| (t, 1.0)).collect();
        assert_eq!(priced(&many, &all).len(), LOOT_TYPES_MAX);
    }

    /// One society of `bots` minds, seeded at neutral signals, deciding
    /// every `DECIDE_EVERY` for `days` under `m`. Returns the miners at the
    /// end and the switches per bot-day.
    fn run(minds: &mut [Mind], s: &Signals, from: u64, days: u64) -> (usize, f64) {
        let step = crate::driver::DECIDE_EVERY.as_secs();
        let mut switches = 0usize;
        let mut t = from;
        while t < from + days * DAY {
            for m in minds.iter_mut() {
                let before = m.career.clone();
                m.decide(CAREERS, s, t);
                switches += usize::from(m.career != before);
            }
            t += step;
        }
        let miners = minds.iter().filter(|m| m.career == "miner").count();
        (miners, switches as f64 / (minds.len() as u64 * days) as f64)
    }

    /// **The mix follows the market** (a pure simulation over the real
    /// table): a mineral shortage at dearer minerals grows the miners over
    /// a few days of decisions, a glut shrinks them, and no bot switches
    /// more than `FLAP_MAX` a day on average.
    #[test]
    fn a_mineral_shortage_grows_the_miners_and_a_glut_shrinks_them() {
        let mut base = Signals::default();
        base.blocked.insert("mission_runner".to_owned());
        let start = 1_000_000;
        let mut minds: Vec<Mind> = (0..50)
            .map(|i| Mind::seeded(&format!("Invented Pilot {i}"), 1, CAREERS, &base, start))
            .collect();
        let neutral = minds.iter().filter(|m| m.career == "miner").count();
        // Neutral prices: nobody moves.
        let (same, rate) = run(&mut minds, &base, start, 2);
        assert_eq!((same, rate), (neutral, 0.0));
        // Short: bids for minerals, no asks, minerals dearer.
        let short = signals(&base, CAREERS, &minerals(1.5, 0, 10_000));
        let (grown, rate) = run(&mut minds, &short, start + 2 * DAY, 3);
        assert!(grown > neutral, "shortage: {neutral} -> {grown}");
        assert!(rate <= FLAP_MAX, "{rate} switches per bot-day");
        // A glut: asks, no bids, minerals cheap.
        let glut = signals(&base, CAREERS, &minerals(0.6, 10_000, 0));
        let (shrunk, rate) = run(&mut minds, &glut, start + 5 * DAY, 3);
        assert!(shrunk < neutral, "glut: {grown} -> {shrunk}");
        assert!(rate <= FLAP_MAX, "{rate} switches per bot-day");
    }

    /// REVIEW-FIX2: the stock target and a friend's want reach the need.
    #[test]
    fn review_fix2_product_need_uses_the_volume_target_and_a_friends_want() {
        let base = q(1, 1.0, 40, 0);
        let flat = product_need(&base);
        assert_eq!(flat, stock_need(40));
        // A busy region wants a bigger stock: 40 asks is then below target.
        let busy = Quote {
            stock_target: Some(crate::pricing::product_stock_target(1_000.0)),
            ..base
        };
        assert!(product_need(&busy) > flat);
        assert_eq!(
            product_need(&busy),
            stock_need_at(40, crate::pricing::product_stock_target(1_000.0))
        );
        // A friend's want lifts it; a stranger's does not.
        let friend = Quote {
            buyer_r: Some(crate::society::FRIEND_AT),
            ..base
        };
        assert_eq!(
            product_need(&friend),
            flat * crate::effects::FRIEND_NEED_BONUS
        );
        let stranger = Quote {
            buyer_r: Some(0.0),
            ..base
        };
        assert_eq!(product_need(&stranger), flat);
        let mut m = Market::default();
        m.products.push(Product {
            quote: base,
            ..Product::default()
        });
        apply_buyers(&mut m, &BTreeMap::from([(1, crate::society::FRIEND_AT)]));
        assert_eq!(m.products[0].quote.buyer_r, Some(crate::society::FRIEND_AT));
    }
}
