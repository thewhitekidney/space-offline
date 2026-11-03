//! **Bot pricing** (R-ECON §2.5 and §3 row E10): the one module every bot
//! prices with -- the builder's asks and standing bids, the miner's least
//! sale, the ratter's loot asks, and what a consumer pays for a hull, a
//! module, a round or a blueprint.
//!
//! - **The anchor.** `P_ref` is the server's reference price
//!   (`config.GetAverageMarketPrices`' `adjustedPrice`, CP31). Once a type
//!   has [`MIN_HIST_DAYS`] days of trades inside the last [`HIST_WINDOW`]
//!   days of the region's history (`marketProxy.GetOldPriceHistory`, the
//!   Market History tab's call, CP42; the folded days E5 keeps in
//!   `mkthist`), `P_mkt` is their volume-weighted average; before that it is
//!   `P_ref` ([`p_mkt`]).
//! - **The seller** ([`ask`]): `max(cost x (1 + MARGIN), P_mkt x (1 -
//!   UNDERCUT))`, lowered by one [`EXCESS_STEP`] a day while the stock is
//!   more than [`TARGET_DAYS`] of the region's sales ([`excess_steps`]),
//!   never under the cost floor.
//! - **The buyer** ([`margin_bids`]): a builder's standing bids leave
//!   [`MARGIN`] of the product's `P_mkt` after the tax and the fees.
//! - **The consumer** ([`reservation`]): the cheapest ask up to
//!   `RESERVATION x P_mkt`.
//! - **The value** ([`value_of`], FE-VALUE, R-FLUIDECON §3.1): `V` blends the
//!   live book's mid, the history EWMA ([`ewma_of`]) and the server's
//!   `averagePrice`; it stands where `P_mkt` stood. Asks mark up on bid
//!   pressure ([`FE_SCARCITY_K`]); a rise is capped per repricing
//!   ([`FE_MAX_STEP`]) and per game day ([`FE_MAX_DAY_MOVE`]).
//! - **The clamp** ([`band`]): a bot never quotes outside `[FE_BAND_MIN,
//!   FE_BAND_MAX] x P_ref`. That is the runaway guard; humans are not
//!   clamped. An NPC's own bid is the NPC's quote, not the bot's, so a bot
//!   may still sell into one.
//!
//! Everything but the wire read ([`Conn::price_history`]) is pure. Every
//! rule here is **[A]** policy, except the call's shape, which it cites.

use crate::{Conn, Error, Result};
use eve_marshal::{DbValue, PyValue};
use eve_server::industry::Blueprint;
use eve_server::market_fees::{Standings, TradeSkills};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a fetched price index is reused by every bot in the process.
/// **[A: 300 s]** -- the server moves the index once a game day
/// (`econ_prices::fold_day`), so five minutes of reuse costs a bot nothing it
/// could see; falsified by a bot pricing off a number that a visible market
/// move has already made wrong (then shorten it, or drop the cache).
pub const PRICE_INDEX_TTL: Duration = Duration::from_secs(300);

/// `(typeID, adjustedPrice, averagePrice)` rows of `GetAverageMarketPrices`.
pub type PriceRows = Vec<(i64, f64, Option<f64>)>;

/// What a connection holds: the rows, shared.
pub type SharedPriceRows = Arc<PriceRows>;

/// **PERF-BURST**: one `GetAverageMarketPrices` answer for the whole bot
/// process, single-flight. The first bot to ask holds the mutex while it
/// fetches; the others wait on it and then read the same `Arc` (300 logins ->
/// 1 call, where each used to make the server copy and marshal a ~12k-entry
/// dict under its lock). A failed fetch is not kept, so the next bot retries.
#[derive(Debug, Default)]
pub struct PriceIndexCache {
    slot: Mutex<Option<(Instant, Arc<PriceRows>)>>,
}

impl PriceIndexCache {
    /// The index at `now`: the kept one while younger than
    /// [`PRICE_INDEX_TTL`], else `fetch(now)`'s, kept.
    pub fn get_or_fetch(
        &self,
        now: Instant,
        fetch: impl FnOnce(Instant) -> Result<PriceRows>,
    ) -> Result<SharedPriceRows> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, rows)) = slot.as_ref()
            && now.saturating_duration_since(*at) < PRICE_INDEX_TTL
        {
            return Ok(Arc::clone(rows));
        }
        let rows = Arc::new(fetch(now)?);
        *slot = Some((now, Arc::clone(&rows)));
        Ok(rows)
    }
}

// ---- the rule's constants ----------------------------------------------------------

/// **The margin** a bot keeps over its cost (R-ECON §2.5's `MARGIN`)
/// **[A]**: an ask is never under `cost x (1 + MARGIN)`, and a standing bid
/// leaves this much of the product's net worth. Falsified by builders whose
/// bids never fill while humans sell ore elsewhere (too high) or who build
/// at a loss under `P_mkt` (too low).
pub const MARGIN: f64 = 0.10;
/// How far under `P_mkt` a seller asks (`UNDERCUT`) **[A]**: falsified by
/// undercut wars (too high: `[econ]`'s index sliding day on day with no
/// glut) or asks that sit while others sell (too low).
pub const UNDERCUT: f64 = 0.01;
/// Days with trades a type needs inside [`HIST_WINDOW`] before its history
/// replaces `P_ref` in the flat [`p_mkt`] (`MIN_HIST_DAYS`) **[A]**; FE-VALUE
/// lowered it from 3 to 1 (one traded day is enough for [`ewma_of`], and the
/// outlier rule holds a lone odd day back). Falsified by one odd day moving
/// every bot's price (too low) or by bots quoting `P_ref` for a week against
/// a market that has moved (too high).
pub const MIN_HIST_DAYS: usize = 1;
/// The history window, whole UTC days before today (`HIST_WINDOW`) **[A]**:
/// falsified by `P_mkt` lagging a move for longer than a player would
/// notice it (too long) or swinging with a day's volume (too short).
pub const HIST_WINDOW: i64 = 7;
/// Days of the region's sales a seller's stock may cover before its ask
/// steps down (`TARGET_DAYS`) **[A]**: falsified by stock piling up behind
/// an ask (too high) or by asks dropping on a normal day's stock (too low).
pub const TARGET_DAYS: f64 = 7.0;
/// What one day of excess stock takes off the ask, compounding **[A]**:
/// falsified by gluts that take weeks to clear (too small) or by asks that
/// reach the cost floor in two days (too large).
pub const EXCESS_STEP: f64 = 0.05;
/// REVIEW-FIX: the lowest order price, as a share of the slow anchor, that
/// the live book counts (bids and asks alike) **[A]**: a bid for a billion
/// units at the minimum price is no demand. Falsified by a real shortage
/// whose bids all sat below this (the bots then read it as no demand).
pub const FE_SPOT_BAND_LO: f64 = 0.5;
/// REVIEW-FIX: the highest counted price, a share of the slow anchor **[A]**,
/// falsified as [`FE_SPOT_BAND_LO`].
pub const FE_SPOT_BAND_HI: f64 = 2.0;
/// REVIEW-FIX: the most one order's units weigh in the book, in days of the
/// regional volume **[A]**. The wire hides the issuer (`GetOrders` has no
/// character column [V]), so the per-pilot cap is a per-order one.
pub const FE_SPOT_ORDER_DAYS: f64 = 1.0;
/// REVIEW-FIX: the least one order may weigh, in units **[A]**, whatever the
/// regional volume (a cold market has none): a replacement bid for a doctrine
/// is thousands of rounds. Falsified by a real order of more units than this
/// that the bots must read whole.
pub const FE_SPOT_ORDER_FLOOR: f64 = 5_000.0;

