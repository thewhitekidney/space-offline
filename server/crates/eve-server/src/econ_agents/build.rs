//! **The builder's settlement** (R-ECON §1.5: "a builder's job completing").
//! The live builder's cycle (`eve-bot/src/builder.rs`), run by the server
//! against the real jobs, market and store:
//!
//! 1. **Deliver** every job of the character whose end time has passed
//!    (`Jobs::plan_delivery`, `Registry::deliver_job`: the same plan
//!    `industryManager.CompleteJob` runs). Jobs the bot started live, of any
//!    activity, are delivered too.
//! 2. **Sell the products**: a stack that is the product of some recipe is
//!    sold into the bids at the station at the floor or better, and what is
//!    left is listed as an ask at the price index (`econ_prices::average`),
//!    never under the materials' `P_ref` plus [`rules::BUILD_MARGIN`], one
//!    ask per (type, station) and within the order cap. The listing pays the
//!    broker's fee and rests in escrow, as a live listing does.
//! 3. **Build**: for each idle original in the hangar with a recipe, best
//!    product first, plan the runs ([`rules::planned_runs`]), buy what the
//!    hangar lacks from the asks here at no more than [`rules::MATERIAL_CAP_OF_REF`]
//!    of `P_ref` and within [`rules::BUILDER_WALLET_SHARE`] of the wallet,
//!    skip a recipe that does not clear [`rules::BUILD_MARGIN`], and install
//!    the runs the hangar covers (`Jobs::plan_install`, the real fee, the
//!    paced end date) until the slots are full.
//!
//! A job installed here ends in the future (`GameClock`-paced), so it is
//! **delivered by a later settlement**: a builder's day is several sweeps.
//! The hours of work are not spent: building is the slots' time, not the
//! pilot's.
//!
//! **Not settled**: standing bids for materials, repricing a listing,
//! invention and copying jobs (their delivery is, their start is not), the
//! blueprint ladder's purchases. The pilot does those live.

use super::{Agent, rules, shop};
use crate::damage::BlueTime;
use crate::industry;
use crate::item::Slot;
use crate::service::Registry;
use std::collections::BTreeMap;

/// Settle one builder.
pub(super) fn settle(
    reg: &mut Registry,
    character: i64,
    a: &Agent,
    _now: BlueTime,
    notes: &mut Vec<String>,
) {
    let real_now = crate::pyutil::filetime_now();
    deliver(reg, character, real_now, notes);
    sell_products(reg, character, a, notes);
    install(reg, character, a, real_now, notes);
}

/// Deliver every ready job of `character`.
fn deliver(reg: &mut Registry, character: i64, now: i64, notes: &mut Vec<String>) {
    let ready: Vec<i64> = reg
        .jobs()
        .for_owner(character, false)
        .into_iter()
        .filter(|j| !j.completed() && j.ready_at(now))
        .map(|j| j.job_id)
        .collect();
    let mut done = 0;
    for id in ready {
        let planned = reg
            .jobs()
            .plan_delivery(reg.content().get(), reg.content(), id, now);
        match planned {
            Ok((delivery, job)) => match reg.deliver_job(delivery, job) {
                Ok(()) => done += 1,
                Err(e) => notes.push(format!("job {id} not delivered: {e}")),
            },
            Err(r) => notes.push(format!("job {id} not delivered: {r:?}")),
        }
    }
    if done > 0 {
        notes.push(format!("{done} jobs delivered"));
    }
}

/// The ISK of materials one unit of `bp`'s product costs at `P_ref`.
fn unit_cost(reg: &Registry, bp: &industry::Blueprint) -> f64 {
    let sum: f64 = bp
        .materials
        .iter()
        .map(|&(t, q)| q as f64 * crate::econ_prices::p_ref(reg, t).unwrap_or(0.0))
        .sum();
    sum / bp.product_quantity.max(1) as f64
}

