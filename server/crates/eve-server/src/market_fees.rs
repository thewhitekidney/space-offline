//! **The market's fees, taxes and limits as pure functions** (X-TRADE): what
//! the client's `marketutil` computes from a character's trade skills and
//! standings, in one place so the server's charges (`service/market.rs`) and
//! a bot's price rule (`eve-bot`'s `pricing.rs` and `trader.rs`) can never
//! drift apart. Nothing here reads the registry.
//!
//! Every formula is read from the client's own bytecode (build 3619879,
//! `tools/dis.py`), the module and line named in each doc **[V]**:
//!
//! | what | client source |
//! |---|---|
//! | sales tax `8 % x (1 - 0.11 x Accounting)` | `marketutil/skilllimits.py:86-87` |
//! | broker commission `3 % - 0.3 % x Broker Relations` | `skilllimits.py:29` (`GetBrokersFeeForLocation`) |
//! | standings: `- 0.0003 x faction - 0.0002 x corp` | `brokerFee.py:85` (`_GetStationOwnerAdjustedCommissionPercentage`), constants `marketutil/const.py:13-14` |
//! | fee floor `mktMinimumFee` | `brokerFee.py:74-76` |
//! | modification fee and its discount `0.5 + 0.06 x Margin Trading` | `brokerFee.py:65-68`, `skilllimits.py:43` |
//! | max orders `5 + 4 Trade + 8 Retail + 16 Wholesale + 32 Tycoon` | `skilllimits.py:82` |
//! | order range by skill: Visibility (`vis`), Marketing (`ask`), Procurement (`bid`), Daytrading (`mod`) | `skilllimits.py:93-102`, `JUMPS_PER_SKILL_LEVEL` `:6-13` |
//!
//! **One fact the skill table hides.** The build's `typeMarginTrading` is
//! type 16597 (`inventorycommon/const.py:1676`), which `types/skills.tsv`
//! names *Advanced Broker Relations* (rank 3, Broker Relations IV and
//! Accounting IV): one skill under two names. The client's fee code reads no
//! other "advanced" skill, so it changes neither the commission nor the sales
//! tax, only the *modification* fee.
//!
//! **Not modelled:** the faction-warfare fee adjustment
//! (`facwarCommon.GetAdjustedFeePercentage`, a militia member's discount in
//! its warzone) and the SCC surcharge (structures only: the client sets it to
//! 0 for a station, `marketsvc.py:192`).

/// `MKT_TRANSACTION_TAX_DEFAULT`, percent (`marketutil/const.py:33`) **[V]**.
pub const SALES_TAX_PERCENT: f64 = 8.0;
/// `ACCOUNTING_SKILL_MODIFIER` (`marketutil/const.py:11`) **[V]**: the sales
/// tax falls by this fraction of itself per Accounting level.
pub const ACCOUNTING_SKILL_MODIFIER: f64 = 0.11;
/// `marketCommissionPercentage` (`marketutil/const.py:7`) **[V]**: the
/// broker's commission at a station.
pub const BROKER_PERCENT: f64 = 3.0;
/// `BROKER_RELATIONS_SKILL_MODIFIER`, percentage points per level
/// (`marketutil/const.py:10`) **[V]**.
pub const BROKER_RELATIONS_SKILL_MODIFIER: f64 = 0.3;
/// `MARKET_FACTION_STANDING_MULTIPLIER` (`marketutil/const.py:13`) **[V]**:
/// the commission (a fraction) falls by this per point of the station
/// owner's faction's standing toward the character.
pub const FACTION_STANDING_MULTIPLIER: f64 = 0.0003;
/// `MARKET_NPC_CORP_STANDING_MULTIPLIER` (`marketutil/const.py:14`) **[V]**:
/// as above, for the station owner corporation's own standing.
pub const NPC_CORP_STANDING_MULTIPLIER: f64 = 0.0002;
/// `appConst.mktMinimumFee`, ISK **[V]**: `BrokerFeeProvider.GetBrokerFeeInfo`
/// raises any fee at or below it to it.
pub const MINIMUM_BROKER_FEE: f64 = 100.0;
/// The modification-fee discount at Margin Trading 0
/// (`skilllimits.GetModificationFeeDiscount`, `0.5 + 0.06 x level`) **[V]**.
pub const MODIFICATION_DISCOUNT_BASE: f64 = 0.5;
/// The discount per Margin Trading level **[V]**.
pub const MODIFICATION_DISCOUNT_PER_LEVEL: f64 = 0.06;

