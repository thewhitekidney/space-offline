//! **Gas and polymer prices for the reactor** (lane T3-PRICE, packet
//! T3-POLYMER-PRICE, `docs/lanes/t3-supply.md` lane 4).
//!
//! The reactor ranks a formula only when its inputs, its product and the
//! formula have a `P_ref` ([`crate::reactor::rank_formulas`]). A harvestable
//! gas has `basePrice` 0 and no recipe, so the oracle (`tools/content_econ.py`)
//! leaves it unpriced, and every polymer that eats it ("source reaction":
//! the sum of its inputs) is unpriced with it. This file fills exactly that
//! hole, in **one table and one function**, so a market-driven price can
//! replace it later (the R-FLUIDECON audit is looking at price formation):
//!
//! - [`GAS_LABOUR`]: the inputs of a gas's base price, and [`gas_ref`], the
//!   base from harvest labour;
//! - [`fill_refs`]: the gases that lack a `P_ref` get [`gas_ref`], then every
//!   reaction product that lacks one gets the oracle's reaction rule (the sum
//!   of its inputs over the units a run yields), to a fixed point.
//!
//! A type that already has a `P_ref` is never touched. No client-visible
//! change: the numbers only feed the bots' ranking.

use crate::hull_choice::Class;
use eve_server::industry;
use std::collections::BTreeMap;

/// What a gas costs to harvest: one row of the table. Every number is **[A]**
/// except the yield, which the server's own mining code fixes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Labour {
    /// Cubic metres one cycle of a Gas Cloud Scoop I cuts **[V-data]**
    /// (`mining.rs`: "a Gas Cloud Scoop I's 10 m3").
    pub yield_m3: f64,
    /// Seconds a cycle takes **[A]**. Falsified by the scoop's duration
    /// attribute in the type data (the base price scales with it).
    pub cycle_seconds: f64,
    /// The share of an hour a huffer spends harvesting, the rest being
    /// scanning, warping, hauling and selling **[A]**. Falsified by a soak's
    /// huffer harvesting far more or less than this share of its active time.
    pub uptime: f64,
    /// The frigate class whose belt-ratting income is the opportunity cost of
    /// an hour of labour **[A]** (a Venture is a frigate; `earnings::ratter_isk_h`).
    /// Falsified by gas that no bot harvests (too cheap against ratting) or
    /// that sells for far more than it costs in a soak (too dear).
    pub class: Class,
}

/// The one table: how a gas's base price is derived.
pub const GAS_LABOUR: Labour = Labour {
    yield_m3: 10.0,
    cycle_seconds: 30.0,
    uptime: 0.5,
    class: Class::Frigate,
};

/// **The base price of one unit of a gas** (pure) of `volume_m3` per unit:
/// an hour of labour at `isk_h` cuts `yield_m3 x 3600 / cycle x uptime` cubic
/// metres, so a cubic metre costs `isk_h` over that, and a unit its volume
/// times that. `None` for a volume or income that is not positive.
pub fn gas_ref(labour: &Labour, isk_h: f64, volume_m3: f64) -> Option<f64> {
    let m3_per_hour = labour.yield_m3 * 3_600.0 / labour.cycle_seconds * labour.uptime;
    let price = isk_h / m3_per_hour * volume_m3;
    (price.is_finite() && price > 0.0 && isk_h > 0.0 && m3_per_hour > 0.0).then_some(price)
}

/// [`gas_ref`] at the table's labour and the frigate ratter's income.
pub fn gas_base(volume_m3: f64) -> Option<f64> {
    gas_ref(
        &GAS_LABOUR,
        crate::earnings::ratter_isk_h(GAS_LABOUR.class),
        volume_m3,
    )
}

/// Every gas the content knows, with its unit volume: [`crate::huffer::GAS_TYPES`]
/// found in the type table.
pub fn gas_volumes(content: &eve_server::content::Content) -> Vec<(i64, f64)> {
    crate::huffer::GAS_TYPES
        .iter()
        .filter_map(|&t| Some((t, eve_server::item::find_in(content, t)?.volume)))
        .collect()
}

