//! **Tech III for the bots** (lane `t3-h`, R-T3 packet T3-H): relics in, a
//! strategic cruiser or a tactical destroyer out, in a closed world where
//! nothing appears from nowhere.
//!
//! The chain, and who does each link (each one is the player's own call):
//!
//! 1. **Explorers** hack ancient sites (HK3) and sell what the cans hold:
//!    relics (category 34) and the Subsystems Engineering datacores
//!    (`hacking_loot`, class `t3_relic` / `t3_datacore`).
//! 2. **A builder with a free laboratory slot** ([`lab`], called from
//!    `builder::run_with` before the T2 laboratory) picks one **doctrine
//!    kit** ([`doctrine`]) and, piece by piece, what that kit still lacks:
//!    the hull's blueprint copy and, for a strategic cruiser, one subsystem
//!    copy per slot flag. For each missing piece it buys a relic of the family
//!    that invents it, buys the datacores, learns the skills, and installs
//!    the invention with `productTypeID` ([`plan_step`], pure).
//! 3. **The same builder builds** the copies through the T2 path (T3-F): the
//!    hull and the subsystems are made of Hybrid Tech Components, so the
//!    reaction chain and the explorers' salvage still stand behind them.
//! 4. **A combat pilot docked without a hull** ([`pilot_step`], called from
//!    `drone_use::hull_in` ahead of the ordinary chooser) assembles a full kit
//!    it holds -- `ship.AssembleShip(hull, name, subSystems=[four])` -- and
//!    boards; with none, it buys the cheapest full kit the asks at its station
//!    offer inside [`SET_SHARE`] of its wallet, once its skills allow
//!    ([`lacking_skills`]; the server does not check them at boarding, so
//!    this is policy). A tactical destroyer is the same with no subsystems.
//!
//! **Nothing is named here by id.** Hulls, subsystems and the groups they sit
//! in are found in the type table by group and category ([`catalog`]); the
//! relic that invents a piece is found in the recipes. A world without the
//! bundle has no kit, so none of this runs.
//!
//! **Honest rate**: `docs/lanes/t3-h.md`. One kit needs five successful
//! inventions (the hull and four subsystems), each relic succeeding about a
//! quarter to a third of the time, each attempt costing six datacores of
//! which three are Subsystems Engineering cores that only ancient cans drop.

use crate::brain::Act;
use crate::builder::{
    self, BROKER_FEE, Cycle, INVENTION_MIN_WALLET, LAB_SLOTS, WALLET_SHARE, held_by_type, noted,
};
use crate::pricing::Prices;
use crate::verbs::BlueprintRow;
use crate::{Conn, Error, ItemRow, Result, acquire};
use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::{drone_skills, industry, stance, subsystems, typedata};
use std::collections::{BTreeMap, BTreeSet};

/// Invention attempts one T3 job takes at most **[A]**: a relic stack is
/// rare and a job holds the slot for `runs x 1 h`, so a few attempts at a
/// time. Falsified by relic stacks left idle (too low) or datacores wasted on
/// a doctrine that changed (too high).
pub const T3_BATCH: i64 = 3;
/// How many kits (lowest hull type id first) share the doctrine **[A]**:
/// builders spread over this many, so a pilot finds the pieces of one hull at
/// a station. Falsified by pilots that find a hull but no subsystems (too
/// many) or by one doctrine hull in every fit (too few).
pub const DOCTRINE_HULLS: usize = 2;
/// The most of its wallet a pilot spends on a whole kit **[A]**.
pub const SET_SHARE: f64 = 0.8;

/// One thing a pilot can fly that Tech III makes: a hull and, for a strategic
/// cruiser, the subsystems that fit it, per slot flag in order
/// (`subsystems::FIRST_FLAG`..). A tactical destroyer has no subsystems.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kit {
    pub hull: i64,
    /// One list per slot flag for a modular hull, else empty.
    pub subs: Vec<Vec<i64>>,
}

impl Kit {
    /// A strategic cruiser (with subsystems).
    pub fn modular(&self) -> bool {
        !self.subs.is_empty()
    }

    /// The pieces to own, each a list of item types any one of which will do:
    /// the hull, then one of each slot's subsystems.
    pub fn pieces(&self) -> Vec<Vec<i64>> {
        std::iter::once(vec![self.hull])
            .chain(self.subs.iter().cloned())
            .collect()
    }
}

