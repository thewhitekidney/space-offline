//! **Speculation and regional arbitrage rules** (FE-TRADE, R-FLUIDECON §3.6,
//! §5 G2): the pure decisions, so a test can pin each one. The flying and the
//! buying are [`crate::trader`]'s; the memory is [`crate::price_memory`]'s.
//!
//! - **Speculation** (a trader's second string, same role): a trader *holds*
//!   a type when its perceived value `V` sits [`SpecKnobs::entry`] under the
//!   bot's own slow anchor and is not falling ([`wants_entry`]), buying the
//!   asks at its own station no dearer than `V`; it *sells* when `V` has
//!   climbed [`SpecKnobs::exit`] over the anchor, or after
//!   [`SpecKnobs::max_days`] whatever the price ([`exit_why`]). Buying into a
//!   glut and selling into scarcity damps both.
//! - **Caps** ([`entry_units`]), so no loop runs away (G2): the holdings at
//!   cost stay within [`SpecKnobs::wallet_share`] of the trader's worth
//!   (wallet plus holdings), a type's holding within
//!   [`SpecKnobs::share_days`] days of the region's daily volume (no
//!   corner), at most [`SpecKnobs::max_positions`] types at once, and a type
//!   sold out of a holding is not bought again for
//!   [`SpecKnobs::cool_days`] (a speculator does not buy its own listing
//!   back).
//! - **Regional arbitrage** ([`confirm`]): a trader plans on a *remembered*
//!   bid in another region ([`crate::price_memory`]) with a risk margin on
//!   top of its usual one, and on arrival checks the live bid: below the
//!   cost plus the margin it does not sell there.
//!
//! **Every number is [A]** with its falsifier below.

use crate::price_memory::{Anchor, Hold};

/// Days of observation before the anchor is trusted **[A]**. Falsified by a
/// speculator that buys on its first sight of a type.
pub const MIN_ANCHOR_DAYS: u32 = 3;

/// A forced exit lists this fraction under `V` **[A]**: it has waited long
/// enough. Falsified by exits that sit unsold for days.
pub const AGED_DISCOUNT: f64 = 0.05;

/// A trader's speculative and cross-region appetite, by difficulty.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpecKnobs {
    /// The most of its worth the holdings may cost (`FE_SPEC_WALLET`).
    /// 0 turns speculation off. **[A]** Falsified by a speculator whose
    /// ledger ends 40 % under its start (lower), or whose holdings never
    /// move the tape's `spec` row (raise).
    pub wallet_share: f64,
    /// A type's holding in days of the region's daily volume
    /// (`FE_SPEC_SHARE`). **[A]** Falsified by a speculator that holds the
    /// whole ask side of a type for a day (lower).
    pub share_days: f64,
    /// How far under its anchor `V` must sit to buy (`FE_SPEC_ENTRY`).
    /// **[A]**
    pub entry: f64,
    /// How far over its anchor `V` must climb to sell (`FE_SPEC_EXIT`).
    /// **[A]**
    pub exit: f64,
    /// The longest a holding is kept, days (`FE_SPEC_MAX_DAYS`). **[A]**
    pub max_days: i64,
    /// Days a sold-out type waits before it is bought again. **[A]**
    pub cool_days: i64,
    /// The most types held at once. **[A]**
    pub max_positions: usize,
    /// What a cross-region deal must clear over the usual margin, as a
    /// fraction of its spend: the memory may be stale and the book may move
    /// while the goods ride. **[A]** Falsified by arbitrage legs that abort
    /// on arrival more than a third of the time (raise), or a regional
    /// spread over twice the round-trip cost that nobody closes (lower).
    pub region_risk: f64,
    /// The longest cross-region trip, jumps out, across and home. **[A]**
    pub region_max_jumps: u32,
}

impl SpecKnobs {
    pub const NORMAL: SpecKnobs = SpecKnobs {
        wallet_share: 0.30,
        share_days: 3.0,
        entry: 0.10,
        exit: 0.10,
        max_days: 14,
        cool_days: 3,
        max_positions: 3,
        region_risk: 0.03,
        region_max_jumps: 16,
    };
    /// The knobs a difficulty level varies; the rest are [`NORMAL`].
    ///
    /// [`NORMAL`]: SpecKnobs::NORMAL
    pub fn with(wallet_share: f64, share_days: f64, region_risk: f64) -> SpecKnobs {
        SpecKnobs {
            wallet_share,
            share_days,
            region_risk,
            ..SpecKnobs::NORMAL
        }
    }