/// The skills the market reads, `inventorycommon/const.py` **[V]**.
pub const TYPE_TRADE: i64 = 3_443;
pub const TYPE_RETAIL: i64 = 3_444;
pub const TYPE_BROKER_RELATIONS: i64 = 3_446;
/// `typeVisibility`: how far a buy order's range may reach.
pub const TYPE_VISIBILITY: i64 = 3_447;
/// `typeProcurement`: remote buy orders.
pub const TYPE_PROCUREMENT: i64 = 16_594;
/// `typeDaytrading`: remote order modification.
pub const TYPE_DAYTRADING: i64 = 16_595;
pub const TYPE_WHOLESALE: i64 = 16_596;
/// `typeMarginTrading` (16597); the content calls it *Advanced Broker
/// Relations* (see the module doc).
pub const TYPE_MARGIN_TRADING: i64 = 16_597;
/// `typeMarketing`: remote sell orders.
pub const TYPE_MARKETING: i64 = 16_598;
pub const TYPE_ACCOUNTING: i64 = 16_622;
pub const TYPE_TYCOON: i64 = 18_580;

/// `rangeStation` (`marketutil/const.py:6`) **[V]**.
pub const RANGE_STATION: i64 = -1;
/// `rangeSolarSystem` **[V]**.
pub const RANGE_SOLAR_SYSTEM: i64 = 0;
/// `rangeRegion` **[V]**.
pub const RANGE_REGION: i64 = 32_767;

/// `JUMPS_PER_SKILL_LEVEL` (`skilllimits.py:6-13`) **[V]**: the reach, in the
/// order-range unit, a level of Visibility, Marketing, Procurement or
/// Daytrading grants. Level 0 is the station, 1 the system, 2..4 five, ten
/// and twenty jumps, 5 the region.
pub fn jumps_per_level(level: i64) -> i64 {
    match level.clamp(0, 5) {
        0 => RANGE_STATION,
        1 => RANGE_SOLAR_SYSTEM,
        2 => 5,
        3 => 10,
        4 => 20,
        _ => RANGE_REGION,
    }
}

/// The skill levels the market reads (`skilllimits.GetMarketSkills`), each
/// 0 when untrained. `Default` is the untrained pilot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TradeSkills {
    pub trade: i64,
    pub retail: i64,
    pub wholesale: i64,
    pub tycoon: i64,
    pub accounting: i64,
    pub broker_relations: i64,
    pub margin_trading: i64,
    pub marketing: i64,
    pub procurement: i64,
    pub visibility: i64,
    pub daytrading: i64,
}

impl TradeSkills {
    /// Read the levels out of `level_of(skill_type)`; each is clamped to
    /// 0..=5.
    pub fn read(level_of: impl Fn(i64) -> i64) -> TradeSkills {
        let l = |t| level_of(t).clamp(0, 5);
        TradeSkills {
            trade: l(TYPE_TRADE),
            retail: l(TYPE_RETAIL),
            wholesale: l(TYPE_WHOLESALE),
            tycoon: l(TYPE_TYCOON),
            accounting: l(TYPE_ACCOUNTING),
            broker_relations: l(TYPE_BROKER_RELATIONS),
            margin_trading: l(TYPE_MARGIN_TRADING),
            marketing: l(TYPE_MARKETING),
            procurement: l(TYPE_PROCUREMENT),
            visibility: l(TYPE_VISIBILITY),
            daytrading: l(TYPE_DAYTRADING),
        }
    }

    /// The sales tax rate, a fraction of a sale ([`sales_tax_rate`]).
    pub fn sales_tax_rate(&self) -> f64 {
        sales_tax_rate(self.accounting)
    }

    /// The broker's commission, a fraction of an order's value, at a station
    /// whose owner stands at `standings` ([`broker_rate`]).
    pub fn broker_rate(&self, standings: Standings) -> f64 {
        broker_rate(self.broker_relations, standings)
    }

    /// The most open orders ([`max_orders`]).
    pub fn max_orders(&self) -> usize {
        max_orders(self.trade, self.retail, self.wholesale, self.tycoon)
    }

    /// The modification-fee discount ([`modification_discount`]).
    pub fn modification_discount(&self) -> f64 {
        modification_discount(self.margin_trading)
    }

    /// The farthest a resting buy order's range may reach
    /// ([`allowed_buy_range`]).
    pub fn buy_range(&self) -> i64 {
        jumps_per_level(self.visibility)
    }
}

/// A character's standing with a station's owner, as the broker's fee reads
/// them (`marketsvc.FactionAndCorpToCharStandings`): the owner's faction and
/// the owner corporation itself, both zero for a player-owned station
/// **[V]** (`marketsvc.py:870`: only an NPC owner has standings).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Standings {
    pub faction: f64,
    pub corp: f64,
}