/// The clamp's low end, a share of the anchor (`FE_BAND_MIN`) **[A]**: no
/// bot asks, bids or sells into a player's bid under it. FE-VALUE widened it
/// from 0.5 (R-FLUIDECON §3.2); the server's index clamp
/// (`econ_prices::CLAMP`) moves with it. Falsified by `[econ]` showing a
/// type pinned at either edge on 20 % or more of type-days (widen), or the
/// index leaving `[0.25, 6]` for three days (narrow, and look at the faucets).
pub const FE_BAND_MIN: f64 = 0.30;
/// The clamp's high end (`FE_BAND_MAX`) **[A]**: falsified as
/// [`FE_BAND_MIN`].
pub const FE_BAND_MAX: f64 = 4.0;
/// Older name of [`FE_BAND_MIN`].
pub const BOT_PRICE_MIN: f64 = FE_BAND_MIN;
/// Older name of [`FE_BAND_MAX`].
pub const BOT_PRICE_MAX: f64 = FE_BAND_MAX;
/// The weight of the live book's mid in the perceived value `V`
/// (`FE_W_SPOT`) **[A]**. Falsified by spot flicker moving quotes more than
/// [`FE_MAX_STEP`] per cycle on a thin book (lower it), or by a lag of more
/// than two game days behind a shock (raise it).
pub const FE_W_SPOT: f64 = 0.5;
/// The weight of the history EWMA in `V` (`FE_W_EWMA`) **[A]**.
pub const FE_W_EWMA: f64 = 0.3;
/// The weight of the server's `averagePrice` in `V` (`FE_W_IDX`) **[A]**.
pub const FE_W_IDX: f64 = 0.2;
/// The EWMA's step per game day (`FE_EWMA_ALPHA`) **[A]**: a half-life of
/// about 1.6 days. Falsified by a shock that takes longer than two days to
/// show half its size (raise), or a tape that follows one day's noise
/// (lower).
pub const FE_EWMA_ALPHA: f64 = 0.35;
/// The EWMA reads this many whole days back (`FE_EWMA_WINDOW`) **[A]**.
pub const FE_EWMA_WINDOW: i64 = 14;
/// A day with fewer units traded does not count in the EWMA
/// (`FE_MIN_DAY_VOLUME`) **[A]**; 1 means every traded day counts, so the
/// outlier rule alone guards the average. Falsified by a one-unit day
/// moving a thin type's `V` (raise).
pub const FE_MIN_DAY_VOLUME: i64 = 1;
/// A day whose VWAP is outside `[1/FE_OUTLIER_BAND, FE_OUTLIER_BAND] x` the
/// running EWMA is held back until the next traded day agrees with it
/// (`FE_OUTLIER_BAND`, R-FLUIDECON G2) **[A]**. Falsified by a real shock
/// that waits a day for confirmation in `[econ]`'s tape (raise).
pub const FE_OUTLIER_BAND: f64 = 2.0;
/// Two days agree when their VWAPs are within this share of each other
/// (`FE_OUTLIER_AGREE`) **[A]**.
pub const FE_OUTLIER_AGREE: f64 = 0.25;
/// How far the book's one side is moved to estimate the mid
/// (`FE_SPOT_ONE_SIDED`) **[A]**. The spec said 0.03 (an ask alone reads as
/// `ask x 0.97`); the builder socket tests falsified it: a lone seller at
/// `P_ref` made `V` 2 % under its ask, so the builders' margin bids sat under
/// the only ask and never filled. 0 reads a lone side as itself (the day cap
/// below keeps a lone odd quote from dragging `V`). Raise it only with the
/// buyer's take cap, or builders stop buying from a lone seller.
pub const FE_SPOT_ONE_SIDED: f64 = 0.0;
/// Daily volume floor when turning units into days (`FE_VOL_FLOOR`) **[A]**.
pub const FE_VOL_FLOOR: f64 = 1.0;
/// The most a bot raises its own ask or bid in one repricing
/// (`FE_MAX_STEP`) **[A]**. Falsified by a justified +50 % level that takes
/// more than eight game days to reach (raise).
pub const FE_MAX_STEP: f64 = 0.15;
/// The most a quote differs from the last settled history level (the EWMA)
/// (`FE_MAX_DAY_MOVE`) **[A]**; the EWMA itself moves at most
/// [`FE_EWMA_ALPHA`] of the gap a day, so a quote moves at most this per
/// game day however the book swings. Same falsifier as [`FE_MAX_STEP`].
pub const FE_MAX_DAY_MOVE: f64 = 0.30;
/// How far over (under) `V` a seller asks at full bid (ask) pressure
/// (`FE_SCARCITY_K`) **[A]**. Falsified by asks at the mark-up that never
/// fill (lower), or an index that overshoots 2.5x (lower).
pub const FE_SCARCITY_K: f64 = 0.25;
/// Days of regional volume a product stock aims at (`FE_PRODUCT_STOCK_DAYS`)
/// **[A]**, with the floor and ceiling in units ([`product_stock_target`]).
/// Falsified by builders holding unsold stacks for more than ten days
/// (lower) or running dry daily (raise).
pub const FE_PRODUCT_STOCK_DAYS: f64 = 5.0;
/// The least stock target, units (`FE_STOCK_MIN`) **[A]**.
pub const FE_STOCK_MIN: i64 = 2;
/// The most stock target, units (`FE_STOCK_MAX`) **[A]**.
pub const FE_STOCK_MAX: i64 = 60;
/// **The consumer's reservation price** (`RESERVATION`): the most a bot
/// pays for a ship, module, round or blueprint, as a multiple of the type's
/// `P_mkt` **[A]**. Falsified by bots that pay more than a human would for
/// a hull (too high) or never buy one while asks rest (too low).
pub const RESERVATION: f64 = 1.5;
/// The most a builder pays for a material, as a fraction of its `P_mkt`
/// **[A]**: falsified by builders that never find materials to buy.
pub const BUY_CAP_OF_MKT: f64 = 1.0;
/// The market's price step, one cent: the server rounds NPC prices to it
/// (`econ_exchange::to_cents`).
pub const TICK: f64 = 0.01;
/// The sales tax a pilot with no Accounting pays on a sale, 8 % (C5 §5.1,
/// E5's server formula) **[V]**.
pub const SALES_TAX: f64 = 0.08;
/// `expectedBrokersFee`, the fraction a bot sends: the base broker rate of a
/// pilot with no Broker Relations and no standings **[A]** (falsified by
/// the client's `brokerFeeNow` for such a pilot, `sellMulti.py:481`).
pub const BROKER_FEE: f64 = 0.03;

/// **What a bot pays the market at its skills** (X-TRADE): the sales tax on
/// a sale and the broker's commission on a resting order, both fractions of
/// the order's value, as the server charges them
/// (`eve_server::market_fees`, the client's `marketutil` formulas **[V]**).
/// [`Rates::UNSKILLED`] is a pilot with no Accounting, no Broker Relations
/// and no standings: the constants above.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    pub sales_tax: f64,
    pub broker_fee: f64,
}

impl Rates {
    /// A pilot with none of the trade skills.
    pub const UNSKILLED: Rates = Rates {
        sales_tax: SALES_TAX,
        broker_fee: BROKER_FEE,
    };

    /// The rates of a pilot with `skills` whose station owner stands at
    /// `standings` toward it. Bots read no standings today, so callers pass
    /// `Standings::default()` **[A]** (falsified by a bot with mission
    /// standings whose journal's broker's fee is under this rate).
    pub fn at(skills: &TradeSkills, standings: Standings) -> Rates {
        Rates {
            sales_tax: skills.sales_tax_rate(),
            broker_fee: skills.broker_rate(standings),
        }
    }

    /// ISK kept from selling one unit at `price` into a bid: the sales tax
    /// comes off.
    pub fn net_of_sale(&self, price: f64) -> f64 {
        price * (1.0 - self.sales_tax)
    }

    /// ISK kept from one unit that sells from a resting ask at `price`: the
    /// sales tax and the broker's fee for listing it come off.
    pub fn net_of_listing(&self, price: f64) -> f64 {
        price * (1.0 - self.sales_tax - self.broker_fee)
    }

    /// ISK spent on one unit bought with a resting bid at `price`: the
    /// broker's fee goes on top.
    pub fn cost_of_bid(&self, price: f64) -> f64 {
        price * (1.0 + self.broker_fee)
    }
}

/// FILETIME ticks in a day: a whole number of them is a UTC midnight
/// (`GetHistoryRowList`, marketsvc.py:388) **[V]**.
pub const DAY_TICKS: i64 = 86_400 * crate::verbs::FILETIME_SECOND;

/// Up to the cent.
pub fn up_to_cent(x: f64) -> f64 {
    (x * 100.0).ceil() / 100.0
}

/// Down to the cent.
pub fn down_to_cent(x: f64) -> f64 {
    (x * 100.0).floor() / 100.0
}

fn positive(x: f64) -> Option<f64> {
    (x.is_finite() && x > 0.0).then_some(x)
}

// ---- the anchor --------------------------------------------------------------------

/// One day of a type's history: `GetOldPriceHistory`'s row, the columns a
/// price needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Day {
    /// The day's midnight, FILETIME.
    pub day: i64,
    /// Its volume-weighted mean price.
    pub avg: f64,
    pub volume: i64,
}

/// The days of `days` inside the window before `today` (a FILETIME; any
/// time of day) that traded.
fn in_window(days: &[Day], today: i64) -> impl Iterator<Item = &Day> {
    let midnight = today.div_euclid(DAY_TICKS) * DAY_TICKS;
    let from = midnight - HIST_WINDOW * DAY_TICKS;
    days.iter().filter(move |d| {
        d.day >= from && d.day < midnight && d.volume > 0 && positive(d.avg).is_some()
    })
}

/// **`P_mkt`**: the volume-weighted average of the window's days once at
/// least [`MIN_HIST_DAYS`] of them traded, else `p_ref`.
pub fn p_mkt(p_ref: Option<f64>, days: &[Day], today: i64) -> Option<f64> {
    let traded: Vec<&Day> = in_window(days, today).collect();
    if traded.len() < MIN_HIST_DAYS {
        return p_ref.and_then(positive);
    }
    let volume: f64 = traded.iter().map(|d| d.volume as f64).sum();
    let isk: f64 = traded.iter().map(|d| d.avg * d.volume as f64).sum();
    positive(isk / volume).or(p_ref.and_then(positive))
}