/// **Every kit the type table can make** (pure), by hull type id: a published
/// hull of the strategic-cruiser group whose four slot flags each have a
/// published subsystem (`fitsToShipType`, `subSystemSlot`), and every
/// published tactical destroyer. A modular hull missing a slot's subsystem is
/// left out: nobody could fly it.
pub fn catalog(content: &Content) -> Vec<Kit> {
    let table = typedata::installed_in(content).table();
    let mut kits: BTreeMap<i64, Kit> = BTreeMap::new();
    for row in table.types().filter(|r| r.published) {
        if row.group_id == subsystems::GROUP_STRATEGIC_CRUISER {
            kits.insert(
                row.type_id,
                Kit {
                    hull: row.type_id,
                    subs: vec![Vec::new(); subsystems::VISIBLE as usize],
                },
            );
        } else if row.group_id == stance::GROUP_TACTICAL_DESTROYER {
            kits.insert(
                row.type_id,
                Kit {
                    hull: row.type_id,
                    subs: Vec::new(),
                },
            );
        }
    }
    for row in table
        .types()
        .filter(|r| r.published && r.category_id == subsystems::CATEGORY_SUBSYSTEM)
    {
        let (Some(hull), Some(flag)) = (
            subsystems::fits_ship_type(content, row.type_id),
            subsystems::slot_flag(content, row.type_id),
        ) else {
            continue;
        };
        if let Some(kit) = kits.get_mut(&hull)
            && let Some(slot) = kit.subs.get_mut((flag - subsystems::FIRST_FLAG) as usize)
        {
            slot.push(row.type_id);
        }
    }
    kits.into_values()
        .filter(|k| !k.modular() || k.subs.iter().all(|s| !s.is_empty()))
        .collect()
}

// ---- the inventor (a builder's laboratory) ----------------------------------------

/// Item type to the blueprint type whose invention a relic makes
/// ([`builder::t3_types`] through each blueprint's manufacturing product).
pub fn inventable(recipes: &industry::Table) -> BTreeMap<i64, i64> {
    builder::t3_types(recipes)
        .into_iter()
        .filter_map(|bp| Some((recipes.find(bp)?.product_type_id, bp)))
        .collect()
}

/// Whether the builder already has `piece`: one of its item types in the
/// hangar, or a copy with runs left of a blueprint that makes one.
fn covered(
    piece: &[i64],
    recipes: &industry::Table,
    blueprints: &[BlueprintRow],
    held: &BTreeMap<i64, i64>,
) -> bool {
    piece.iter().any(|t| held.get(t).copied().unwrap_or(0) > 0)
        || blueprints.iter().any(|b| {
            !b.is_original()
                && b.runs > 0
                && recipes
                    .find(b.type_id)
                    .is_some_and(|bp| piece.contains(&bp.product_type_id))
        })
}

/// **The doctrine** (pure): the kit this builder works towards. Feasible
/// kits are those whose every piece is covered or inventable; of them the
/// most covered wins, so a builder finishes what it began; ties go to the
/// class the seed likes (even: a strategic cruiser, odd: a destroyer) and then
/// to one of the first [`DOCTRINE_HULLS`] by hull id, by the seed.
pub fn doctrine<'a>(
    kits: &'a [Kit],
    recipes: &industry::Table,
    blueprints: &[BlueprintRow],
    held: &BTreeMap<i64, i64>,
    seed: u64,
) -> Option<&'a Kit> {
    let can = inventable(recipes);
    let mut scored: Vec<(usize, &Kit)> = kits
        .iter()
        .filter_map(|k| {
            let mut done = 0;
            for piece in k.pieces() {
                if covered(&piece, recipes, blueprints, held) {
                    done += 1;
                } else if !piece.iter().any(|t| can.contains_key(t)) {
                    return None;
                }
            }
            Some((done, k))
        })
        .collect();
    let best = scored.iter().map(|(d, _)| *d).max()?;
    scored.retain(|(d, _)| *d == best);
    let prefer_modular = seed.is_multiple_of(2);
    if scored.iter().any(|(_, k)| k.modular() == prefer_modular) {
        scored.retain(|(_, k)| k.modular() == prefer_modular);
    }
    scored.sort_by_key(|(_, k)| k.hull);
    let n = scored.len().clamp(1, DOCTRINE_HULLS);
    scored.get((seed / 2) as usize % n).map(|(_, k)| *k)
}

/// The blueprint types `kit` still lacks, in piece order: for each missing
/// piece the first of its item types that some relic can invent.
pub fn wants(
    kit: &Kit,
    recipes: &industry::Table,
    blueprints: &[BlueprintRow],
    held: &BTreeMap<i64, i64>,
) -> Vec<i64> {
    let can = inventable(recipes);
    kit.pieces()
        .iter()
        .filter(|p| !covered(p, recipes, blueprints, held))
        .filter_map(|p| p.iter().find_map(|t| can.get(t).copied()))
        .collect()
}

/// The relic types whose invention can make `blueprint`, best first: the most
/// runs on the copy, then the lowest type id.
pub fn relics_for(recipes: &industry::Table, blueprint: i64) -> Vec<i64> {
    let mut v: Vec<(i64, i64)> = recipes
        .relics
        .iter()
        .filter_map(|r| {
            let p = r
                .invention()
                .products
                .iter()
                .find(|p| p.type_id == blueprint)?;
            Some((-p.quantity, r.blueprint.blueprint_type_id))
        })
        .collect();
    v.sort_unstable();
    v.into_iter().map(|(_, t)| t).collect()
}