/// The sales tax as a fraction of a sale at Accounting `accounting`:
/// `8 % x (1 - 0.11 x Accounting)` (`skilllimits.py:86-87`) **[V]**. The
/// location discount (`:88`) is Zarzakh's only and never ours.
pub fn sales_tax_rate(accounting: i64) -> f64 {
    SALES_TAX_PERCENT / 100.0 * (1.0 - accounting.clamp(0, 5) as f64 * ACCOUNTING_SKILL_MODIFIER)
}

/// The broker's commission as a fraction of an order's value:
/// `(3 % - 0.3 % x Broker Relations) - 0.0003 x faction standing - 0.0002 x
/// corp standing` (`skilllimits.py:29`, `brokerFee.py:85`) **[V]**. A negative
/// standing *raises* it, as the client's does; it never goes below 0.
pub fn broker_rate(broker_relations: i64, standings: Standings) -> f64 {
    let base = (BROKER_PERCENT
        - broker_relations.clamp(0, 5) as f64 * BROKER_RELATIONS_SKILL_MODIFIER)
        / 100.0;
    (base
        - standings.faction * FACTION_STANDING_MULTIPLIER
        - standings.corp * NPC_CORP_STANDING_MULTIPLIER)
        .max(0.0)
}

/// The modification-fee discount at Margin Trading `level`: `0.5 + 0.06 x
/// level` (`skilllimits.GetModificationFeeDiscount`) **[V]**.
pub fn modification_discount(level: i64) -> f64 {
    MODIFICATION_DISCOUNT_BASE + MODIFICATION_DISCOUNT_PER_LEVEL * level.clamp(0, 5) as f64
}

/// What a new order, or a change to a resting one, costs the broker's fee
/// before rounding to the cent and before the floor
/// (`BrokerFeeProvider.GetBrokerFeeInfo`, `brokerFee.py:54-77`) **[V]**:
///
/// - a new order (`old_amount` `None`): `new x rate`;
/// - a change: the modification fee `new x rate x (1 - discount)`, plus
///   `(new - old) x rate` when the order grows.
///
/// `amount` is price times remaining volume. The caller applies
/// [`MINIMUM_BROKER_FEE`] ([`fee_with_floor`]).
pub fn raw_broker_fee(old_amount: Option<f64>, new_amount: f64, rate: f64, discount: f64) -> f64 {
    match old_amount {
        None => new_amount * rate,
        Some(old) => {
            let modification = new_amount * rate * (1.0 - discount);
            let growth = if new_amount > old {
                (new_amount - old) * rate
            } else {
                0.0
            };
            modification + growth
        }
    }
}

/// A fee at or below [`MINIMUM_BROKER_FEE`] is the minimum
/// (`brokerFee.py:74-76`) **[V]**.
pub fn fee_with_floor(fee: f64) -> f64 {
    if fee <= MINIMUM_BROKER_FEE {
        MINIMUM_BROKER_FEE
    } else {
        fee
    }
}

/// The most open orders: `5 + 4 Trade + 8 Retail + 16 Wholesale + 32 Tycoon`
/// (`skilllimits.py:82`) **[V]**.
pub fn max_orders(trade: i64, retail: i64, wholesale: i64, tycoon: i64) -> usize {
    (5 + 4 * trade + 8 * retail + 16 * wholesale + 32 * tycoon).max(0) as usize
}

/// The widest range a resting buy order may be given by a pilot at
/// Visibility `visibility`: the client offers the station, plus every range
/// whose jumps are within `JUMPS_PER_SKILL_LEVEL[Visibility]`
/// (`buyThisTypeWindow._GetAvailableRanges:435`) **[V]**. Returns `range`
/// itself when allowed, else the widest allowed one below it.
pub fn allowed_buy_range(range: i64, visibility: i64) -> i64 {
    if range == RANGE_STATION {
        return range;
    }
    let reach = jumps_per_level(visibility);
    if reach == RANGE_STATION {
        return RANGE_STATION;
    }
    if range <= reach {
        return range;
    }
    // Over reach: the widest rung of the client's list at or below it.
    const RUNGS: [i64; 11] = [
        RANGE_SOLAR_SYSTEM,
        1,
        2,
        3,
        4,
        5,
        10,
        20,
        30,
        40,
        RANGE_REGION,
    ];
    RUNGS
        .iter()
        .copied()
        .filter(|&r| r <= reach)
        .max()
        .unwrap_or(RANGE_STATION)
}