/// **Fill the holes in `refs`** (pure) and return the types added, ascending:
/// a gas of `gas` (`(type, unit volume)`) with no `P_ref` gets
/// [`gas_ref`] at `isk_h`; then every reaction product with none, whose inputs
/// all have one, gets `sum(P_ref x quantity) / product_quantity` (the
/// oracle's `reaction` rule), repeated until nothing more prices, so a chain of
/// reactions (gas to polymer to a component) prices through.
pub fn fill_refs(
    refs: &mut BTreeMap<i64, f64>,
    table: &industry::Table,
    gas: &[(i64, f64)],
    isk_h: f64,
) -> Vec<i64> {
    let mut added = Vec::new();
    for &(t, volume) in gas {
        if !refs.contains_key(&t)
            && let Some(p) = gas_ref(&GAS_LABOUR, isk_h, volume)
        {
            refs.insert(t, p);
            added.push(t);
        }
    }
    loop {
        let mut more = false;
        for bp in &table.reactions {
            if refs.contains_key(&bp.product_type_id) {
                continue;
            }
            let sum: Option<f64> = bp
                .materials
                .iter()
                .map(|&(m, q)| refs.get(&m).filter(|p| **p > 0.0).map(|p| p * q as f64))
                .sum();
            if let Some(sum) = sum.filter(|s| *s > 0.0 && !bp.materials.is_empty()) {
                refs.insert(bp.product_type_id, sum / bp.product_quantity.max(1) as f64);
                added.push(bp.product_type_id);
                more = true;
            }
        }
        if !more {
            break;
        }
    }
    added.sort_unstable();
    added.dedup();
    added
}

