//! **PLEX on the market, by a bot** (L7 BOT-ECON, issue #34; spec
//! `handback/r-nes-econ/report.md` §2.7). Two bots do it, both on the price
//! memory the traders already keep ([`crate::price_memory`]):
//!
//! - **A trader** buys PLEX when the cheapest ask sits under the slow anchor
//!   (the 20-day EWMA of what the bot has seen) and sells what it holds when
//!   the best bid sits over it ([`decide`]). It speculates in lots of
//!   [`LOT`] and never spends more than [`WALLET_SHARE`] of its ISK on one.
//! - **Any bot with surplus PLEX in the vault** sells it ([`surplus`]): what
//!   is above twice what its NES wishlist costs, less [`KEEP`]. It takes the
//!   standing bid only at or above [`DUMP_FLOOR`] of the anchor, so a bot
//!   does not dump into a thin book.
//!
//! **Nothing from nowhere.** PLEX enters the world only through the owner's
//! sources (drops, geodes, the owner's own purchases); a bot sells what it
//! was paid. The price is the market's. `BOT_PLEX_MULT` 0.5 is the sources'
//! side (`eve-mod-econ` drops), not here.
//!
//! **Every number is [A]**, falsified by the soak's `[econ] plex` band after
//! two hours: bots that pile up PLEX (lower [`KEEP`]) or a PLEX price that
//! swings a tenth a day (raise [`BUY_BELOW`], lower [`LOT`]).

use crate::price_memory::Anchor;

/// PLEX a bot keeps in the vault whatever its wishlist says **[A]**.
pub const KEEP: i64 = 20;
/// Units one speculative buy or sale moves **[A]**.
pub const LOT: i64 = 10;
/// A trader buys when the ask is this far under the anchor **[A]**.
pub const BUY_BELOW: f64 = 0.04;
/// ... and sells when the bid is this far over it **[A]**.
pub const SELL_ABOVE: f64 = 0.04;
/// A surplus is sold into a bid at least this share of the anchor **[A]**.
pub const DUMP_FLOOR: f64 = 0.95;
/// The share of its ISK a trader puts into one buy **[A]**.
pub const WALLET_SHARE: f64 = 0.10;
/// Days the anchor needs before a trader trusts it **[A]**.
pub const MIN_DAYS: u32 = 3;

/// What a bot does with PLEX on the market this turn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Move {
    /// Buy up to `units` at `price` or better.
    Buy {
        units: i64,
        price: f64,
    },
    /// Sell `units` into the bid at `price` or better.
    Sell {
        units: i64,
        price: f64,
    },
    Hold,
}

/// The surplus PLEX of a vault with `balance` whole PLEX when the bot's NES
/// wishlist costs `wish`: what is above twice the wish (never under
/// [`KEEP`]).
pub fn surplus(balance: i64, wish: i64) -> i64 {
    (balance - (2 * wish.max(0)).max(KEEP)).max(0)
}

/// **A trader's move** over `anchor`, the cheapest `ask` and best `bid`
/// seen, the `held` units in its hangar and its `isk`. Pure.
pub fn decide(
    anchor: Option<&Anchor>,
    ask: Option<f64>,
    bid: Option<f64>,
    held: i64,
    isk: f64,
) -> Move {
    let Some(a) = anchor.filter(|a| a.n >= MIN_DAYS && a.ewma > 0.0) else {
        return Move::Hold;
    };
    if held > 0
        && let Some(b) = bid
        && b >= a.ewma * (1.0 + SELL_ABOVE)
    {
        return Move::Sell {
            units: held.min(LOT),
            price: b,
        };
    }
    if let Some(p) = ask
        && p > 0.0
        && p <= a.ewma * (1.0 - BUY_BELOW)
    {
        let afford = (isk * WALLET_SHARE / p).floor() as i64;
        let units = afford.min(LOT);
        if units >= 1 {
            return Move::Buy { units, price: p };
        }
    }
    Move::Hold
}

/// **Sell a vault surplus?** `want` units withdrawn, into `bid` when it is
/// not under [`DUMP_FLOOR`] of the anchor (no anchor: any bid).
pub fn dump(anchor: Option<&Anchor>, bid: Option<f64>, want: i64) -> Move {
    let (Some(b), true) = (bid, want > 0) else {
        return Move::Hold;
    };
    if anchor.is_some_and(|a| b < a.ewma * DUMP_FLOOR) {
        return Move::Hold;
    }
    Move::Sell {
        units: want,
        price: b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(ewma: f64, n: u32) -> Anchor {
        Anchor {
            ewma,
            v: ewma,
            prev: ewma,
            day: 10,
            n,
        }
    }

    #[test]
    fn a_vault_keeps_twice_the_wishlist_and_a_floor() {
        assert_eq!(surplus(300, 100), 100);
        assert_eq!(
            surplus(200, 100),
            0,
            "exactly twice the wish is not surplus"
        );
        assert_eq!(
            surplus(50, 0),
            30,
            "the floor applies when nothing is wished"
        );
        assert_eq!(surplus(10, 0), 0);
        assert_eq!(surplus(5, -9), 0);
    }

    #[test]
    fn a_trader_buys_under_the_anchor_and_sells_over_it() {
        let a = anchor(1_000.0, 10);
        assert_eq!(
            decide(Some(&a), Some(900.0), Some(850.0), 0, 100_000.0),
            Move::Buy {
                units: 10,
                price: 900.0
            }
        );
        assert_eq!(
            decide(Some(&a), Some(1_200.0), Some(1_100.0), 25, 100_000.0),
            Move::Sell {
                units: LOT,
                price: 1_100.0
            }
        );
        assert_eq!(
            decide(Some(&a), Some(1_000.0), Some(1_000.0), 25, 100_000.0),
            Move::Hold,
            "at the anchor there is nothing to do"
        );
    }

    #[test]
    fn a_trader_with_nothing_to_sell_does_not_sell_and_one_without_isk_does_not_buy() {
        let a = anchor(1_000.0, 10);
        assert_eq!(decide(Some(&a), None, Some(1_500.0), 0, 1e9), Move::Hold);
        assert_eq!(decide(Some(&a), Some(500.0), None, 0, 100.0), Move::Hold);
        // It buys only what a tenth of its wallet affords.
        assert_eq!(
            decide(Some(&a), Some(500.0), None, 0, 20_000.0),
            Move::Buy {
                units: 4,
                price: 500.0
            }
        );
    }

    #[test]
    fn a_young_anchor_or_none_is_not_trusted() {
        assert_eq!(
            decide(Some(&anchor(1_000.0, 1)), Some(100.0), None, 0, 1e9),
            Move::Hold
        );
        assert_eq!(decide(None, Some(100.0), Some(5_000.0), 9, 1e9), Move::Hold);
    }

    #[test]
    fn a_surplus_is_not_dumped_into_a_thin_book() {
        let a = anchor(1_000.0, 10);
        assert_eq!(
            dump(Some(&a), Some(960.0), 30),
            Move::Sell {
                units: 30,
                price: 960.0
            }
        );
        assert_eq!(dump(Some(&a), Some(500.0), 30), Move::Hold);
        assert_eq!(dump(Some(&a), None, 30), Move::Hold);
        assert_eq!(dump(Some(&a), Some(960.0), 0), Move::Hold);
        assert!(matches!(dump(None, Some(1.0), 3), Move::Sell { .. }));
    }
}
