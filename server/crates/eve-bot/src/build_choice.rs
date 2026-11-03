//! **FE-BUILD: a builder chooses what to build by margin** (R-FLUIDECON
//! 3.3 and the FE-BUILD packet; DOC-7's demand map).
//!
//! Until now a builder walked the ladder in rank order and `return`ed after
//! one blueprint, so the cheapest rung it held was built whatever the
//! market paid. Here it **appraises every blueprint it holds and can feed**:
//! the product at `V` (the value model, `pricing::Prices`), less the inputs
//! at `V` with the broker's fee, the listing's tax and an install fee, per
//! hour of the job's time ([`appraise`]). The jobs are ranked by that
//! ([`order_jobs`]), a job that loses money is skipped, and the builder
//! starts as many as it has free slots ([`jobs_to_start`]).
//!
//! **Demand tilts the choice.** The book's bids against its asks for the
//! product, and the units the leaders' doctrines want of a part
//! ([`crate::doctrine_parts::PartsBoard`]), make a pressure in -1..1
//! ([`pressure_of`]); the product's revenue is tilted by [`DEMAND_TILT`] of
//! it. A rung whose product is wanted climbs ahead of the ladder's order
//! ([`pull_demanded`]) once the pressure has held for [`FE_DEMAND_CYCLES`]
//! cycles or a doctrine names the part.
//!
//! Pure: no socket, no clock, no random. Every number is **[A]**; each
//! falsifier is on its const.

use crate::pricing::Rates;
use eve_server::industry::Blueprint;
use std::collections::BTreeMap;

/// The bid pressure on a product (in -1..1) at which a builder buys the
/// blueprint ahead of the ladder's order **[A]** (R-FLUIDECON
/// `FE_DEMAND_BUY`). Falsified by builders that climb for a one-bid blip
/// (too low) or never pull a rung while hulls sell out (too high).
pub const FE_DEMAND_BUY: f64 = 0.3;
/// Cycles the pressure must hold before the pull **[A]**. Falsified by a
/// pull that follows a single human's bid and is gone next cycle.
pub const FE_DEMAND_CYCLES: u32 = 2;
/// Job-runs of inputs a builder keeps ahead for the job it will start next
/// **[A]**: it rests bids for this many jobs' worth while its slots are
/// full, so the next job starts the cycle a slot frees. Falsified by idle
/// slots waiting on inputs (too low) or wallets locked in bids (too high).
pub const FE_INPUT_RUNS: i64 = 2;
/// How much of a product's revenue the demand pressure moves **[A]**: at
/// full pressure the job is worth this much more (less at full glut).
/// Falsified by the tape's job-hours by basket not following a bid-heavy
/// shift (too low) or whipsawing on it (too high).
pub const DEMAND_TILT: f64 = 0.15;
/// What installing a job costs, as a share of its inputs' value **[A]**
/// (the server charges by the system cost index; this is the NPC-station
/// scale). Falsified by `install` refusals for want of ISK, or jobs the
/// choice calls profitable that the wallet shows losing.
pub const INSTALL_FEE_SHARE: f64 = 0.01;
/// Most picks that rest standing bids in one cycle **[A]**: the wallet
/// share bounds the sum, this bounds the spread. Falsified by stalls on
/// `NotEnoughMoney` (too high).
pub const BID_PICKS: usize = 2;

/// What one blueprint is worth to build, per hour of its slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Appraisal {
    pub blueprint: i64,
    pub product: i64,
    /// ISK a run earns, net of the listing's tax and fee, less inputs at
    /// their value with the bid fee and the install fee.
    pub profit_per_run: f64,
    /// Game hours a run holds a slot.
    pub run_hours: f64,
    pub profit_per_hour: f64,
}

/// The pressure of `bids` against `asks`, in -1..1 (0 with neither).
pub fn pressure_of(bids: i64, asks: i64) -> f64 {
    let (b, a) = (bids.max(0) as f64, asks.max(0) as f64);
    if b + a <= 0.0 {
        0.0
    } else {
        ((b - a) / (b + a)).clamp(-1.0, 1.0)
    }
}