/// **The history EWMA** (FE-VALUE): the folded days of the last
/// [`FE_EWMA_WINDOW`] whole days before `today`, oldest first, each moving
/// the average [`FE_EWMA_ALPHA`] of the way to its VWAP (compounded over a
/// gap of days with no trade). One traded day is enough. A day under
/// [`FE_MIN_DAY_VOLUME`] is skipped; a day outside
/// `[1/FE_OUTLIER_BAND, FE_OUTLIER_BAND] x` the running average waits for
/// the next traded day to agree with it ([`FE_OUTLIER_AGREE`]) and is
/// dropped if none does (a wash day, R-FLUIDECON G2). `None` with no
/// usable day.
pub fn ewma_of(days: &[Day], today: i64) -> Option<f64> {
    let midnight = today.div_euclid(DAY_TICKS) * DAY_TICKS;
    let from = midnight - FE_EWMA_WINDOW * DAY_TICKS;
    let mut rows: Vec<&Day> = days
        .iter()
        .filter(|d| {
            d.day >= from
                && d.day < midnight
                && d.volume >= FE_MIN_DAY_VOLUME.max(1)
                && positive(d.avg).is_some()
        })
        .collect();
    rows.sort_by_key(|d| d.day);
    let mut est: Option<(f64, i64)> = None;
    let mut pending: Option<f64> = None;
    for d in rows {
        let Some((e, last)) = est else {
            est = Some((d.avg, d.day));
            continue;
        };
        let outlier = d.avg > e * FE_OUTLIER_BAND || d.avg < e / FE_OUTLIER_BAND;
        let mut accept = d.avg;
        if outlier {
            match pending {
                Some(p) if (d.avg / p - 1.0).abs() <= FE_OUTLIER_AGREE => {
                    pending = None;
                }
                _ => {
                    pending = Some(d.avg);
                    continue;
                }
            }
        } else {
            pending = None;
            accept = d.avg;
        }
        let gap = ((d.day - last) / DAY_TICKS).max(1) as i32;
        let alpha = 1.0 - (1.0 - FE_EWMA_ALPHA).powi(gap);
        est = Some((e + alpha * (accept - e), d.day));
    }
    est.map(|(e, _)| e)
}

/// One type's live book in the pilot's region, **player orders only** (an
/// NPC order is a fixed anchor, not the market's mood): the best prices and
/// the units resting on each side.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Spot {
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
    pub bid_units: i64,
    pub ask_units: i64,
}

impl Spot {
    /// The player half of `book`.
    pub fn of(book: &crate::verbs::Book) -> Spot {
        Spot::without(book, &BTreeSet::new())
    }

    /// [`Spot::of`], leaving out the orders in `own`: a bot's own resting
    /// ask must not drag the value its next ask is priced from (a bot would
    /// chase its own undercut down).
    pub fn without(book: &crate::verbs::Book, own: &BTreeSet<i64>) -> Spot {
        let bids = book
            .bids
            .iter()
            .filter(|o| !o.is_npc() && !own.contains(&o.order_id));
        let asks = book
            .asks
            .iter()
            .filter(|o| !o.is_npc() && !own.contains(&o.order_id));
        Spot {
            best_bid: bids.clone().map(|o| o.price).max_by(f64::total_cmp),
            best_ask: asks.clone().map(|o| o.price).min_by(f64::total_cmp),
            bid_units: bids.map(|o| o.vol_remaining.max(0)).sum(),
            ask_units: asks.map(|o| o.vol_remaining.max(0)).sum(),
        }
    }

    /// REVIEW-FIX: [`Spot::without`] over the orders that matter. With an
    /// `anchor` (the slow value, no book in it) only prices within
    /// `[FE_SPOT_BAND_LO, FE_SPOT_BAND_HI] x anchor` count, for the best
    /// prices and the units alike, and one order weighs at most
    /// [`FE_SPOT_ORDER_DAYS`] of `daily_volume` (never below
    /// [`FE_SPOT_ORDER_FLOOR`]).
    pub fn banded(
        book: &crate::verbs::Book,
        own: &BTreeSet<i64>,
        anchor: Option<f64>,
        daily_volume: f64,
    ) -> Spot {
        let cap = (FE_SPOT_ORDER_DAYS * daily_volume.max(0.0))
            .max(FE_SPOT_ORDER_FLOOR)
            .ceil() as i64;
        let anchor = anchor.and_then(positive);
        let keep = |o: &&crate::verbs::BookOrder| {
            !o.is_npc()
                && !own.contains(&o.order_id)
                && anchor.is_none_or(|a| {
                    o.price >= a * FE_SPOT_BAND_LO && o.price <= a * FE_SPOT_BAND_HI
                })
        };
        let bids: Vec<_> = book.bids.iter().filter(keep).collect();
        let asks: Vec<_> = book.asks.iter().filter(keep).collect();
        let units = |v: &[&crate::verbs::BookOrder]| -> i64 {
            v.iter().map(|o| o.vol_remaining.clamp(0, cap)).sum()
        };
        Spot {
            best_bid: bids.iter().map(|o| o.price).max_by(f64::total_cmp),
            best_ask: asks.iter().map(|o| o.price).min_by(f64::total_cmp),
            bid_units: units(&bids),
            ask_units: units(&asks),
        }
    }

    /// **The mid**: both sides average; one side alone is moved by
    /// [`FE_SPOT_ONE_SIDED`]; an empty book has none.
    pub fn mid(&self) -> Option<f64> {
        let bid = self.best_bid.and_then(positive);
        let ask = self.best_ask.and_then(positive);
        match (bid, ask) {
            (Some(b), Some(a)) => Some((a + b) / 2.0),
            (None, Some(a)) => Some(a * (1.0 - FE_SPOT_ONE_SIDED)),
            (Some(b), None) => Some(b * (1.0 + FE_SPOT_ONE_SIDED)),
            (None, None) => None,
        }
    }

    /// **Pressure** in `[-1, 1]`: bid days against ask days of the regional
    /// daily volume; 0 on an empty book.
    pub fn pressure(&self, daily_volume: f64) -> f64 {
        let v = daily_volume.max(FE_VOL_FLOOR);
        let (b, a) = (self.bid_units as f64 / v, self.ask_units as f64 / v);
        if b + a <= 0.0 {
            0.0
        } else {
            ((b - a) / (b + a)).clamp(-1.0, 1.0)
        }
    }
}

/// **The perceived value `V`**: the weighted mean of whichever of the
/// book's mid, the EWMA and `averagePrice` exist (weights renormalised),
/// then kept within [`FE_MAX_DAY_MOVE`] of the EWMA (else of the index) when
/// there is one.
/// With no input, `p_ref` (cold start); `None` with nothing at all.
pub fn value_of(
    p_ref: Option<f64>,
    spot_mid: Option<f64>,
    ewma: Option<f64>,
    idx: Option<f64>,
) -> Option<f64> {
    let parts = [
        (spot_mid.and_then(positive), FE_W_SPOT),
        (ewma.and_then(positive), FE_W_EWMA),
        (idx.and_then(positive), FE_W_IDX),
    ];
    let (num, den) = parts
        .iter()
        .filter_map(|&(v, w)| v.map(|v| (v * w, w)))
        .fold((0.0, 0.0), |(n, d), (a, b)| (n + a, d + b));
    if den <= 0.0 {
        return p_ref.and_then(positive);
    }
    let v = num / den;
    // The slow anchor: the EWMA, else the server's index. With neither, V is
    // all there is.
    Some(match ewma.and_then(positive).or(idx.and_then(positive)) {
        Some(e) => v.clamp(e * (1.0 - FE_MAX_DAY_MOVE), e * (1.0 + FE_MAX_DAY_MOVE)),
        None => v,
    })
}

/// **The step cap**: `target`, but a rise over the order's `resting` price
/// is held to [`FE_MAX_STEP`] per repricing. A fall is never capped.
pub fn capped_rise(resting: Option<f64>, target: f64) -> f64 {
    match resting.and_then(positive) {
        Some(r) => target.min(down_to_cent(r * (1.0 + FE_MAX_STEP)).max(r)),
        None => target,
    }
}

/// **The stock target** in units: [`FE_PRODUCT_STOCK_DAYS`] of the
/// region's daily volume, between [`FE_STOCK_MIN`] and [`FE_STOCK_MAX`].
pub fn product_stock_target(daily_volume: f64) -> i64 {
    ((FE_PRODUCT_STOCK_DAYS * daily_volume.max(0.0)).ceil() as i64)
        .clamp(FE_STOCK_MIN, FE_STOCK_MAX)
}

/// The region's sales a day, over the window (zero with no history).
pub fn daily_volume(days: &[Day], today: i64) -> f64 {
    in_window(days, today).map(|d| d.volume as f64).sum::<f64>() / HIST_WINDOW as f64
}