/// What the laboratory does next for the doctrine.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Invent `product` from this relic stack, `runs` attempts.
    Invent {
        relic: BlueprintRow,
        product: i64,
        runs: i64,
    },
    /// A relic is held but its datacores are short: buy `(type, units)`.
    Datacores(Vec<(i64, i64)>),
    /// No relic of the family is held: buy one of these types, best first.
    Relic(Vec<i64>),
}

/// **The next step** (pure). An invention the hangar can already run beats
/// buying datacores, which beats buying a relic.
pub fn plan_step(
    kits: &[Kit],
    recipes: &industry::Table,
    station: i64,
    blueprints: &[BlueprintRow],
    held: &BTreeMap<i64, i64>,
    seed: u64,
) -> Option<Step> {
    let kit = doctrine(kits, recipes, blueprints, held, seed)?;
    let mut buy_cores: Option<Step> = None;
    let mut buy_relic: Option<Step> = None;
    for want in wants(kit, recipes, blueprints, held) {
        let families = relics_for(recipes, want);
        let row = families.iter().find_map(|t| {
            blueprints
                .iter()
                .find(|b| {
                    b.type_id == *t && b.location_id == station && b.runs > 0 && b.job_id.is_none()
                })
                .copied()
        });
        let Some(row) = row else {
            buy_relic.get_or_insert(Step::Relic(families));
            continue;
        };
        let Some(relic) = recipes.relic(row.type_id) else {
            continue;
        };
        let inv = relic.invention();
        let runs = builder::datacore_sets(inv, held, row.runs.min(T3_BATCH));
        if runs > 0 {
            return Some(Step::Invent {
                relic: row,
                product: want,
                runs,
            });
        }
        let attempts = row.runs.min(T3_BATCH);
        let short: Vec<(i64, i64)> = inv
            .materials
            .iter()
            .map(|&(t, per)| (t, per * attempts - held.get(&t).copied().unwrap_or(0)))
            .filter(|&(_, n)| n > 0)
            .collect();
        buy_cores.get_or_insert(Step::Datacores(short));
    }
    buy_cores.or(buy_relic)
}