/// Round to the cent, as the wallet does.
pub fn cents(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounting_lowers_the_tax_by_eleven_percent_of_itself_a_level() {
        assert!((sales_tax_rate(0) - 0.08).abs() < 1e-12);
        for l in 0..=5 {
            let want = 0.08 * (1.0 - 0.11 * l as f64);
            assert!((sales_tax_rate(l) - want).abs() < 1e-12, "{l}");
        }
        assert!(sales_tax_rate(5) < sales_tax_rate(4));
        // Out-of-range levels are clamped, never a negative tax.
        assert!((sales_tax_rate(99) - sales_tax_rate(5)).abs() < 1e-12);
        assert!((sales_tax_rate(-3) - sales_tax_rate(0)).abs() < 1e-12);
    }

    #[test]
    fn broker_relations_and_standings_lower_the_commission_and_it_never_goes_negative() {
        let none = Standings::default();
        assert!((broker_rate(0, none) - 0.03).abs() < 1e-12);
        assert!((broker_rate(5, none) - 0.015).abs() < 1e-12);
        let liked = Standings {
            faction: 10.0,
            corp: 10.0,
        };
        assert!((broker_rate(0, liked) - (0.03 - 0.003 - 0.002)).abs() < 1e-12);
        assert!((broker_rate(5, liked) - (0.015 - 0.005)).abs() < 1e-12);
        let hated = Standings {
            faction: -10.0,
            corp: -10.0,
        };
        assert!(broker_rate(0, hated) > broker_rate(0, none));
        let absurd = Standings {
            faction: 1e6,
            corp: 1e6,
        };
        assert_eq!(broker_rate(5, absurd), 0.0);
    }

    #[test]
    fn the_modification_fee_discounts_the_whole_amount_and_charges_growth_in_full() {
        let rate = 0.03;
        // New order: the commission on the amount.
        assert!((raw_broker_fee(None, 10_000.0, rate, 0.5) - 300.0).abs() < 1e-9);
        // A cut price: the discounted modification fee only.
        let cut = raw_broker_fee(Some(10_000.0), 8_000.0, rate, 0.5);
        assert!((cut - 8_000.0 * rate * 0.5).abs() < 1e-9);
        // A raised price: that, plus the commission on the growth.
        let grown = raw_broker_fee(Some(10_000.0), 12_000.0, rate, 0.5);
        assert!((grown - (12_000.0 * rate * 0.5 + 2_000.0 * rate)).abs() < 1e-9);
        // Margin Trading deepens the discount.
        assert!(modification_discount(5) > modification_discount(0));
        assert!((modification_discount(0) - 0.5).abs() < 1e-12);
        assert!((modification_discount(5) - 0.8).abs() < 1e-12);
    }

    #[test]
    fn the_minimum_fee_catches_a_fee_at_or_below_it() {
        assert_eq!(fee_with_floor(MINIMUM_BROKER_FEE), MINIMUM_BROKER_FEE);
        assert_eq!(fee_with_floor(1.0), MINIMUM_BROKER_FEE);
        assert_eq!(fee_with_floor(100.01), 100.01);
    }

    #[test]
    fn the_order_cap_adds_the_four_skills() {
        assert_eq!(max_orders(0, 0, 0, 0), 5);
        assert_eq!(max_orders(5, 5, 5, 5), 5 + 20 + 40 + 80 + 160);
        let s = TradeSkills {
            trade: 4,
            retail: 4,
            ..TradeSkills::default()
        };
        assert_eq!(s.max_orders(), 5 + 16 + 32);
    }

    #[test]
    fn a_buy_range_is_clamped_to_the_visibility_reach() {
        // No Visibility: the station only.
        assert_eq!(allowed_buy_range(RANGE_REGION, 0), RANGE_STATION);
        assert_eq!(allowed_buy_range(RANGE_SOLAR_SYSTEM, 0), RANGE_STATION);
        assert_eq!(allowed_buy_range(RANGE_STATION, 0), RANGE_STATION);
        // Level 1: the system.
        assert_eq!(allowed_buy_range(RANGE_SOLAR_SYSTEM, 1), RANGE_SOLAR_SYSTEM);
        assert_eq!(allowed_buy_range(3, 1), RANGE_SOLAR_SYSTEM);
        // Level 2: up to five jumps; ten is cut to five.
        assert_eq!(allowed_buy_range(5, 2), 5);
        assert_eq!(allowed_buy_range(10, 2), 5);
        assert_eq!(allowed_buy_range(20, 3), 10);
        // Level 5: the region.
        assert_eq!(allowed_buy_range(RANGE_REGION, 5), RANGE_REGION);
        assert_eq!(allowed_buy_range(40, 4), 20);
    }

    #[test]
    fn the_skill_read_clamps_and_defaults_to_untrained() {
        let s = TradeSkills::read(|t| if t == TYPE_ACCOUNTING { 9 } else { 0 });
        assert_eq!(s.accounting, 5);
        assert_eq!(
            s,
            TradeSkills {
                accounting: 5,
                ..TradeSkills::default()
            }
        );
        assert!((s.sales_tax_rate() - sales_tax_rate(5)).abs() < 1e-12);
        assert_eq!(cents(1.005_f64 + 0.000_1), 1.01);
    }
}