/// **The clamp**: `[BOT_PRICE_MIN, BOT_PRICE_MAX] x p_ref`, to the cents
/// inside it. `None` with no usable `p_ref`.
pub fn band(p_ref: f64) -> Option<(f64, f64)> {
    let p = positive(p_ref)?;
    let lo = up_to_cent(BOT_PRICE_MIN * p).max(TICK);
    let hi = down_to_cent(BOT_PRICE_MAX * p);
    (lo <= hi).then_some((lo, hi))
}

/// The least a bot sells into a player's bid for: the clamp's low end.
pub fn floor(p_ref: f64) -> Option<f64> {
    band(p_ref).map(|(lo, _)| lo)
}

// ---- the seller --------------------------------------------------------------------

/// What a seller knows of one type when it prices an ask.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quote {
    pub p_ref: f64,
    /// [`p_mkt`]; `p_ref` when there is no history.
    pub p_mkt: f64,
    /// What one unit cost the seller (a builder's materials; zero for loot).
    pub cost: f64,
    /// An NPC ask here, which a bot never meets or passes.
    pub ceiling: Option<f64>,
    /// [`excess_steps`].
    pub steps: i64,
    /// The book's pressure in `[-1, 1]` ([`Spot::pressure`]); 0 when unknown.
    pub pressure: f64,
    /// The price of the seller's own resting ask on this type, if any: a
    /// rise over it is held to [`FE_MAX_STEP`].
    pub resting: Option<f64>,
}

impl Quote {
    /// A type with nothing known but `P_ref`.
    pub fn at_ref(p_ref: f64) -> Quote {
        Quote {
            p_ref,
            p_mkt: p_ref,
            cost: 0.0,
            ceiling: None,
            steps: 0,
            pressure: 0.0,
            resting: None,
        }
    }
}

/// **Days of excess**: the days an ask has rested, while `stock` (listed
/// and waiting) is more than [`TARGET_DAYS`] of the region's sales a day;
/// zero otherwise. Never falls as stock grows.
pub fn excess_steps(stock: i64, daily_volume: f64, days_listed: i64) -> i64 {
    if stock as f64 > TARGET_DAYS * daily_volume.max(0.0) {
        days_listed.max(0)
    } else {
        0
    }
}

/// **The seller's ask** (R-ECON §2.5): `V x (1 + FE_SCARCITY_K x pressure) x
/// (1 - UNDERCUT)` (`V` is `Quote::p_mkt`), less one
/// [`EXCESS_STEP`] per step, to the cent; never under `cost x (1 +
/// MARGIN)` nor outside the clamp, and strictly under an NPC ceiling.
/// `None` when no price meets all of them (nothing to list at a profit).
pub fn ask(q: &Quote) -> Option<f64> {
    let (lo, mut hi) = band(q.p_ref)?;
    let lo = lo.max(up_to_cent(q.cost.max(0.0) * (1.0 + MARGIN)));
    if let Some(c) = q.ceiling {
        hi = hi.min(up_to_cent(c) - TICK);
    }
    if lo > hi {
        return None;
    }
    let mkt = positive(q.p_mkt).unwrap_or(q.p_ref);
    let decay = (1.0 - EXCESS_STEP).powi(q.steps.clamp(0, 1_000) as i32);
    let scarcity = 1.0 + FE_SCARCITY_K * q.pressure.clamp(-1.0, 1.0);
    let anchor = up_to_cent(mkt * scarcity * (1.0 - UNDERCUT) * decay);
    let anchor = capped_rise(q.resting, anchor);
    Some(anchor.clamp(lo, hi))
}

// ---- the buyers --------------------------------------------------------------------

/// **The consumer's cap**: `RESERVATION x P_mkt`, and never past the
/// clamp's high end when `P_ref` is known. `None` with neither price (no
/// cap: the server quotes none).
pub fn reservation(p_ref: Option<f64>, p_mkt: Option<f64>) -> Option<f64> {
    let mkt = p_mkt.and_then(positive).or(p_ref.and_then(positive))?;
    let cap = RESERVATION * mkt;
    Some(match p_ref.and_then(band) {
        Some((_, hi)) => cap.min(hi),
        None => cap,
    })
}

/// The most a builder pays for a raw material from an ask:
/// [`BUY_CAP_OF_MKT`] x `V`, inside the clamp.
pub fn buy_cap(p_ref: f64, p_mkt: f64) -> Option<f64> {
    let (_, hi) = band(p_ref)?;
    Some((BUY_CAP_OF_MKT * positive(p_mkt).unwrap_or(p_ref)).min(hi))
}

/// What one run of `bp` may cost in materials under the margin rule: the
/// run's product at `product_mkt` (`P_mkt`), less the sales tax and the
/// broker's fee on selling it, kept [`MARGIN`] above the material cost.
/// For a pilot with no trade skills ([`material_budget_at`]).
pub fn material_budget(bp: &Blueprint, product_mkt: f64) -> f64 {
    material_budget_at(bp, product_mkt, Rates::UNSKILLED)
}

/// [`material_budget`] at a pilot's own [`Rates`]: a builder with
/// Accounting may pay more for its materials and keep the same margin.
pub fn material_budget_at(bp: &Blueprint, product_mkt: f64, rates: Rates) -> f64 {
    let net = rates.net_of_listing(product_mkt * bp.product_quantity as f64);
    (net / (1.0 + MARGIN)).max(0.0)
}

/// **The margin rule's bids** for `bp`'s materials (R-ECON §2.5's buyer):
/// each material's `P_mkt` (`mkt`) scaled by one factor, so that one run's
/// materials, bought at these prices plus the broker's fee on a bid, cost
/// at most [`material_budget`]; none above [`BUY_CAP_OF_MKT`] x its `P_mkt`
/// nor the clamp's high end on its `P_ref` (`refs`). Down to the cent.
/// `None` when a price is missing, or a bid would fall under the clamp's
/// low end: nothing to build at a profit.
pub fn margin_bids(
    bp: &Blueprint,
    mkt: &BTreeMap<i64, f64>,
    refs: &BTreeMap<i64, f64>,
) -> Option<BTreeMap<i64, f64>> {
    margin_bids_at(bp, mkt, refs, Rates::UNSKILLED)
}

/// [`margin_bids`] at a pilot's own [`Rates`].
pub fn margin_bids_at(
    bp: &Blueprint,
    mkt: &BTreeMap<i64, f64>,
    refs: &BTreeMap<i64, f64>,
    rates: Rates,
) -> Option<BTreeMap<i64, f64>> {
    let product = mkt.get(&bp.product_type_id).copied().and_then(positive)?;
    let budget = material_budget_at(bp, product, rates);
    let mut at_mkt = 0.0;
    for &(t, q) in &bp.materials {
        let p = mkt.get(&t).copied().and_then(positive)?;
        at_mkt += q as f64 * rates.cost_of_bid(p);
    }
    if !(at_mkt > 0.0 && budget > 0.0) {
        return None;
    }
    let k = (budget / at_mkt).min(BUY_CAP_OF_MKT);
    let mut out = BTreeMap::new();
    for &(t, _) in &bp.materials {
        let p_ref = refs.get(&t).copied().unwrap_or(mkt[&t]);
        let (lo, hi) = band(p_ref)?;
        let bid = down_to_cent(mkt[&t] * k).min(hi);
        if bid < lo {
            return None;
        }
        out.insert(t, bid);
    }
    Some(out)
}

/// One unit of `bp`'s product in materials at `mkt`, ME 0: the seller's
/// `cost`. Zero when a material has no price (the floor is then the clamp).
pub fn unit_cost(bp: &Blueprint, mkt: &BTreeMap<i64, f64>) -> f64 {
    let mut isk = 0.0;
    for &(t, q) in &bp.materials {
        let Some(p) = mkt.get(&t).copied().and_then(positive) else {
            return 0.0;
        };
        isk += q as f64 * p;
    }
    isk / bp.product_quantity.max(1) as f64
}

// ---- over the wire -----------------------------------------------------------------

/// **A cycle's prices**: the server's `P_ref`s, and each type's history
/// read once, when a rule first asks for its `P_mkt`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prices {
    refs: BTreeMap<i64, f64>,
    days: BTreeMap<i64, Vec<Day>>,
    /// `averagePrice` by type (`GetAverageMarketPrices`, free with `refs`).
    avg: BTreeMap<i64, f64>,
    /// Each type's live player book, read once ([`Prices::spot`]).
    spots: BTreeMap<i64, Option<Spot>>,
    /// The ids of the bot's own open orders, read once with the first book.
    own: Option<BTreeSet<i64>>,
    /// FILETIME the cycle prices at.
    now: i64,
}

impl Prices {
    /// The `P_ref`s at `now` (FILETIME), no history read yet.
    pub fn new(refs: BTreeMap<i64, f64>, now: i64) -> Prices {
        Prices {
            refs,
            days: BTreeMap::new(),
            avg: BTreeMap::new(),
            spots: BTreeMap::new(),
            own: None,
            now,
        }
    }