/// **The laboratory's T3 work, over the wire**: with a free lab slot and
/// [`INVENTION_MIN_WALLET`], run [`plan_step`] and do it. `true`: something
/// was done (a job installed, or goods bought for the next pass). A refusal
/// is a note; a refused invention learns its skills from the relic's recipe.
pub fn lab(
    conn: &mut Conn,
    station: i64,
    recipes: &industry::Table,
    content: &Content,
    prices: &mut Prices,
    cycle: &mut Cycle,
) -> Result<bool> {
    let kits = catalog(content);
    if kits.is_empty() || recipes.relics.is_empty() {
        return Ok(false);
    }
    let Some(jobs) = noted(conn.industry_jobs(), cycle, "t3 jobs")? else {
        return Ok(false);
    };
    let busy = jobs
        .iter()
        .filter(|j| !j.completed() && industry::is_science(j.activity_id))
        .count();
    if busy >= LAB_SLOTS || conn.cash_balance()? < INVENTION_MIN_WALLET {
        return Ok(false);
    }
    let seed = conn.session().char_id().unwrap_or(0) as u64;
    // Buying, then installing in the same cycle: at most three passes
    // (a relic, its datacores, the job).
    for pass in 0..3 {
        let Some(blueprints) = noted(conn.blueprints(), cycle, "t3 blueprints")? else {
            return Ok(false);
        };
        let held = held_by_type(&conn.hangar_items()?);
        if pass == 0
            && let Some(note) = kit_note(&kits, recipes, &blueprints, &held, seed)
        {
            cycle.notes.push(note);
        }
        let Some(step) = plan_step(&kits, recipes, station, &blueprints, &held, seed) else {
            return reaction_step(conn, station, recipes, &kits, content, prices, cycle);
        };
        match step {
            Step::Invent {
                relic,
                product,
                runs,
            } => return invent(conn, recipes, cycle, &relic, product, runs),
            Step::Datacores(short) => {
                if !buy(conn, station, prices, &short, cycle)? {
                    return Ok(false);
                }
            }
            Step::Relic(types) => {
                let mut got = false;
                for t in types {
                    if buy(conn, station, prices, &[(t, 1)], cycle)? {
                        got = true;
                        break;
                    }
                }
                if !got {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

fn invent(
    conn: &mut Conn,
    recipes: &industry::Table,
    cycle: &mut Cycle,
    relic: &BlueprintRow,
    product: i64,
    runs: i64,
) -> Result<bool> {
    match builder::install_lab_job(conn, relic, industry::INVENTION, runs, None, Some(product)) {
        Ok(job_id) => {
            cycle
                .lab
                .push((job_id, relic.type_id, industry::INVENTION, runs));
            Ok(true)
        }
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => {
            cycle.notes.push(format!("t3 lab {}: {e}", relic.type_id));
            if let Some(r) = recipes.relic(relic.type_id)
                && let Some(got) = noted(
                    acquire::learn_skills(conn, &r.invention().skills, None),
                    cycle,
                    "t3 skills",
                )?
            {
                cycle.notes.extend(got.missing);
            }
            Ok(false)
        }
    }
}

/// Buy each `(type, units)` from asks here under the consumer's cap and
/// [`WALLET_SHARE`] of the wallet; `true` when anything filled.
fn buy(
    conn: &mut Conn,
    station: i64,
    prices: &mut Prices,
    lines: &[(i64, i64)],
    cycle: &mut Cycle,
) -> Result<bool> {
    let mut any = false;
    for &(t, n) in lines {
        let cap = prices.reservation(conn, t)?;
        let book = conn.market_orders(t)?;
        let isk = conn.cash_balance()? * WALLET_SHARE;
        for (ask, units) in acquire::plan(&book, station, cap, isk, n) {
            let Some(row) = noted(
                conn.buy(station, t, ask.price, units, 0, BROKER_FEE),
                cycle,
                "t3 goods",
            )?
            else {
                break;
            };
            let got = units - row.vol_remaining.clamp(0, units);
            if got > 0 {
                any = true;
                cycle.bought.push((t, got, got as f64 * ask.price));
            }
        }
    }
    Ok(any)
}

// ---- the kit's bill, and the builder's own reactions ------------------------------

/// How many levels of manufacturing the bill follows (hull, component,
/// sub-component) before a material counts as raw.
pub const BILL_DEPTH: usize = 4;
/// The most runs of a reaction a builder installs for its own kit at once
/// **[A]**: a kit needs a few hundred units of a polymer, and a run yields
/// tens. Falsified by builders whose reaction jobs idle for want of inputs
/// (too high) or that reinstall every cycle (too low).
pub const REACTION_BATCH: i64 = 5;

/// **What one run of each piece of `kit` eats, down to raw materials** (pure),
/// less what the hangar `held` already covers at each level: `type -> units
/// still missing`. A piece is built from the first of its item types that a
/// manufacturing blueprint makes; a material that a blueprint makes is
/// expanded (a component), a reaction product or a raw good is a leaf.
pub fn bill(kit: &Kit, recipes: &industry::Table, held: &BTreeMap<i64, i64>) -> BTreeMap<i64, i64> {
    let mut out = BTreeMap::new();
    let mut left = held.clone();
    for piece in kit.pieces() {
        if let Some(t) = piece.iter().find(|t| recipes.by_product(**t).is_some()) {
            expand(recipes, *t, 1, 0, &mut left, &mut out);
        }
    }
    out
}

fn expand(
    recipes: &industry::Table,
    type_id: i64,
    units: i64,
    depth: usize,
    left: &mut BTreeMap<i64, i64>,
    out: &mut BTreeMap<i64, i64>,
) {
    let have = left.entry(type_id).or_insert(0);
    let used = (*have).min(units);
    *have -= used;
    let units = units - used;
    if units <= 0 {
        return;
    }
    match recipes.by_product(type_id) {
        Some(bp) if depth < BILL_DEPTH => {
            let runs = (units + bp.product_quantity.max(1) - 1) / bp.product_quantity.max(1);
            for &(m, q) in &bp.materials {
                expand(recipes, m, q * runs, depth + 1, left, out);
            }
        }
        _ => *out.entry(type_id).or_insert(0) += units,
    }
}

/// **The `[t3] kit` note** (pure): the doctrine kit's missing raw goods by
/// type, most first, or `None` with no kit. T3-ANCIENT-RATE's per-builder
/// line: the per-type spread may leave one type short long before the mean
/// says done.
pub fn kit_note(
    kits: &[Kit],
    recipes: &industry::Table,
    blueprints: &[BlueprintRow],
    held: &BTreeMap<i64, i64>,
    seed: u64,
) -> Option<String> {
    let kit = doctrine(kits, recipes, blueprints, held, seed)?;
    let mut missing: Vec<(i64, i64)> = bill(kit, recipes, held).into_iter().collect();
    missing.sort_by_key(|&(t, n)| (-n, t));
    let list: Vec<String> = missing.iter().map(|(t, n)| format!("{t}={n}")).collect();
    Some(format!(
        "[t3] kit {} missing {} types: {}",
        kit.hull,
        missing.len(),
        list.join(" ")
    ))
}

/// What the builder does for a reaction product its kit needs.
#[derive(Debug, Clone, PartialEq)]
pub enum Make {
    /// Install `runs` of this formula.
    Install { formula: BlueprintRow, runs: i64 },
    /// The formula is held but its inputs are short: buy `(type, units)`.
    Inputs(Vec<(i64, i64)>),
    /// No formula held: buy this blueprint type.
    Formula(i64),
}

/// **The builder's own reaction** (pure): for each reaction product in
/// `need` (`type -> units missing`) that nobody `sold` offers, in type order,
/// run a formula it holds with the inputs on hand, else buy the inputs, else
/// buy the formula. Installing beats buying inputs beats buying a formula.
pub fn plan_reaction(
    need: &BTreeMap<i64, i64>,
    recipes: &industry::Table,
    station: i64,
    blueprints: &[BlueprintRow],
    held: &BTreeMap<i64, i64>,
    sold: &BTreeSet<i64>,
) -> Option<Make> {
    let mut buy_inputs: Option<Make> = None;
    let mut buy_formula: Option<Make> = None;
    for (&t, &short) in need {
        if short <= 0 || sold.contains(&t) {
            continue;
        }
        let Some(bp) = recipes.reactions.iter().find(|b| b.product_type_id == t) else {
            continue;
        };
        let Some(formula) = blueprints.iter().find(|b| {
            b.type_id == bp.blueprint_type_id && b.location_id == station && b.job_id.is_none()
        }) else {
            buy_formula.get_or_insert(Make::Formula(bp.blueprint_type_id));
            continue;
        };
        let wanted = ((short + bp.product_quantity.max(1) - 1) / bp.product_quantity.max(1))
            .clamp(1, REACTION_BATCH);
        let runs = builder::runs_on_hand(bp, wanted, 0, held);
        if runs > 0 {
            return Some(Make::Install {
                formula: *formula,
                runs,
            });
        }
        buy_inputs.get_or_insert(Make::Inputs(builder::shortfall(bp, wanted, 0, held)));
    }
    buy_inputs.or(buy_formula)
}

/// **The reaction step, over the wire** (T3-POLYMER-PRICE): the doctrine
/// kit's missing reaction products that no seller offers are made here, by
/// the builder, when no reactor does. `true`: something was done.
fn reaction_step(
    conn: &mut Conn,
    station: i64,
    recipes: &industry::Table,
    kits: &[Kit],
    content: &Content,
    prices: &mut Prices,
    cycle: &mut Cycle,
) -> Result<bool> {
    crate::gas_price::fill(prices, Some(content));
    let Some(blueprints) = noted(conn.blueprints(), cycle, "t3 blueprints")? else {
        return Ok(false);
    };
    let held = held_by_type(&conn.hangar_items()?);
    let seed = conn.session().char_id().unwrap_or(0) as u64;
    let Some(kit) = doctrine(kits, recipes, &blueprints, &held, seed) else {
        return Ok(false);
    };
    let products: BTreeSet<i64> = recipes
        .reactions
        .iter()
        .map(|b| b.product_type_id)
        .collect();
    let need: BTreeMap<i64, i64> = bill(kit, recipes, &held)
        .into_iter()
        .filter(|(t, _)| products.contains(t))
        .collect();
    if need.is_empty() {
        return Ok(false);
    }
    let Some(jobs) = noted(conn.industry_jobs(), cycle, "t3 reactions")? else {
        return Ok(false);
    };
    let running = jobs
        .iter()
        .filter(|j| !j.completed() && j.activity_id == industry::REACTION)
        .count();
    if running >= crate::reactor::SLOTS {
        return Ok(false);
    }
    let mut sold = BTreeSet::new();
    for &t in need.keys() {
        if conn
            .market_orders(t)?
            .asks
            .iter()
            .any(|a| a.vol_remaining > 0)
        {
            sold.insert(t);
        }
    }
    match plan_reaction(&need, recipes, station, &blueprints, &held, &sold) {
        None => Ok(false),
        Some(Make::Install { formula, runs }) => {
            crate::reactor::install_reaction(conn, &formula, runs, cycle)?;
            Ok(true)
        }
        Some(Make::Inputs(short)) => buy(conn, station, prices, &short, cycle),
        Some(Make::Formula(t)) => buy(conn, station, prices, &[(t, 1)], cycle),
    }
}

// ---- the pilot --------------------------------------------------------------------

/// A kit the hangar holds whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembly {
    pub hull_type: i64,
    pub hull_item: i64,
    /// `(type, item)` per slot flag, in order; empty for a destroyer.
    pub subs: Vec<(i64, i64)>,
}

impl Assembly {
    /// Every type the pilot must be able to use.
    pub fn types(&self) -> Vec<i64> {
        std::iter::once(self.hull_type)
            .chain(self.subs.iter().map(|s| s.0))
            .collect()
    }
}

/// **The kits the hangar holds whole** (pure): a hull (packaged or
/// assembled) and, for a modular one, a subsystem for each slot. Strategic
/// cruisers first, then by hull type id. Each item is used by one kit only.
pub fn ready(kits: &[Kit], hangar: &[ItemRow]) -> Vec<Assembly> {
    let mut order: Vec<&Kit> = kits.iter().collect();
    order.sort_by_key(|k| (!k.modular(), k.hull));
    let mut used: BTreeSet<i64> = BTreeSet::new();
    let mut out = Vec::new();
    for kit in order {
        let free = |types: &[i64], category: i64, used: &BTreeSet<i64>| {
            hangar
                .iter()
                .find(|r| {
                    types.contains(&r.type_id)
                        && r.category_id == category
                        && r.flag_id == crate::FLAG_HANGAR
                        && !used.contains(&r.item_id)
                })
                .map(|r| (r.type_id, r.item_id))
        };
        let Some((hull_type, hull_item)) = free(&[kit.hull], acquire::CATEGORY_SHIP, &used) else {
            continue;
        };
        let mut taken = used.clone();
        taken.insert(hull_item);
        let mut subs = Vec::new();
        for slot in &kit.subs {
            match free(slot, subsystems::CATEGORY_SUBSYSTEM, &taken) {
                Some(s) => {
                    taken.insert(s.1);
                    subs.push(s);
                }
                None => break,
            }
        }
        if subs.len() != kit.subs.len() {
            continue;
        }
        used = taken;
        out.push(Assembly {
            hull_type,
            hull_item,
            subs,
        });
    }
    out
}

/// The `(skill, level)` the pilot still lacks to use `types` (pure over the
/// type table and the trained levels).
pub fn lacking_skills(
    content: &Content,
    types: &[i64],
    levels: &BTreeMap<i64, i64>,
) -> Vec<(i64, i64)> {
    let table = typedata::installed_in(content).table();
    let mut need: BTreeMap<i64, i64> = BTreeMap::new();
    for &t in types {
        for (skill, level) in drone_skills::required_skills(table, t) {
            let e = need.entry(skill).or_insert(0);
            *e = (*e).max(level);
        }
    }
    need.into_iter()
        .filter(|(s, l)| levels.get(s).copied().unwrap_or(0) < *l)
        .collect()
}

/// Whether the ship at `active` in `hangar` is a Tech III hull: a strategic
/// cruiser or a tactical destroyer. The escort reads this to keep it instead
/// of boarding a ranked hull.
pub fn is_flying(hangar: &[ItemRow], active: Option<i64>) -> bool {
    active.is_some_and(|a| {
        hangar.iter().any(|r| {
            r.item_id == a
                && (r.group_id == subsystems::GROUP_STRATEGIC_CRUISER
                    || r.group_id == stance::GROUP_TACTICAL_DESTROYER)
        })
    })
}

/// **Assemble a held kit and board it**, over the wire: a destroyer or a
/// packaged hull through the ordinary `AssembleShip`, a strategic cruiser
/// with `subSystems=[the four item ids]` (`invItemFunctions.py:201`). Returns
/// the boarded ship's item id, or the server's refusal as text.
pub fn assemble_and_board(
    conn: &mut Conn,
    station: i64,
    a: &Assembly,
) -> Result<std::result::Result<i64, String>> {
    let mut kwargs = vec![(PyValue::None, crate::bstr("name"))];
    if !a.subs.is_empty() {
        kwargs.push((
            PyValue::List(a.subs.iter().map(|s| PyValue::Int(s.1)).collect()),
            crate::bstr("subSystems"),
        ));
    }
    let answer = conn.moniker_call(
        "ship",
        crate::station_moniker(station),
        "AssembleShip",
        vec![PyValue::List(vec![PyValue::Int(a.hull_item)])],
        kwargs,
    )?;
    if let Some(key) = answer.outcome.refusal() {
        return Ok(Err(format!("assembling {}: {key}", a.hull_item)));
    }
    // A stack of one assembles in place; a bigger stack makes a new ship.
    let ship = conn
        .hangar_items()?
        .into_iter()
        .filter(|r| r.type_id == a.hull_type && r.singleton)
        .map(|r| r.item_id)
        .max();
    let Some(ship) = ship else {
        return Ok(Err(format!("hull {} did not assemble", a.hull_item)));
    };
    match conn.board(ship) {
        Ok(_) => Ok(Ok(ship)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => Ok(Err(format!("boarding {ship}: {e}"))),
    }
}

/// What [`pilot_step`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Piloted {
    /// The pilot now sits in a Tech III hull: the ordinary chooser stops.
    pub flying: bool,
    /// The weapon system of the kit it boarded ([`kit_weapon`]).
    pub weapon: Option<crate::hull_choice::Weapon>,
    pub note: String,
}

/// **A docked combat pilot's Tech III step**, ahead of the ordinary hull
/// chooser: assemble and board a kit it holds (once its skills allow, else
/// learn them), else buy the cheapest kit the asks here offer inside
/// [`SET_SHARE`] of the wallet (again only with the skills), then assemble.
pub fn pilot_step(conn: &mut Conn, content: &Content, seed: u64) -> Result<Piloted> {
    let kits = catalog(content);
    let Some(station) = conn.session().station_id() else {
        return Ok(Piloted::default());
    };
    if kits.is_empty() {
        return Ok(Piloted::default());
    }
    let levels = acquire::skill_levels(conn)?;
    let mut out = Piloted::default();
    for round in 0..2 {
        let hangar = conn.hangar_items()?;
        if let Some(a) = ready(&kits, &hangar).into_iter().find(|a| {
            // A kit whose skills the pilot lacks waits, and is learned.
            let lacking = lacking_skills(content, &a.types(), &levels);
            lacking.is_empty()
        }) {
            match assemble_and_board(conn, station, &a)? {
                Ok(ship) => {
                    out.flying = true;
                    let subs: Vec<i64> = a.subs.iter().map(|s| s.0).collect();
                    out.weapon = Some(kit_weapon(content, a.hull_type, &subs, seed));
                    out.note = format!("t3: assembled {} as {ship} and boarded", a.hull_type);
                }
                Err(why) => out.note = format!("t3: {why}"),
            }
            return Ok(out);
        }
        if let Some(a) = ready(&kits, &hangar).into_iter().next() {
            let lacking = lacking_skills(content, &a.types(), &levels);
            acquire::learn_skills(conn, &lacking, None)?;
            out.note = format!("t3: learning {lacking:?} for hull {}", a.hull_type);
            return Ok(out);
        }
        if round == 1 {
            break;
        }
        match buy_kit(conn, content, &kits, station, &levels)? {
            Bought::Whole => {}
            Bought::Learning(note) => {
                out.note = note;
                return Ok(out);
            }
            Bought::None => break,
        }
    }
    Ok(out)
}

/// The race of each Tech III hull a bot can fly **[A]**: the strategic
/// cruisers (Tengu, Legion, Proteus, Loki) and the tactical destroyers
/// (Jackdaw, Confessor, Hecate, Svipul), by the race that built them.
/// Falsified by a bot whose kit weapon does not match its hull's bonus.
pub const HULL_RACES: &[(i64, crate::progression::Race)] = &[
    (29_984, crate::progression::Race::Caldari),
    (29_986, crate::progression::Race::Amarr),
    (29_988, crate::progression::Race::Gallente),
    (29_990, crate::progression::Race::Minmatar),
    (34_828, crate::progression::Race::Caldari),
    (34_317, crate::progression::Race::Amarr),
    (35_683, crate::progression::Race::Gallente),
    (34_562, crate::progression::Race::Minmatar),
];

/// **The weapon system a Tech III kit carries** (pure): a kit with launcher
/// hardpoints (the hull's own plus the subsystems' `launcherHardPointModifier`)
/// fires missiles, else the turret of the hull's race; a hull of unknown race
/// takes the seed's race, with a turret in place of missiles. The ordinary
/// seed draw ([`crate::progression::rung`]) knows nothing of the kit, so a
/// turret bot in a launcher-only Tengu never fitted anything.
pub fn kit_weapon(
    content: &Content,
    hull: i64,
    subs: &[i64],
    seed: u64,
) -> crate::hull_choice::Weapon {
    use crate::hull_choice::Weapon;
    use crate::progression::{Race, weapon_of};
    let read = |t: i64| {
        typedata::attribute_in(content, t, subsystems::HARDPOINT_MODIFIERS[1].1).unwrap_or(0.0)
            as i64
    };
    let own = eve_server::fitting::hull_in(content, hull).map_or(0, |h| h.launchers);
    if own + subs.iter().map(|&s| read(s)).sum::<i64>() > 0 {
        return Weapon::Missile;
    }
    let race = HULL_RACES
        .iter()
        .find(|(t, _)| *t == hull)
        .map_or_else(|| Race::draw(seed), |(_, r)| *r);
    match weapon_of(race, seed) {
        Weapon::Missile => Weapon::Hybrid,
        w => w,
    }
}

/// **The weapon of the Tech III hull the pilot already flies** (after a
/// restart): the active ship's fitted subsystems, read over the wire.
pub fn flown_weapon(
    conn: &mut Conn,
    content: &Content,
    seed: u64,
) -> Result<Option<crate::hull_choice::Weapon>> {
    let Some(ship) = conn.session().ship_id() else {
        return Ok(None);
    };
    let hull = conn
        .hangar_items()?
        .into_iter()
        .find(|r| r.item_id == ship)
        .map(|r| r.type_id);
    let Some(hull) = hull else {
        return Ok(None);
    };
    let subs: Vec<i64> = conn
        .ship_items(ship, None)?
        .into_iter()
        .filter(|r| subsystems::is_subsystem_flag(r.flag_id))
        .map(|r| r.type_id)
        .collect();
    Ok(Some(kit_weapon(content, hull, &subs, seed)))
}

enum Bought {
    /// Every line of a kit filled.
    Whole,
    /// A kit is on offer; its skills are being learned first.
    Learning(String),
    None,
}

/// The cheapest whole kit the asks here offer, as `(type, ask price)` lines:
/// the cheapest ask per piece. `None` when any piece has no ask.
fn quote(
    conn: &mut Conn,
    prices: &mut Prices,
    kit: &Kit,
    station: i64,
    isk: f64,
) -> Result<Option<Vec<(i64, f64)>>> {
    let mut lines = Vec::new();
    for piece in kit.pieces() {
        let mut best: Option<(i64, f64)> = None;
        for t in piece {
            let cap = prices.reservation(conn, t)?;
            let book = conn.market_orders(t)?;
            if let Some((ask, _)) = acquire::plan(&book, station, cap, isk, 1).first()
                && best.is_none_or(|(_, p)| ask.price < p)
            {
                best = Some((t, ask.price));
            }
        }
        match best {
            Some(b) => lines.push(b),
            None => return Ok(None),
        }
    }
    Ok(Some(lines))
}

/// Buy the first affordable whole kit. Nothing is bought before the skills
/// allow: the pilot learns them first.
fn buy_kit(
    conn: &mut Conn,
    content: &Content,
    kits: &[Kit],
    station: i64,
    levels: &BTreeMap<i64, i64>,
) -> Result<Bought> {
    let budget = conn.cash_balance()? * SET_SHARE;
    let mut prices = Prices::read(conn)?;
    let mut order: Vec<&Kit> = kits.iter().collect();
    order.sort_by_key(|k| (!k.modular(), k.hull));
    for kit in order {
        // The hull first: one call per hull when nobody sells it.
        let hull_book = conn.market_orders(kit.hull)?;
        if acquire::plan(&hull_book, station, None, budget, 1).is_empty() {
            continue;
        }
        let Some(lines) = quote(conn, &mut prices, kit, station, budget)? else {
            continue;
        };
        if lines.iter().map(|l| l.1).sum::<f64>() > budget {
            continue;
        }
        let types: Vec<i64> = lines.iter().map(|l| l.0).collect();
        let lacking = lacking_skills(content, &types, levels);
        if !lacking.is_empty() {
            acquire::learn_skills(conn, &lacking, None)?;
            return Ok(Bought::Learning(format!(
                "t3: a kit for hull {} is on offer; learning {lacking:?} first",
                kit.hull
            )));
        }
        for &(t, price) in &lines {
            match conn.buy(station, t, price, 1, 0, BROKER_FEE) {
                Ok(_) => {}
                Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                Err(_) => return Ok(Bought::None),
            }
        }
        return Ok(Bought::Whole);
    }
    Ok(Bought::None)
}

// ---- the tactical destroyer's stance ----------------------------------------------

/// The stance a bot flying a tactical destroyer should be in (`None` for any
/// other hull): [`stance::pick`] when the hull offers it. Pure.
pub fn stance_wanted(
    content: &Content,
    hull_type: i64,
    hurt: f64,
    range_m: Option<f64>,
    reach_m: f64,
) -> Option<i64> {
    let want = stance::pick(hurt, range_m, reach_m);
    stance::stances_of(content, hull_type)
        .contains(&want)
        .then_some(want)
}

/// How often a fight asks for a stance **[A]**: the hull's switch lock is
/// `stanceSwitchTime`, 10 s on the tactical destroyers `docs/lanes/t3-e.md`
/// read, so a request each 15 s never meets it twice in a row. Falsified by
/// `ShipAlreadyChangingStance` in the act log.
pub const STANCE_EVERY: std::time::Duration = std::time::Duration::from_secs(15);

/// **The fight's stance steering** (PROG-PVE2, pure): in a Tech III hull
/// ([`crate::drone_use::Mind::t3_hull`]) with a target engaged and the bot not
/// on its way home, put one [`Act::Stance`] ahead of the acts, at most once
/// per [`STANCE_EVERY`]. The driver asks [`stance_wanted`], which knows the
/// hull and which stances it offers (a strategic cruiser offers none: the
/// request is then a quiet no-op). `hurt` is the share of shield and armour
/// lost, `range` the distance to the target.
pub fn stance_steer(
    m: &mut crate::brain::Memory,
    o: &crate::brain::Observation,
    acts: Vec<Act>,
) -> Vec<Act> {
    if !m.combat.t3_hull || m.homing.is_some() {
        return acts;
    }
    let Some(target) = m.target.as_ref().map(|(id, _)| *id) else {
        return acts;
    };
    if m.combat.stance_next.is_some_and(|t| o.now < t) {
        return acts;
    }
    let Some(park) = &o.park else {
        return acts;
    };
    let range = park
        .hostiles
        .iter()
        .find(|h| h.ball.id == target)
        .map(|h| park.ego.surface_to(&h.ball));
    let hurt = o
        .health
        .map_or(0.0, |h| 1.0 - (h.shield + h.armour) / 2.0)
        .clamp(0.0, 1.0);
    m.combat.stance_next = Some(o.now + STANCE_EVERY);
    let mut out = vec![Act::Stance {
        hurt,
        range,
        reach: crate::escort::fight_range(&o.guns),
    }];
    out.extend(acts);
    out
}

/// `shipStanceMgr.SetShipStance(shipID, stanceID)` [V]; the refusal key on
/// failure (the switch lock raises `ShipAlreadyChangingStance`).
pub fn set_stance(
    conn: &mut Conn,
    ship: i64,
    stance: i64,
) -> Result<std::result::Result<(), String>> {
    let out = conn.call(
        "shipStanceMgr",
        "SetShipStance",
        vec![PyValue::Int(ship), PyValue::Int(stance)],
    )?;
    Ok(match out.refusal() {
        None => Ok(()),
        Some(key) => Err(key.to_owned()),
    })
}

#[cfg(test)]
mod tests;