/// **The appraisal** of `bp` at ME `me`: the product at `mkt[product]`, each
/// input at `mkt[input]` (the `V` of [`crate::pricing::Prices::mkt_of`]),
/// `pressure` the demand for the product. `None` when a price is missing
/// or the recipe yields nothing.
pub fn appraise(
    bp: &Blueprint,
    me: i64,
    runs: i64,
    mkt: &BTreeMap<i64, f64>,
    pressure: f64,
    rates: Rates,
) -> Option<Appraisal> {
    let runs = runs.max(1);
    let price = |t: i64| mkt.get(&t).copied().filter(|p| *p > 0.0);
    let product = price(bp.product_type_id)?;
    let units = bp.product_quantity.max(0) as f64;
    if units <= 0.0 || bp.time <= 0 {
        return None;
    }
    let tilt = 1.0 + DEMAND_TILT * pressure.clamp(-1.0, 1.0);
    let revenue = rates.net_of_listing(product * units) * tilt;
    let mut inputs = 0.0;
    for (t, q) in crate::builder::materials_for(bp, runs, me) {
        inputs += q as f64 * rates.cost_of_bid(price(t)?);
    }
    let per_run = inputs / runs as f64;
    let profit = revenue - per_run * (1.0 + INSTALL_FEE_SHARE);
    let hours = bp.time as f64 / 3_600.0;
    Some(Appraisal {
        blueprint: bp.blueprint_type_id,
        product: bp.product_type_id,
        profit_per_run: profit,
        run_hours: hours,
        profit_per_hour: profit / hours,
    })
}

/// **The jobs worth doing, best first**: a job that does not pay
/// (`profit_per_hour <= 0`) is dropped; the rest by profit per hour, then
/// by `rank` (the ladder's, higher first), then by blueprint id. `T` is
/// whatever the caller carries along (a blueprint row, its bids).
pub fn order_jobs<T>(jobs: Vec<(T, Appraisal, i64)>) -> Vec<(T, Appraisal)> {
    let mut v: Vec<(T, Appraisal, i64)> = jobs
        .into_iter()
        .filter(|(_, a, _)| a.profit_per_hour.is_finite() && a.profit_per_hour > 0.0)
        .collect();
    v.sort_by(|a, b| {
        b.1.profit_per_hour
            .total_cmp(&a.1.profit_per_hour)
            .then(b.2.cmp(&a.2))
            .then(a.1.blueprint.cmp(&b.1.blueprint))
    });
    v.into_iter().map(|(t, a, _)| (t, a)).collect()
}

/// The first `free` of the ranked jobs, each product once (two originals of
/// one blueprint are one choice).
pub fn jobs_to_start<T: Clone>(ranked: &[(T, Appraisal)], free: usize) -> Vec<(T, Appraisal)> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for (t, a) in ranked {
        if out.len() >= free {
            break;
        }
        if seen.contains(&a.product) {
            continue;
        }
        seen.push(a.product);
        out.push((t.clone(), *a));
    }
    out
}

/// The runs of inputs to rest bids for while the slots are full: the job's
/// `planned` runs times [`FE_INPUT_RUNS`].
pub fn buffered_runs(planned: i64) -> i64 {
    planned.max(0).saturating_mul(FE_INPUT_RUNS)
}

/// Does the demand for a product call for its blueprint ahead of the
/// ladder? A doctrine wants it (`wanted_units > 0`), or the pressure has
/// been at least [`FE_DEMAND_BUY`] for [`FE_DEMAND_CYCLES`] cycles.
pub fn pulls(wanted_units: i64, streak: u32) -> bool {
    wanted_units > 0 || streak >= FE_DEMAND_CYCLES
}