    /// `config.GetAverageMarketPrices` now: both `adjustedPrice` and
    /// `averagePrice`.
    pub fn read(conn: &mut Conn) -> Result<Prices> {
        let mut refs = BTreeMap::new();
        let mut avg = BTreeMap::new();
        for &(t, adjusted, average) in conn.price_index_shared()?.iter() {
            refs.insert(t, adjusted);
            if let Some(a) = average.and_then(positive) {
                avg.insert(t, a);
            }
        }
        let mut p = Prices::new(refs, eve_server::pyutil::filetime_now());
        p.avg = avg;
        Ok(p)
    }

    /// Put the server's `averagePrice` in (a test's fixture).
    pub fn with_average(mut self, type_id: i64, average: f64) -> Prices {
        self.avg.insert(type_id, average);
        self
    }

    /// Put a type's live book in (a test's fixture).
    pub fn with_spot(mut self, type_id: i64, spot: Option<Spot>) -> Prices {
        self.spots.insert(type_id, spot);
        self
    }

    /// The server's `averagePrice` of `type_id`.
    pub fn average(&self, type_id: i64) -> Option<f64> {
        self.avg.get(&type_id).copied()
    }

    /// **The live player book** of `type_id` in the pilot's region
    /// (`GetOrders`), read once per cycle. A refusal is no book.
    pub fn spot(&mut self, conn: &mut Conn, type_id: i64) -> Result<Option<Spot>> {
        if let Some(s) = self.spots.get(&type_id) {
            return Ok(*s);
        }
        if self.own.is_none() {
            let ids = match conn.my_orders() {
                Ok(o) => o.iter().map(|o| o.order_id).collect(),
                Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                Err(_) => BTreeSet::new(),
            };
            self.own = Some(ids);
        }
        let own = self.own.clone().unwrap_or_default();
        let book = match conn.market_orders(type_id) {
            Ok(b) => Some(b),
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => None,
        };
        let spot = match book {
            Some(b) => {
                // REVIEW-FIX: the band is read around the slow anchor (the
                // history EWMA, else the index, else `P_ref`), never the book.
                let now = self.now;
                let days = self.history(conn, type_id)?;
                let (ewma, volume) = (ewma_of(days, now), daily_volume(days, now));
                let anchor = value_of(self.p_ref(type_id), None, ewma, self.average(type_id));
                Some(Spot::banded(&b, &own, anchor, volume))
            }
            None => None,
        };
        self.spots.insert(type_id, spot);
        Ok(spot)
    }

    /// The `P_ref`s.
    pub fn refs(&self) -> &BTreeMap<i64, f64> {
        &self.refs
    }

    /// The `P_ref`s, to fill (`gas_price::fill`).
    pub fn refs_mut(&mut self) -> &mut BTreeMap<i64, f64> {
        &mut self.refs
    }

    pub fn p_ref(&self, type_id: i64) -> Option<f64> {
        self.refs.get(&type_id).copied()
    }

    /// Put a type's history in, as a read would (a test's fixture).
    pub fn with_history(mut self, type_id: i64, days: Vec<Day>) -> Prices {
        self.days.insert(type_id, days);
        self
    }

    fn history(&mut self, conn: &mut Conn, type_id: i64) -> Result<&[Day]> {
        match self.days.entry(type_id) {
            std::collections::btree_map::Entry::Occupied(e) => Ok(e.into_mut()),
            std::collections::btree_map::Entry::Vacant(e) => {
                let days = match conn.price_history(type_id) {
                    Ok(d) => d,
                    Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                    // A refusal is no history: `P_ref` it is.
                    Err(_) => Vec::new(),
                };
                Ok(e.insert(days))
            }
        }
    }

    /// **The perceived value `V`** of `type_id` ([`value_of`]): the live
    /// book, the history EWMA and `averagePrice`, each read once.
    pub fn value(&mut self, conn: &mut Conn, type_id: i64) -> Result<Option<f64>> {
        let p_ref = self.p_ref(type_id);
        let now = self.now;
        let ewma = ewma_of(self.history(conn, type_id)?, now);
        let spot = self.spot(conn, type_id)?.and_then(|s| s.mid());
        Ok(value_of(p_ref, spot, ewma, self.average(type_id)))
    }

    /// **`P_mkt`** of `type_id`: since FE-VALUE, the perceived value
    /// ([`Prices::value`]); the flat window average is the pure [`p_mkt`].
    pub fn p_mkt(&mut self, conn: &mut Conn, type_id: i64) -> Result<Option<f64>> {
        self.value(conn, type_id)
    }

    /// The book's pressure on `type_id` ([`Spot::pressure`]), 0 with no book.
    pub fn pressure(&mut self, conn: &mut Conn, type_id: i64) -> Result<f64> {
        let volume = self.daily_volume(conn, type_id)?;
        Ok(self
            .spot(conn, type_id)?
            .map_or(0.0, |s| s.pressure(volume)))
    }

    /// The region's sales a day of `type_id` ([`daily_volume`]).
    pub fn daily_volume(&mut self, conn: &mut Conn, type_id: i64) -> Result<f64> {
        let now = self.now;
        Ok(daily_volume(self.history(conn, type_id)?, now))
    }

    /// `P_mkt` for each of `types`, those with a price.
    pub fn mkt_of(&mut self, conn: &mut Conn, types: &[i64]) -> Result<BTreeMap<i64, f64>> {
        let mut out = BTreeMap::new();
        for &t in types {
            if let Some(p) = self.p_mkt(conn, t)? {
                out.insert(t, p);
            }
        }
        Ok(out)
    }

    /// **The price a listing is made at** for each of `types`: `V`
    /// ([`Prices::value`]), else `P_ref`; types with neither are left out.
    /// FE-VALUE: loot, gas, PI and explorer stacks list at the market's
    /// value, not at the static reference.
    pub fn values_of(&mut self, conn: &mut Conn, types: &[i64]) -> Result<BTreeMap<i64, f64>> {
        let mut out = BTreeMap::new();
        for &t in types {
            if let Some(v) = self.value(conn, t)?.or_else(|| self.p_ref(t)) {
                out.insert(t, v);
            }
        }
        Ok(out)
    }

    /// The consumer's cap for `type_id` ([`reservation`]).
    pub fn reservation(&mut self, conn: &mut Conn, type_id: i64) -> Result<Option<f64>> {
        let mkt = self.p_mkt(conn, type_id)?;
        Ok(reservation(self.p_ref(type_id), mkt))
    }

    /// The FILETIME the cycle prices at.
    pub fn now(&self) -> i64 {
        self.now
    }
}

impl Conn {
    /// **A type's history in the pilot's region, before today**:
    /// `marketProxy.GetOldPriceHistory(typeID)`, the call
    /// `marketsvc.GetPriceHistory` makes for the Market History tab
    /// (marketsvc.py:383) **[V]**. A `CRowset` of `(historyDate, lowPrice,
    /// highPrice, avgPrice, volume, orders)`; the prices are `DBTYPE_CY`.
    pub fn price_history(&mut self, type_id: i64) -> Result<Vec<Day>> {
        let outcome = self.call(
            "marketProxy",
            "GetOldPriceHistory",
            vec![PyValue::Int(type_id)],
        )?;
        match outcome.value() {
            Some(v) => Ok(history_rows(&v)),
            None => Err(Error::Refused(format!(
                "GetOldPriceHistory({type_id}) answered nothing ({:?})",
                outcome.refusal()
            ))),
        }
    }
}

/// Every history row anywhere inside `v` (a `CRowset`'s rows are a list in
/// its `ObjectEx` header); rows that do not read are skipped.
pub fn history_rows(v: &PyValue) -> Vec<Day> {
    let mut out = Vec::new();
    collect(v, &mut out);
    out.sort_by_key(|d| d.day);
    out
}