    /// Speculation is off.
    pub fn off(self) -> SpecKnobs {
        SpecKnobs {
            wallet_share: 0.0,
            ..self
        }
    }
}

impl Default for SpecKnobs {
    fn default() -> SpecKnobs {
        SpecKnobs::NORMAL
    }
}

/// **Buy now?** `v` is under the anchor by [`SpecKnobs::entry`], the anchor
/// has [`MIN_ANCHOR_DAYS`] of sight, and `v` is not falling day on day.
pub fn wants_entry(k: &SpecKnobs, a: &Anchor, v: f64) -> bool {
    k.wallet_share > 0.0
        && a.n >= MIN_ANCHOR_DAYS
        && v > 0.0
        && v < a.ewma * (1.0 - k.entry)
        && a.trend_up()
}

/// Why a holding is sold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// `V` climbed over the anchor.
    Gain,
    /// [`SpecKnobs::max_days`] passed.
    Aged,
}

/// **Sell now?** (pure): `Gain` when `v` is at least [`SpecKnobs::exit`] over
/// the anchor, `Aged` after [`SpecKnobs::max_days`] at any price.
pub fn exit_why(
    k: &SpecKnobs,
    hold: &Hold,
    anchor: Option<&Anchor>,
    v: f64,
    day: i64,
) -> Option<Exit> {
    if anchor.is_some_and(|a| v > 0.0 && v >= a.ewma * (1.0 + k.exit)) {
        return Some(Exit::Gain);
    }
    (day - hold.day >= k.max_days).then_some(Exit::Aged)
}

/// The listing price of an exit.
pub fn exit_price(why: Exit, v: f64) -> f64 {
    match why {
        Exit::Gain => v,
        Exit::Aged => v * (1.0 - AGED_DISCOUNT),
    }
}

/// **How many units** to buy at `price` (pure): the least of the money left
/// under [`SpecKnobs::wallet_share`] of `worth` (wallet plus holdings at
/// cost), the room under [`SpecKnobs::share_days`] of `daily_volume` less
/// what is `held_units`, and `ask_units`; 0 when [`SpecKnobs::max_positions`]
/// are already held (`positions`, counting this type if held).
#[allow(clippy::too_many_arguments)]
pub fn entry_units(
    k: &SpecKnobs,
    worth: f64,
    held_cost: f64,
    held_units: i64,
    positions: usize,
    daily_volume: f64,
    price: f64,
    ask_units: i64,
) -> i64 {
    if k.wallet_share <= 0.0 || price <= 0.0 || positions >= k.max_positions {
        return 0;
    }
    let money = (k.wallet_share * worth - held_cost).max(0.0);
    let by_money = (money / price).floor();
    let room = (k.share_days * daily_volume).floor() - held_units as f64;
    let n = by_money.min(room).min(ask_units as f64);
    if n.is_finite() && n >= 1.0 {
        n as i64
    } else {
        0
    }
}

/// **Confirm a remembered bid on arrival** (pure): the live best bid pays
/// the unit cost plus `margin`, after the sales tax. Below that the leg is
/// aborted and the goods go home.
pub fn confirm(live_bid: Option<f64>, unit_cost: f64, margin: f64, sales_tax: f64) -> bool {
    live_bid.is_some_and(|b| b * (1.0 - sales_tax) >= unit_cost * (1.0 + margin))
}

/// The lowest price a confirmed leg sells at: the unit cost plus `margin`,
/// before tax.
pub fn confirm_floor(unit_cost: f64, margin: f64, sales_tax: f64) -> f64 {
    unit_cost * (1.0 + margin) / (1.0 - sales_tax).max(f64::EPSILON)
}

/// A tape line for the speculator's holdings in `region`: `spec`, the
/// region, units, ISK at cost (`R-FLUIDECON` §4, the `spec` row's columns;
/// the server's tape does not know who speculates, so the bots print it).
pub fn spec_row(region: i64, units: i64, isk: f64) -> String {
    format!("spec\t{region}\t{units}\t{isk:.0}")
}

#[cfg(test)]
mod tests {
    //! Invented numbers; the knobs are spelled out where a test leans on one.
    use super::*;

    const K: SpecKnobs = SpecKnobs::NORMAL;

    fn anchor(ewma: f64, v: f64, prev: f64, n: u32) -> Anchor {
        Anchor {
            ewma,
            v,
            prev,
            day: 10,
            n,
        }
    }