/// Sell the product stacks into bids, then list the rest.
fn sell_products(reg: &mut Registry, character: i64, a: &Agent, notes: &mut Vec<String>) {
    let table = reg.content().get::<industry::Table>();
    let products: BTreeMap<i64, f64> = shop::stacks(reg, character, a.station)
        .iter()
        .filter_map(|i| {
            let bp = table.by_product(i.type_id)?;
            Some((i.type_id, unit_cost(reg, bp)))
        })
        .collect();
    if products.is_empty() {
        return;
    }
    shop::sell_where(
        reg,
        character,
        a.station,
        |i| products.contains_key(&i.type_id),
        notes,
    );
    shop::list_where(
        reg,
        character,
        a.station,
        |i| products.contains_key(&i.type_id),
        |i| products.get(&i.type_id).copied().unwrap_or(0.0) * (1.0 + rules::BUILD_MARGIN),
        notes,
    );
}

/// What the hangar lacks for `runs` of `bp` at `me`: `(type, units)`.
fn shortfall(
    bp: &industry::Blueprint,
    runs: i64,
    me: i64,
    held: &BTreeMap<i64, i64>,
) -> Vec<(i64, i64)> {
    industry::job_materials(bp, runs, me, 1.0)
        .into_iter()
        .map(|m| {
            (
                m.type_id,
                m.quantity - held.get(&m.type_id).copied().unwrap_or(0),
            )
        })
        .filter(|&(_, short)| short > 0)
        .collect()
}

fn held(reg: &Registry, character: i64, station: i64) -> BTreeMap<i64, i64> {
    let mut out: BTreeMap<i64, i64> = BTreeMap::new();
    for i in reg.store().at(shop::slot(character, station)) {
        if !i.singleton {
            *out.entry(i.type_id).or_default() += i.quantity;
        }
    }
    out
}

/// One idle original the character can build from.
struct Candidate {
    item: industry::BlueprintItem,
    value: f64,
}

/// The idle originals in the hangar with a recipe, best product first.
fn candidates(reg: &Registry, character: i64, station: i64) -> Vec<Candidate> {
    let table = reg.content().get::<industry::Table>();
    let mut out: Vec<Candidate> = reg
        .store()
        .at(Slot::new(character, station, crate::model::FLAG_HANGAR))
        .filter_map(|i| {
            let item = reg.jobs().blueprint_item(table, i)?;
            let bp = table.find(item.type_id)?;
            (item.original() && reg.jobs().job_for_blueprint(item.item_id).is_none()).then(|| {
                Candidate {
                    item,
                    value: bp.product_quantity as f64
                        * crate::econ_prices::p_ref(reg, bp.product_type_id).unwrap_or(0.0),
                }
            })
        })
        .collect();
    out.sort_by(|x, y| {
        y.value
            .total_cmp(&x.value)
            .then(x.item.type_id.cmp(&y.item.type_id))
            .then(x.item.item_id.cmp(&y.item.item_id))
    });
    out
}