/// Fill a cycle's [`crate::pricing::Prices`] from the content (the reactor's
/// call): no content or no formulas, nothing to do.
pub fn fill(prices: &mut crate::pricing::Prices, content: Option<&eve_server::content::Content>) {
    let Some(content) = content else { return };
    let Some(table) = crate::reactor::table_of(Some(content)) else {
        return;
    };
    let isk_h = crate::earnings::ratter_isk_h(GAS_LABOUR.class);
    fill_refs(prices.refs_mut(), table, &gas_volumes(content), isk_h);
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids and numbers only.
    const FORMULA: i64 = 990_950_001;
    const FORMULA_2: i64 = 990_950_002;
    const GAS: i64 = 990_950_101;
    const GAS_PRICED: i64 = 990_950_102;
    const FUEL: i64 = 990_950_103;
    const POLYMER: i64 = 990_950_201;
    const COMPONENT: i64 = 990_950_202;
    const FUEL_REF: f64 = 20.0;
    const GAS_VOLUME: f64 = 2.0;
    const LABOUR_ISK_H: f64 = 1_000_000.0;
    const PER_RUN: i64 = 10;

    fn table() -> industry::Table {
        let prices = eve_server::econ_prices::Table::parse(&[
            format!("{FUEL}\t{FUEL_REF}\tbase\t{FUEL_REF}\t4\t99\t\t1"),
            format!("{FORMULA}\t5000.0\tbase\t0.0\t9\t1888\t\t1"),
            format!("{FORMULA_2}\t5000.0\tbase\t0.0\t9\t1888\t\t1"),
        ])
        .expect("prices");
        let t = 3_600;
        let skill = eve_server::reactions::SKILL_REACTIONS;
        industry::Table::parse(
            &[
                format!("{FORMULA}\t9\tactivity\t0\t0\t\t{t}\t1000"),
                format!("{FORMULA}\t9\tmaterial\t{FUEL}\t5\t\t{t}\t1000"),
                format!("{FORMULA}\t9\tmaterial\t{GAS}\t100\t\t{t}\t1000"),
                format!("{FORMULA}\t9\tproduct\t{POLYMER}\t{PER_RUN}\t\t{t}\t1000"),
                format!("{FORMULA}\t9\tskill\t{skill}\t3\t\t{t}\t1000"),
                format!("{FORMULA_2}\t9\tactivity\t0\t0\t\t{t}\t1000"),
                format!("{FORMULA_2}\t9\tmaterial\t{POLYMER}\t10\t\t{t}\t1000"),
                format!("{FORMULA_2}\t9\tmaterial\t{FUEL}\t5\t\t{t}\t1000"),
                format!("{FORMULA_2}\t9\tproduct\t{COMPONENT}\t5\t\t{t}\t1000"),
                format!("{FORMULA_2}\t9\tskill\t{skill}\t3\t\t{t}\t1000"),
            ],
            &prices,
        )
        .expect("table")
    }

    #[test]
    fn a_gas_costs_its_volume_of_harvest_labour() {
        let l = GAS_LABOUR;
        let m3_h = l.yield_m3 * 3_600.0 / l.cycle_seconds * l.uptime;
        let p = gas_ref(&l, LABOUR_ISK_H, GAS_VOLUME).expect("priced");
        assert!((p - LABOUR_ISK_H / m3_h * GAS_VOLUME).abs() < 1e-9);
        // Twice the volume, twice the price; twice the income, twice again.
        assert!((gas_ref(&l, LABOUR_ISK_H, 2.0 * GAS_VOLUME).unwrap() - 2.0 * p).abs() < 1e-9);
        assert!((gas_ref(&l, 2.0 * LABOUR_ISK_H, GAS_VOLUME).unwrap() - 2.0 * p).abs() < 1e-9);
    }

    #[test]
    fn no_income_or_no_volume_prices_nothing() {
        assert_eq!(gas_ref(&GAS_LABOUR, 0.0, 1.0), None);
        assert_eq!(gas_ref(&GAS_LABOUR, LABOUR_ISK_H, 0.0), None);
        assert_eq!(gas_ref(&GAS_LABOUR, f64::NAN, 1.0), None);
        assert!(gas_base(1.0).is_some_and(|p| p > 0.0));
    }

    #[test]
    fn gas_then_the_polymer_then_the_component_price_through() {
        let t = table();
        let mut refs = t.prices.clone();
        assert!(!refs.contains_key(&POLYMER) && !refs.contains_key(&GAS));
        let added = fill_refs(&mut refs, &t, &[(GAS, GAS_VOLUME)], LABOUR_ISK_H);
        assert_eq!(added, vec![GAS, POLYMER, COMPONENT]);
        let gas = gas_ref(&GAS_LABOUR, LABOUR_ISK_H, GAS_VOLUME).unwrap();
        let polymer = (5.0 * FUEL_REF + 100.0 * gas) / PER_RUN as f64;
        assert!((refs[&POLYMER] - polymer).abs() < 1e-6);
        let component = (10.0 * polymer + 5.0 * FUEL_REF) / 5.0;
        assert!((refs[&COMPONENT] - component).abs() < 1e-6);
    }

    #[test]
    fn a_price_the_oracle_gave_is_never_replaced() {
        let t = table();
        let mut refs = t.prices.clone();
        refs.insert(GAS, 7.0);
        refs.insert(POLYMER, 11.0);
        let added = fill_refs(&mut refs, &t, &[(GAS, GAS_VOLUME)], LABOUR_ISK_H);
        assert_eq!(refs[&GAS], 7.0);
        assert_eq!(refs[&POLYMER], 11.0);
        assert_eq!(added, vec![COMPONENT]);
    }

    #[test]
    fn a_gas_the_world_lacks_leaves_its_polymer_unpriced() {
        let t = table();
        let mut refs = t.prices.clone();
        let added = fill_refs(&mut refs, &t, &[], LABOUR_ISK_H);
        assert!(added.is_empty());
        assert!(!refs.contains_key(&POLYMER));
        // An unrelated gas does not help either.
        let none = fill_refs(&mut refs, &t, &[(GAS_PRICED, 1.0)], LABOUR_ISK_H);
        assert_eq!(none, vec![GAS_PRICED]);
        assert!(!refs.contains_key(&POLYMER));
    }

    #[test]
    fn the_filled_prices_let_the_reactor_rank_the_formula() {
        let t = table();
        let mut bare = t.prices.clone();
        assert!(crate::reactor::rank_formulas(&t, &bare, &[], None).is_empty());
        fill_refs(&mut bare, &t, &[(GAS, GAS_VOLUME)], LABOUR_ISK_H);
        // Ranked or not by payback, the formula is no longer skipped for want
        // of a price: its bids exist.
        let bp = t.find_reaction(FORMULA).expect("formula");
        let fee = crate::reactor::run_fee(&t, bp, 1);
        assert!(crate::reactor::reaction_bids(bp, &bare, &bare, fee).is_some());
    }
}