fn collect(v: &PyValue, out: &mut Vec<Day>) {
    match v {
        PyValue::DbRow(row) => {
            let get = |name: &str| {
                row.descriptor()
                    .columns()
                    .iter()
                    .position(|c| c.name == name.as_bytes())
                    .and_then(|i| row.values().get(i))
            };
            let int = |name: &str| match get(name) {
                Some(DbValue::Int(n)) => Some(*n),
                Some(DbValue::Real(r)) => Some(r.round() as i64),
                _ => None,
            };
            let cy = match get("avgPrice") {
                Some(DbValue::Int(n)) => Some(*n as f64 / 10_000.0),
                Some(DbValue::Real(r)) => Some(*r),
                _ => None,
            };
            if let (Some(day), Some(avg), Some(volume)) = (int("historyDate"), cy, int("volume")) {
                out.push(Day { day, avg, volume });
            }
        }
        PyValue::List(items) | PyValue::Tuple(items) => {
            for i in items {
                collect(i, out);
            }
        }
        PyValue::ObjectEx { header, .. } => collect(header, out),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// PERF-BURST: callers inside the TTL make one wire call (a counting
    /// fake), at once or one after another; after it a second is made; a
    /// failed fetch is not kept.
    #[test]
    fn the_price_index_is_fetched_once_per_ttl_whoever_asks() {
        use super::*;
        let cache = PriceIndexCache::default();
        let calls = AtomicUsize::new(0);
        let fake = |_: Instant| -> Result<PriceRows> {
            calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20));
            Ok(vec![(1_001, 5.0, Some(6.0))])
        };
        let t0 = Instant::now();
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    let rows = cache.get_or_fetch(t0, fake).expect("rows");
                    assert_eq!(rows[0].0, 1_001);
                });
            }
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1, "single flight");
        let within = t0 + PRICE_INDEX_TTL - Duration::from_secs(1);
        cache.get_or_fetch(within, fake).expect("rows");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "inside the TTL");
        let after = t0 + PRICE_INDEX_TTL;
        cache.get_or_fetch(after, fake).expect("rows");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "after the TTL");
        let failing = PriceIndexCache::default();
        assert!(
            failing
                .get_or_fetch(t0, |_| Err(Error::Refused("down".to_owned())))
                .is_err()
        );
        failing.get_or_fetch(t0, fake).expect("retried");
        assert_eq!(calls.load(Ordering::SeqCst), 3, "a failure is not kept");
    }

    use super::*;

    /// Today's midnight in the tests: an invented day.
    const TODAY: i64 = 160_000 * DAY_TICKS;

    fn day(back: i64, avg: f64, volume: i64) -> Day {
        Day {
            day: TODAY - back * DAY_TICKS,
            avg,
            volume,
        }
    }

    /// A small deterministic generator (no dependencies).
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
        /// A price over eight orders of magnitude.
        fn price(&mut self) -> f64 {
            10f64.powf(self.next() * 8.0 - 1.0)
        }
    }

    /// **The row's test (1)**: the ask is never under `cost x (1 +
    /// MARGIN)` nor outside the clamp, for any anchor, cost, ceiling and
    /// excess; `None` only when no price meets both.
    #[test]
    fn the_ask_never_goes_below_the_cost_floor_or_outside_the_clamp() {
        let mut r = Lcg(7);
        for _ in 0..20_000 {
            let p_ref = r.price();
            let q = Quote {
                p_ref,
                p_mkt: p_ref * r.next() * 4.0,
                cost: if r.next() < 0.3 {
                    0.0
                } else {
                    p_ref * r.next() * 2.5
                },
                ceiling: (r.next() < 0.3).then(|| p_ref * r.next() * 3.0),
                steps: (r.next() * 40.0) as i64,
                pressure: r.next() * 2.0 - 1.0,
                resting: (r.next() < 0.3).then(|| p_ref * r.next() * 3.0),
            };
            let (lo, hi) = band(p_ref).map_or((f64::INFINITY, 0.0), |b| b);
            match ask(&q) {
                Some(a) => {
                    assert!(
                        a + 1e-9 >= q.cost * (1.0 + MARGIN),
                        "{a} under the cost floor: {q:?}"
                    );
                    assert!(
                        a + 1e-9 >= p_ref * BOT_PRICE_MIN,
                        "{a} under the clamp: {q:?}"
                    );
                    assert!(
                        a <= p_ref * BOT_PRICE_MAX + 1e-9,
                        "{a} over the clamp: {q:?}"
                    );
                    assert!(q.ceiling.is_none_or(|c| a < c), "{a} at the ceiling: {q:?}");
                    assert!(a >= lo && a <= hi);
                }
                None => {
                    let floor = up_to_cent(q.cost * (1.0 + MARGIN)).max(lo);
                    let top = q.ceiling.map_or(hi, |c| hi.min(up_to_cent(c) - TICK));
                    assert!(floor > top, "an ask was possible: {q:?}");
                }
            }
        }
        assert_eq!(ask(&Quote::at_ref(0.0)), None);
        assert_eq!(ask(&Quote::at_ref(f64::NAN)), None);
    }

    /// **The row's test (2)**: more stock never raises the ask, nor do more
    /// days of excess; under the target it is the undercut anchor.
    #[test]
    fn excess_stock_lowers_the_ask_monotonically() {
        let mut r = Lcg(11);
        for _ in 0..2_000 {
            let p_ref = r.price();
            let daily = r.next() * 1_000.0;
            let days_listed = (r.next() * 30.0) as i64;
            let base = Quote {
                p_ref,
                p_mkt: p_ref * (0.6 + r.next()),
                cost: p_ref * r.next() * 0.5,
                ceiling: None,
                steps: 0,
                pressure: 0.0,
                resting: None,
            };
            let mut last = f64::INFINITY;
            for stock in (0..60).map(|i| i * 250) {
                let q = Quote {
                    steps: excess_steps(stock, daily, days_listed),
                    ..base
                };
                let a = ask(&q).expect("an ask inside a wide band");
                assert!(
                    a <= last + 1e-9,
                    "stock {stock} raised the ask {last} -> {a}"
                );
                last = a;
            }
            let mut last = f64::INFINITY;
            for days in 0..40 {
                let a = ask(&Quote {
                    steps: excess_steps(i64::MAX / 2, daily, days),
                    ..base
                })
                .expect("an ask");
                assert!(a <= last + 1e-9, "day {days} raised the ask");
                last = a;
            }
        }
        // Stock within the target: no step, the undercut anchor.
        assert_eq!(excess_steps(70, 10.0, 9), 0);
        assert_eq!(excess_steps(71, 10.0, 9), 9);
        let q = Quote::at_ref(100.0);
        assert_eq!(ask(&q), Some(up_to_cent(100.0 * (1.0 - UNDERCUT))));
        let stepped = ask(&Quote { steps: 3, ..q }).expect("an ask");
        assert!(stepped < ask(&q).expect("an ask"));
    }

    /// **The row's test (3)**: `P_mkt` is `P_ref` until `MIN_HIST_DAYS`
    /// days in the window traded, then their volume-weighted average;
    /// today, days past the window and empty days do not count.
    #[test]
    fn p_mkt_comes_from_history_once_min_hist_days_is_reached_else_p_ref() {
        let p_ref = 100.0;
        assert_eq!(p_mkt(Some(p_ref), &[], TODAY), Some(p_ref));
        assert_eq!(p_mkt(None, &[], TODAY), None);
        let mut days: Vec<Day> = (1..MIN_HIST_DAYS as i64)
            .map(|i| day(i, 150.0, 10))
            .collect();
        // Not counted: today, beyond the window, no volume.
        days.push(day(0, 999.0, 1_000));
        days.push(day(HIST_WINDOW + 1, 999.0, 1_000));
        days.push(day(HIST_WINDOW.min(MIN_HIST_DAYS as i64 + 1), 999.0, 0));
        assert_eq!(
            p_mkt(Some(p_ref), &days, TODAY + 5),
            Some(p_ref),
            "short of MIN_HIST_DAYS"
        );
        // One more traded day: the volume-weighted average.
        days.push(day(MIN_HIST_DAYS as i64, 50.0, 30));
        let n = (MIN_HIST_DAYS - 1) as f64;
        let want = (150.0 * 10.0 * n + 50.0 * 30.0) / (10.0 * n + 30.0);
        let got = p_mkt(Some(p_ref), &days, TODAY + 5).expect("a price");
        assert!((got - want).abs() < 1e-9, "{got} != {want}");
        // History without a P_ref still prices.
        assert!(p_mkt(None, &days, TODAY).is_some());
        assert!((daily_volume(&days, TODAY) - (10.0 * n + 30.0) / HIST_WINDOW as f64).abs() < 1e-9);
    }

    /// **The row's test (4)**: whatever `P_mkt` is, the consumer's cap is at
    /// most `RESERVATION x P_mkt` (and inside the clamp), so no planned buy
    /// pays more.
    #[test]
    fn the_consumer_never_pays_above_reservation_times_p_mkt() {
        let mut r = Lcg(3);
        for _ in 0..5_000 {
            let p_ref = r.price();
            let mkt = p_ref * r.next() * 5.0;
            let cap = reservation(Some(p_ref), Some(mkt)).expect("a cap");
            let bound = RESERVATION * positive(mkt).unwrap_or(p_ref);
            assert!(cap <= bound + 1e-9, "{cap} over {bound}");
            assert!(cap <= p_ref * BOT_PRICE_MAX + 1e-9);
            // The buy plan honours it: asks from half to twice the cap.
            let book = crate::verbs::Book {
                asks: (1..=8)
                    .map(|i| crate::verbs::BookOrder {
                        order_id: i,
                        type_id: 990_001,
                        station_id: 69_000_001,
                        price: cap * (0.25 * i as f64),
                        vol_remaining: 3,
                        bid: false,
                    })
                    .collect(),
                bids: vec![],
            };
            for (a, _) in crate::acquire::plan(&book, 69_000_001, Some(cap), f64::MAX, 100) {
                assert!(a.price <= bound + 1e-9);
            }
        }
        assert_eq!(reservation(None, None), None);
        assert_eq!(reservation(None, Some(10.0)), Some(10.0 * RESERVATION));
    }

    /// The margin rule on `P_mkt`: the run's bids cost at most the budget,
    /// none over the cap or outside the clamp.
    #[test]
    fn the_margin_bids_follow_p_mkt_inside_the_clamp() {
        let bp = Blueprint {
            blueprint_type_id: 990_100,
            product_type_id: 990_200,
            product_quantity: 10,
            time: 1,
            max_production_limit: 1,
            materials: vec![(990_301, 50), (990_302, 5)],
            skills: vec![],
        };
        let refs = BTreeMap::from([(990_200, 100.0), (990_301, 4.0), (990_302, 10.0)]);
        let at_ref = margin_bids(&bp, &refs, &refs).expect("a margin at P_ref");
        // The product's market falls: the bids fall with it.
        let mut mkt = refs.clone();
        mkt.insert(990_200, 40.0);
        let low = margin_bids(&bp, &mkt, &refs).expect("still a margin");
        for (t, b) in &low {
            assert!(*b <= at_ref[t]);
            assert!(*b >= refs[t] * BOT_PRICE_MIN - 1e-9);
        }
        let cost: f64 = bp
            .materials
            .iter()
            .map(|&(t, q)| q as f64 * low[&t] * (1.0 + BROKER_FEE))
            .sum();
        assert!(cost <= material_budget(&bp, 40.0) + 1e-6);
        // Too low to keep the margin inside the clamp: no bids.
        mkt.insert(990_200, 5.0);
        assert_eq!(margin_bids(&bp, &mkt, &refs), None);
        assert!((unit_cost(&bp, &refs) - 25.0).abs() < 1e-9);
    }

    /// **X-TRADE**: the unskilled constants are the server's own numbers
    /// (`market_fees`), and skills move the rates the way the client's
    /// formulas do.
    #[test]
    fn the_rates_are_the_servers_and_skills_lower_them() {
        let none = TradeSkills::default();
        let r0 = Rates::at(&none, Standings::default());
        assert_eq!(r0, Rates::UNSKILLED);
        assert!((SALES_TAX - eve_server::market_fees::sales_tax_rate(0)).abs() < 1e-12);
        assert!(
            (BROKER_FEE - eve_server::market_fees::broker_rate(0, Standings::default())).abs()
                < 1e-12
        );
        let skilled = TradeSkills {
            accounting: 4,
            broker_relations: 4,
            ..none
        };
        let r4 = Rates::at(&skilled, Standings::default());
        assert!(r4.sales_tax < r0.sales_tax && r4.broker_fee < r0.broker_fee);
        assert!((r4.sales_tax - 0.08 * (1.0 - 0.11 * 4.0)).abs() < 1e-12);
        assert!((r4.broker_fee - (0.03 - 0.003 * 4.0)).abs() < 1e-12);
        assert!(r4.net_of_sale(100.0) > r0.net_of_sale(100.0));
        assert!(r4.net_of_listing(100.0) > r0.net_of_listing(100.0));
        assert!(r4.cost_of_bid(100.0) < r0.cost_of_bid(100.0));
        // A liked station owner lowers the commission further.
        let liked = Rates::at(
            &skilled,
            Standings {
                faction: 5.0,
                corp: 5.0,
            },
        );
        assert!(liked.broker_fee < r4.broker_fee);
        assert_eq!(
            liked.sales_tax, r4.sales_tax,
            "standings never touch the tax"
        );
    }

    /// **X-TRADE**: a builder with Accounting and Broker Relations may bid
    /// more for the same materials and keep its margin; the unskilled rule
    /// is [`margin_bids`] as it was.
    #[test]
    fn skills_raise_the_margin_rules_budget() {
        let bp = Blueprint {
            blueprint_type_id: 990_199,
            product_type_id: 990_200,
            product_quantity: 1,
            time: 1,
            max_production_limit: 1,
            materials: vec![(990_301, 50), (990_302, 5)],
            skills: vec![],
        };
        let skilled = Rates::at(
            &TradeSkills {
                accounting: 5,
                broker_relations: 5,
                ..TradeSkills::default()
            },
            Standings::default(),
        );
        assert_eq!(
            material_budget(&bp, 1_000.0),
            material_budget_at(&bp, 1_000.0, Rates::UNSKILLED)
        );
        assert!(material_budget_at(&bp, 1_000.0, skilled) > material_budget(&bp, 1_000.0));
        let mkt = BTreeMap::from([(990_200, 1_000.0), (990_301, 4.0), (990_302, 10.0)]);
        assert_eq!(
            margin_bids(&bp, &mkt, &mkt),
            margin_bids_at(&bp, &mkt, &mkt, Rates::UNSKILLED)
        );
        let (plain, rich) = (
            margin_bids(&bp, &mkt, &mkt).expect("bids"),
            margin_bids_at(&bp, &mkt, &mkt, skilled).expect("bids"),
        );
        assert!(rich[&990_301] >= plain[&990_301]);
    }

    /// A history rowset reads by column name, `avgPrice` in CY.
    #[test]
    fn a_history_rowset_reads_by_column_name() {
        use eve_marshal::{Column, DbRow, DbRowDescriptor, DbType};
        let descriptor = DbRowDescriptor::new(
            [
                ("historyDate", DbType::FileTime),
                ("lowPrice", DbType::Cy),
                ("highPrice", DbType::Cy),
                ("avgPrice", DbType::Cy),
                ("volume", DbType::I8),
                ("orders", DbType::I4),
            ]
            .iter()
            .map(|&(n, t)| Column {
                name: n.as_bytes().to_vec(),
                db_type: t,
            })
            .collect(),
        )
        .expect("a layout");
        let row = |d: i64, avg: f64, vol: i64| {
            PyValue::DbRow(Box::new(
                DbRow::new(
                    descriptor.clone(),
                    vec![
                        DbValue::Int(d),
                        DbValue::Int(0),
                        DbValue::Int(0),
                        DbValue::Int((avg * 10_000.0) as i64),
                        DbValue::Int(vol),
                        DbValue::Int(1),
                    ],
                )
                .expect("a row"),
            ))
        };
        let v = PyValue::ObjectEx {
            form: eve_marshal::ObjectForm::Call,
            header: Box::new(PyValue::Tuple(vec![
                PyValue::None,
                PyValue::List(vec![
                    row(TODAY - DAY_TICKS, 12.5, 7),
                    row(TODAY - 2 * DAY_TICKS, 3.0, 2),
                ]),
            ])),
            list: Vec::new(),
            dict: Vec::new(),
        };
        assert_eq!(history_rows(&v), vec![day(2, 3.0, 2), day(1, 12.5, 7)]);
    }

    // ---- FE-VALUE (R-FLUIDECON §3.1-3.4) ------------------------------------------
    // Invented ids and prices; the weights and caps are named, never typed.

    fn spot(bid: Option<f64>, ask: Option<f64>, bu: i64, au: i64) -> Spot {
        Spot {
            best_bid: bid,
            best_ask: ask,
            bid_units: bu,
            ask_units: au,
        }
    }

    #[test]
    fn value_uses_spot_when_book_has_both_sides() {
        let s = spot(Some(90.0), Some(130.0), 5, 5);
        assert_eq!(s.mid(), Some(110.0));
        let v = value_of(Some(100.0), s.mid(), Some(100.0), Some(100.0)).expect("a value");
        let want = (110.0 * FE_W_SPOT + 100.0 * FE_W_EWMA + 100.0 * FE_W_IDX)
            / (FE_W_SPOT + FE_W_EWMA + FE_W_IDX);
        assert!((v - want).abs() < 1e-9 && v > 100.0, "{v} != {want}");
        // One side alone is moved by the one-sided spread.
        assert_eq!(
            spot(None, Some(100.0), 0, 3).mid(),
            Some(100.0 * (1.0 - FE_SPOT_ONE_SIDED))
        );
        assert_eq!(
            spot(Some(100.0), None, 3, 0).mid(),
            Some(100.0 * (1.0 + FE_SPOT_ONE_SIDED))
        );
        assert_eq!(Spot::default().mid(), None);
    }

    #[test]
    fn value_falls_back_to_pref_with_no_input() {
        assert_eq!(value_of(Some(100.0), None, None, None), Some(100.0));
        assert_eq!(value_of(None, None, None, None), None);
        // Weights renormalise over what is present: the index alone is V.
        assert_eq!(value_of(Some(100.0), None, None, Some(140.0)), Some(140.0));
        assert_eq!(
            value_of(Some(100.0), None, Some(120.0), Some(140.0)),
            Some((120.0 * FE_W_EWMA + 140.0 * FE_W_IDX) / (FE_W_EWMA + FE_W_IDX))
        );
    }

    #[test]
    fn one_traded_day_is_enough_for_ewma() {
        assert_eq!(ewma_of(&[], TODAY), None);
        assert_eq!(ewma_of(&[day(1, 80.0, 3)], TODAY), Some(80.0));
        // Today and a day past the window do not count.
        assert_eq!(
            ewma_of(&[day(0, 80.0, 3), day(FE_EWMA_WINDOW + 1, 70.0, 3)], TODAY),
            None
        );
        // The flat average needs MIN_HIST_DAYS (now one) too.
        assert_eq!(p_mkt(Some(100.0), &[day(1, 80.0, 3)], TODAY), Some(80.0));
        // Two days: the newer moves the average ALPHA of the gap.
        let e = ewma_of(&[day(2, 100.0, 5), day(1, 120.0, 5)], TODAY).expect("ewma");
        assert!((e - (100.0 + FE_EWMA_ALPHA * 20.0)).abs() < 1e-9, "{e}");
    }

    #[test]
    fn memory_decays_with_age() {
        // The same jump seen long ago has been absorbed: the older the
        // shock, the closer the EWMA to the new level.
        let step = |age: i64| {
            let days: Vec<Day> = (age..=6)
                .map(|b| day(b, if b > age { 100.0 } else { 150.0 }, 5))
                .collect();
            let mut days = days;
            for b in 1..age {
                days.push(day(b, 150.0, 5));
            }
            ewma_of(&days, TODAY).expect("ewma")
        };
        assert!(
            step(1) < step(3) && step(3) < step(5),
            "{} {} {}",
            step(1),
            step(3),
            step(5)
        );
        assert!(step(5) < 150.0 + 1e-9);
    }

    #[test]
    fn a_wash_day_is_held_back_until_a_second_day_agrees() {
        let calm = [day(4, 100.0, 5), day(3, 100.0, 5), day(2, 100.0, 5)];
        let mut one = calm.to_vec();
        one.push(day(1, 100.0 * FE_OUTLIER_BAND * 2.0, 1));
        assert_eq!(ewma_of(&one, TODAY), Some(100.0), "a lone odd day waits");
        let mut two = calm[..2].to_vec();
        two.push(day(2, 100.0 * FE_OUTLIER_BAND * 2.0, 5));
        two.push(day(1, 100.0 * FE_OUTLIER_BAND * 2.0, 5));
        assert!(
            ewma_of(&two, TODAY).expect("ewma") > 100.0,
            "two agreeing days count"
        );
    }

    #[test]
    fn ask_marks_up_with_bid_pressure_within_band() {
        let p_ref = 1_000.0;
        let flat = ask(&Quote::at_ref(p_ref)).expect("ask");
        let hot = ask(&Quote {
            pressure: 1.0,
            ..Quote::at_ref(p_ref)
        })
        .expect("ask");
        let cold = ask(&Quote {
            pressure: -1.0,
            ..Quote::at_ref(p_ref)
        })
        .expect("ask");
        assert!(cold < flat && flat < hot);
        assert_eq!(
            hot,
            up_to_cent(p_ref * (1.0 + FE_SCARCITY_K) * (1.0 - UNDERCUT))
        );
        // Pressure never lifts an ask past the band's top, nor under the cost floor.
        let (_, top) = band(p_ref).expect("band");
        let huge = ask(&Quote {
            p_mkt: p_ref * 100.0,
            pressure: 1.0,
            ..Quote::at_ref(p_ref)
        })
        .expect("ask");
        assert_eq!(huge, top);
        let costly = ask(&Quote {
            cost: p_ref,
            pressure: -1.0,
            ..Quote::at_ref(p_ref)
        })
        .expect("ask");
        assert!(costly >= up_to_cent(p_ref * (1.0 + MARGIN)));
        // The pressure itself: bid days against ask days.
        assert_eq!(spot(Some(1.0), Some(2.0), 10, 0).pressure(5.0), 1.0);
        assert_eq!(spot(Some(1.0), Some(2.0), 0, 10).pressure(5.0), -1.0);
        assert_eq!(spot(None, None, 0, 0).pressure(5.0), 0.0);
        assert_eq!(spot(Some(1.0), Some(2.0), 5, 5).pressure(5.0), 0.0);
    }

    #[test]
    fn step_and_day_move_are_capped() {
        // Per repricing: a rise over the resting price is held to FE_MAX_STEP.
        let resting = 1_000.0;
        let up = capped_rise(Some(resting), 5_000.0);
        assert_eq!(up, down_to_cent(resting * (1.0 + FE_MAX_STEP)));
        assert_eq!(
            capped_rise(Some(resting), 400.0),
            400.0,
            "a fall is not capped"
        );
        assert_eq!(capped_rise(None, 5_000.0), 5_000.0);
        let q = Quote {
            resting: Some(resting),
            pressure: 1.0,
            p_mkt: 5_000.0,
            ..Quote::at_ref(1_000.0)
        };
        assert!(ask(&q).expect("ask") <= resting * (1.0 + FE_MAX_STEP) + 1e-9);
        // Per day: whatever the book says, V stays within FE_MAX_DAY_MOVE of the EWMA.
        let v = value_of(Some(100.0), Some(10_000.0), Some(100.0), Some(100.0)).expect("v");
        assert!((v - 100.0 * (1.0 + FE_MAX_DAY_MOVE)).abs() < 1e-9, "{v}");
        let v = value_of(Some(100.0), Some(1.0), Some(100.0), Some(100.0)).expect("v");
        assert!((v - 100.0 * (1.0 - FE_MAX_DAY_MOVE)).abs() < 1e-9, "{v}");
        // The ratchet is bounded: seller markup, then buyer reservation, from the EWMA.
        let ceiling = 100.0 * (1.0 + FE_MAX_DAY_MOVE);
        assert!(
            reservation(Some(100.0), Some(v.max(ceiling))).expect("cap")
                <= RESERVATION * ceiling + 1e-9
        );
    }

    #[test]
    fn listing_prices_at_value_not_pref() {
        // A traded market at twice P_ref: the listing is made near V.
        let p_ref = 100.0;
        let v = value_of(Some(p_ref), None, Some(180.0), Some(160.0)).expect("v");
        let at_value = ask(&Quote {
            p_mkt: v,
            ..Quote::at_ref(p_ref)
        })
        .expect("ask");
        let at_ref = ask(&Quote::at_ref(p_ref)).expect("ask");
        assert!(at_value > 1.5 * at_ref, "{at_value} vs {at_ref}");
        // The old clamp (2x) would have bound at 200; the new band passes 4x.
        assert_eq!(band(p_ref), Some((30.0, 400.0)));
        assert_eq!((FE_BAND_MIN, FE_BAND_MAX), (BOT_PRICE_MIN, BOT_PRICE_MAX));
    }

    #[test]
    fn stock_target_follows_regional_volume() {
        assert_eq!(product_stock_target(0.0), FE_STOCK_MIN);
        assert_eq!(product_stock_target(1_000_000.0), FE_STOCK_MAX);
        assert_eq!(
            product_stock_target(4.0),
            (4.0 * FE_PRODUCT_STOCK_DAYS) as i64
        );
    }

    #[test]
    fn a_bots_own_orders_do_not_move_its_spot() {
        use crate::verbs::{Book, BookOrder};
        let order = |id: i64, price: f64, bid: bool| BookOrder {
            order_id: id,
            type_id: 1,
            station_id: 2,
            price,
            vol_remaining: 4,
            bid,
        };
        let book = Book {
            asks: vec![order(1, 120.0, false), order(2, 150.0, false)],
            bids: vec![order(3, 100.0, true)],
        };
        let all = Spot::of(&book);
        assert_eq!((all.best_ask, all.ask_units), (Some(120.0), 8));
        let own = BTreeSet::from([1]);
        let rest = Spot::without(&book, &own);
        assert_eq!((rest.best_ask, rest.ask_units), (Some(150.0), 4));
        assert_eq!(rest.best_bid, Some(100.0));
    }

    #[test]
    fn review_fix_a_far_bid_and_a_huge_order_do_not_make_pressure() {
        use crate::verbs::{Book, BookOrder};
        let order = |id: i64, price: f64, units: i64, bid: bool| BookOrder {
            order_id: id,
            type_id: 1,
            station_id: 2,
            price,
            vol_remaining: units,
            bid,
        };
        let anchor = 100.0;
        let volume = FE_SPOT_ORDER_FLOOR * 3.0;
        let book = Book {
            asks: vec![
                order(1, 105.0, 5, false),
                order(2, anchor * FE_BAND_MAX, 5, false),
            ],
            bids: vec![
                // The attacker: absurd units at the floor price, then a huge
                // order inside the band.
                order(3, anchor * FE_SPOT_BAND_LO / 2.0, 1_000_000_000, true),
                order(4, 95.0, 1_000_000_000, true),
                order(5, 90.0, 2, true),
            ],
        };
        let s = Spot::banded(&book, &BTreeSet::new(), Some(anchor), volume);
        assert_eq!(s.best_bid, Some(95.0), "the far bid is not the best bid");
        assert_eq!(s.best_ask, Some(105.0), "the far ask is not the best ask");
        let cap = (FE_SPOT_ORDER_DAYS * volume).ceil() as i64;
        assert_eq!(s.bid_units, cap + 2, "one order weighs at most the cap");
        assert_eq!(s.ask_units, 5);
        assert!(s.pressure(volume).abs() < 1.0);
        // No anchor: only the per-order cap applies.
        let s = Spot::banded(&book, &BTreeSet::new(), None, volume);
        assert_eq!(s.bid_units, 2 * cap + 2);
    }
}