/// Install jobs until the slots are full or nothing is worth building.
fn install(reg: &mut Registry, character: i64, a: &Agent, now: i64, notes: &mut Vec<String>) {
    let mut started = 0;
    for cand in candidates(reg, character, a.station) {
        let table = reg.content().get::<industry::Table>();
        let Some(bp) = table.find(cand.item.type_id).cloned() else {
            continue;
        };
        let me = cand.item.material_efficiency;
        let planned = rules::planned_runs(bp.time);
        let wallet = reg.character(character).map_or(0.0, |c| c.balance);
        let held_now = held(reg, character, a.station);
        // The most a material may cost, by type: a type with no `P_ref` has
        // no price to buy at, so a recipe that needs it costs infinity.
        let caps: BTreeMap<i64, f64> = bp
            .materials
            .iter()
            .filter_map(|&(t, _)| {
                Some((
                    t,
                    crate::econ_prices::p_ref(reg, t)? * rules::MATERIAL_CAP_OF_REF,
                ))
            })
            .collect();
        let cost = |runs: i64| -> f64 {
            shortfall(&bp, runs, me, &held_now)
                .iter()
                .map(|&(t, n)| n as f64 * caps.get(&t).copied().unwrap_or(f64::INFINITY))
                .sum()
        };
        let budget = wallet * rules::BUILDER_WALLET_SHARE;
        let on_hand = rules::most_runs(planned, |r| shortfall(&bp, r, me, &held_now).is_empty());
        let runs_plan = rules::most_runs(planned, |r| cost(r) <= budget).max(on_hand);
        if runs_plan == 0 {
            notes.push(format!(
                "blueprint {}: materials beyond the budget",
                cand.item.type_id
            ));
            continue;
        }
        let materials_value: f64 = industry::job_materials(&bp, runs_plan, me, 1.0)
            .iter()
            .map(|m| m.quantity as f64 * crate::econ_prices::p_ref(reg, m.type_id).unwrap_or(0.0))
            .sum();
        let product_value = cand.value / bp.product_quantity.max(1) as f64
            * (bp.product_quantity * runs_plan) as f64;
        if !rules::worth_building(product_value, materials_value) {
            notes.push(format!("blueprint {}: no margin", cand.item.type_id));
            continue;
        }
        let now_market = crate::pyutil::filetime_now();
        for (t, short) in shortfall(&bp, runs_plan, me, &held_now) {
            let Some(&cap) = caps.get(&t) else { continue };
            let _ = reg.settle_buy(character, t, cap, short, now_market);
        }
        let held_after = held(reg, character, a.station);
        let runs = rules::most_runs(runs_plan, |r| shortfall(&bp, r, me, &held_after).is_empty());
        if runs == 0 {
            notes.push(format!(
                "blueprint {}: short of materials",
                cand.item.type_id
            ));
            continue;
        }
        match install_one(reg, character, a.station, &cand.item, runs, now) {
            Ok(job) => {
                started += 1;
                notes.push(format!(
                    "job {job}: {runs} runs of blueprint {}",
                    cand.item.type_id
                ));
            }
            Err(why) => {
                notes.push(format!(
                    "blueprint {}: not installed: {why}",
                    cand.item.type_id
                ));
                if why.starts_with("SlotsFull") {
                    break;
                }
            }
        }
    }
    if started == 0 && candidates(reg, character, a.station).is_empty() {
        notes.push("no idle original to build from".to_owned());
    }
}

/// `InstallJob` for `runs` of `bp` at the station: the same `plan_install`
/// and `install_job` the client's call runs.
fn install_one(
    reg: &mut Registry,
    character: i64,
    station: i64,
    bp: &industry::BlueprintItem,
    runs: i64,
    now: i64,
) -> Result<i64, String> {
    let Some(facility) = reg.industry_facility(Some(character)) else {
        return Err("no facility".to_owned());
    };
    let request = industry::InstallRequest {
        activity_id: industry::MANUFACTURING,
        blueprint_id: bp.item_id,
        runs,
        input_location_id: station,
        input_flag_id: crate::model::FLAG_HANGAR,
        output_location_id: station,
        output_flag_id: crate::model::FLAG_HANGAR,
        licensed_runs: 0,
        product_type_id: None,
        decryptor: None,
    };
    let on_hand: Vec<industry::Stack> = reg
        .store()
        .items()
        .iter()
        .filter(|i| i.owner_id == character && i.location_id == station)
        .map(industry::Stack::of)
        .collect();
    let clock = crate::game_clock::of(reg);
    let install = {
        let pilot = industry::Pilot {
            character_id: character,
            corporation_id: None,
            skills: reg.skills_for(Some(character)),
            implants: &[],
            balance: reg.character(character).map_or(0.0, |c| c.balance),
        };
        reg.jobs()
            .plan_install(
                reg.content().get(),
                reg.content(),
                now,
                &pilot,
                &facility,
                bp,
                &request,
                &on_hand,
            )
            .map_err(|r| format!("{r:?}"))?
            .paced(clock)
    };
    reg.install_job(install).map_err(|e| e.to_string())
}