/// **Demand first**: `rungs` with those `demanded` accepts moved to the
/// front, the order kept within each half (the ladder's, and the demand
/// order among the demanded).
pub fn pull_demanded(rungs: Vec<Vec<i64>>, demanded: &dyn Fn(&[i64]) -> bool) -> Vec<Vec<i64>> {
    let (mut front, back): (Vec<_>, Vec<_>) = rungs.into_iter().partition(|r| demanded(r));
    front.extend(back);
    front
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRODUCT_A: i64 = 991_100_001;
    const PRODUCT_B: i64 = 991_100_002;
    const INPUT: i64 = 991_100_101;
    const BP_A: i64 = 991_100_201;
    const BP_B: i64 = 991_100_202;

    fn bp(id: i64, product: i64, qty: i64, input_qty: i64, time: i64) -> Blueprint {
        Blueprint {
            blueprint_type_id: id,
            product_type_id: product,
            product_quantity: qty,
            time,
            max_production_limit: 100,
            materials: vec![(INPUT, input_qty)],
            skills: vec![],
        }
    }

    fn mkt(a: f64, b: f64, input: f64) -> BTreeMap<i64, f64> {
        BTreeMap::from([(PRODUCT_A, a), (PRODUCT_B, b), (INPUT, input)])
    }

    fn rated(bp: &Blueprint, m: &BTreeMap<i64, f64>, pressure: f64) -> Option<Appraisal> {
        appraise(bp, 0, 1, m, pressure, Rates::UNSKILLED)
    }

    #[test]
    fn chooses_the_job_with_the_best_profit_per_hour() {
        // Same inputs; B pays more per hour although A pays more per run.
        let a = bp(BP_A, PRODUCT_A, 1, 10, 7_200);
        let b = bp(BP_B, PRODUCT_B, 1, 10, 1_800);
        let m = mkt(1_000.0, 800.0, 10.0);
        let (ra, rb) = (rated(&a, &m, 0.0).unwrap(), rated(&b, &m, 0.0).unwrap());
        assert!(ra.profit_per_run > rb.profit_per_run);
        let ranked = order_jobs(vec![("a", ra, 5), ("b", rb, 0)]);
        assert_eq!(ranked.iter().map(|r| r.0).collect::<Vec<_>>(), ["b", "a"]);
    }

    #[test]
    fn negative_margin_job_is_skipped() {
        let a = bp(BP_A, PRODUCT_A, 1, 100, 3_600);
        let m = mkt(50.0, 50.0, 10.0);
        let r = rated(&a, &m, 0.0).unwrap();
        assert!(r.profit_per_hour < 0.0);
        assert!(order_jobs(vec![((), r, 0)]).is_empty());
    }

    #[test]
    fn a_missing_price_is_no_appraisal() {
        let a = bp(BP_A, PRODUCT_A, 1, 10, 3_600);
        let mut m = mkt(100.0, 100.0, 1.0);
        m.remove(&INPUT);
        assert!(rated(&a, &m, 0.0).is_none());
    }

    #[test]
    fn two_free_slots_two_jobs() {
        let a = bp(BP_A, PRODUCT_A, 1, 10, 3_600);
        let b = bp(BP_B, PRODUCT_B, 1, 10, 3_600);
        let m = mkt(500.0, 400.0, 10.0);
        let ranked = order_jobs(vec![
            (1, rated(&a, &m, 0.0).unwrap(), 0),
            (2, rated(&b, &m, 0.0).unwrap(), 0),
            // A second original of A's blueprint is the same choice.
            (3, rated(&a, &m, 0.0).unwrap(), 0),
        ]);
        assert_eq!(jobs_to_start(&ranked, 2).len(), 2);
        assert_eq!(jobs_to_start(&ranked, 1).len(), 1);
        assert_eq!(jobs_to_start(&ranked, 5).len(), 2, "each product once");
        assert!(jobs_to_start(&ranked, 0).is_empty());
    }

    #[test]
    fn demand_pressure_tilts_the_choice_within_its_bound() {
        let a = bp(BP_A, PRODUCT_A, 1, 10, 3_600);
        let m = mkt(500.0, 500.0, 10.0);
        let cold = rated(&a, &m, -1.0).unwrap();
        let flat = rated(&a, &m, 0.0).unwrap();
        let hot = rated(&a, &m, 1.0).unwrap();
        assert!(cold.profit_per_hour < flat.profit_per_hour);
        assert!(flat.profit_per_hour < hot.profit_per_hour);
        let net = Rates::UNSKILLED.net_of_listing(500.0);
        assert!((hot.profit_per_run - flat.profit_per_run - DEMAND_TILT * net).abs() < 1e-6);
        assert_eq!(pressure_of(10, 0), 1.0);
        assert_eq!(pressure_of(0, 0), 0.0);
        assert!(pressure_of(1, 3) < 0.0);
    }

    #[test]
    fn input_buffer_keeps_two_runs() {
        assert_eq!(buffered_runs(5), 5 * FE_INPUT_RUNS);
        assert_eq!(buffered_runs(0), 0);
        assert_eq!(buffered_runs(-3), 0);
    }

    #[test]
    fn demanded_product_pulls_its_bpo_ahead_of_the_ladder() {
        let ladder = vec![vec![1, 2], vec![3], vec![4, 5], vec![6]];
        let hot = |r: &[i64]| r.contains(&4) || r.contains(&6);
        assert_eq!(
            pull_demanded(ladder.clone(), &hot),
            vec![vec![4, 5], vec![6], vec![1, 2], vec![3]]
        );
        assert_eq!(pull_demanded(ladder.clone(), &|_| false), ladder);
        // A doctrine naming the part pulls at once; a bid streak needs its cycles.
        assert!(pulls(3, 0));
        assert!(!pulls(0, FE_DEMAND_CYCLES - 1));
        assert!(pulls(0, FE_DEMAND_CYCLES));
    }
}