    fn hold(day: i64) -> Hold {
        Hold {
            units: 10,
            cost: 900.0,
            day,
            station: 1,
            region: 2,
        }
    }

    #[test]
    fn speculator_buys_below_anchor_and_sells_above() {
        let a = anchor(100.0, 85.0, 84.0, 10);
        assert!(
            wants_entry(&K, &a, 85.0),
            "under the anchor and not falling"
        );
        assert!(!wants_entry(&K, &a, 100.0), "at the anchor: no");
        assert!(
            !wants_entry(&K, &anchor(100.0, 85.0, 90.0, 10), 85.0),
            "falling: no"
        );
        assert!(
            !wants_entry(&K, &anchor(100.0, 85.0, 84.0, 1), 85.0),
            "no sight yet"
        );
        let h = hold(5);
        let up = anchor(100.0, 115.0, 110.0, 12);
        assert_eq!(exit_why(&K, &h, Some(&up), 115.0, 8), Some(Exit::Gain));
        assert_eq!(exit_why(&K, &h, Some(&up), 100.0, 8), None);
        assert_eq!(exit_price(Exit::Gain, 115.0), 115.0);
        assert!(exit_price(Exit::Aged, 100.0) < 100.0);
    }

    #[test]
    fn a_holding_is_sold_after_its_longest_stay_at_any_price() {
        let h = hold(0);
        let down = anchor(100.0, 70.0, 70.0, 12);
        assert_eq!(exit_why(&K, &h, Some(&down), 70.0, K.max_days - 1), None);
        assert_eq!(
            exit_why(&K, &h, Some(&down), 70.0, K.max_days),
            Some(Exit::Aged)
        );
        assert_eq!(exit_why(&K, &h, None, 70.0, K.max_days), Some(Exit::Aged));
    }

    #[test]
    fn speculation_respects_share_cap() {
        // Plenty of money, 20 units a day: three days is 60, less 25 held.
        let n = entry_units(&K, 1e9, 0.0, 25, 1, 20.0, 10.0, 10_000);
        assert_eq!(n, (K.share_days * 20.0) as i64 - 25);
        // The wallet cap: 30 % of 10,000 is 3,000, 1,000 held: 200 at 10.
        let m = entry_units(&K, 10_000.0, 1_000.0, 0, 1, 1e6, 10.0, 10_000);
        assert_eq!(m, ((K.wallet_share * 10_000.0 - 1_000.0) / 10.0) as i64);
        // Already at the cap of holdings at cost: nothing.
        assert_eq!(
            entry_units(&K, 10_000.0, K.wallet_share * 10_000.0, 0, 1, 1e6, 10.0, 99),
            0
        );
        // The ask side limits.
        assert_eq!(entry_units(&K, 1e9, 0.0, 0, 0, 1e6, 10.0, 7), 7);
        // Positions full.
        assert_eq!(
            entry_units(&K, 1e9, 0.0, 0, K.max_positions, 1e6, 10.0, 7),
            0
        );
        // Off.
        assert_eq!(entry_units(&K.off(), 1e9, 0.0, 0, 0, 1e6, 10.0, 7), 0);
    }

    #[test]
    fn arbitrage_over_remembered_price_aborts_when_live_bid_is_lower() {
        let (cost, margin, tax) = (100.0, 0.04, 0.08);
        let floor = confirm_floor(cost, margin, tax);
        assert!(confirm(Some(floor), cost, margin, tax));
        assert!(
            !confirm(Some(floor * 0.99), cost, margin, tax),
            "under the floor: abort"
        );
        assert!(!confirm(None, cost, margin, tax), "no bid at all: abort");
        // A live bid that pays after tax clears; one that only pays before tax does not.
        assert!(!confirm(Some(cost * (1.0 + margin)), cost, margin, tax));
    }

    #[test]
    fn the_levels_scale_the_appetite_and_off_is_off() {
        let easy = SpecKnobs::with(0.15, 2.0, 0.05);
        let brutal = SpecKnobs::with(0.50, 5.0, 0.01);
        assert!(easy.wallet_share < K.wallet_share && K.wallet_share < brutal.wallet_share);
        assert_eq!(K.off().wallet_share, 0.0);
        assert!(!wants_entry(&K.off(), &anchor(100.0, 85.0, 84.0, 10), 85.0));
    }

    #[test]
    fn the_tape_row_names_region_units_and_isk() {
        assert_eq!(spec_row(7, 12, 1234.4), "spec\t7\t12\t1234");
    }
}
