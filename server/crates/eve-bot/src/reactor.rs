//! **The reactor career** (R-MOON packet MN-G): a bot that buys a reaction
//! formula, buys the inputs, runs the reaction at its station (MN-A), and
//! lists the product for the builders. It is the market half of the moon
//! chain: miners refine moon ore into moon materials and sell them into the
//! reactor's standing bids; the reactor's composites and polymers are what
//! the T2 and T3 builders' `build_t2` still lacks.
//!
//! **One cycle, docked** ([`run`], once every [`MARKET_EVERY`] through
//! [`Act::Market`]; the driver's arm sends a reactor here and every other
//! role to the builder):
//!
//! 1. **delivers** every job whose end time has passed
//!    ([`crate::builder::deliver`], the builder's own step, which works for
//!    any activity);
//! 2. **lists** each reaction product in the hangar, one ask per (type,
//!    station), at the seller's ask ([`crate::builder::list`] with this
//!    career's product test); a product that one of its own formulas eats is
//!    **kept**, so a chain of two formulas feeds itself;
//! 3. **buys a formula** ([`acquire_formula`]) when it holds fewer than
//!    [`MAX_FORMULAS`] and a seller has one that pays back in
//!    [`PAYBACK_DAYS`]. **The only source is the NPC's ask** (O5-a: NPC
//!    corporations sell blueprint originals, formulas included, at
//!    `corporationTrades` stations; `docs/lanes/mn-fg.md` MN-F): nothing is
//!    minted for a reactor. With a map, the nearest station within reach
//!    that sells it is fetched ([`crate::travel`]);
//! 4. **reacts** ([`work`]): for its best idle formula at this station it
//!    buys the inputs from asks at its station at no more than the margin
//!    rule's price ([`reaction_bids`]), installs a job of the runs the
//!    hangar covers (`InstallJob`, activity 9), and otherwise rests **one
//!    standing bid per (input, station)** for what is short, repriced as
//!    the builder's are. A refusal (no skill) queues Reactions through
//!    [`crate::acquire`].
//!
//! **Prices** are [`crate::pricing`]'s, the builder's rules; the one new
//! term is the job's fee, the Estimated Item Value of the inputs times the
//! SCC surcharge and the security tax (`industry::cost_lines`, R-MOON §1.1,
//! [V], the same rates MN-A charges) -- the margin rule is run on a product
//! price lowered by the fee ([`fee_adjusted`]), so the bids already leave room
//! for it. **The honest result at reference prices is thin**: a reaction's
//! `P_ref` is the sum of its inputs (`econ/prices.tsv` source `reaction`), so
//! the margin is all in the bids' discount, and a formula that costs
//! 10-15 M ISK pays back only if the runs a day times the margin a run does
//! ([`Ranked::payback_at`]). A reactor therefore buys a formula only when it
//! does, and otherwise says why in its act line.
//!
//! Every rule here is **[A] policy**, what a player would do, except the
//! client's own call shapes and the server's fee rates, which are cited [V].

use crate::brain::{self, Act, Memory, Observation};
use crate::builder::{self, Cycle, deliver, held_by_type, list, noted, settle, station_of};
use crate::pricing::{self, Prices, Rates};
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::verbs::BlueprintRow;
use crate::{Conn, Error, Result};
use eve_server::industry::{self, Blueprint};
use eve_server::reactions::{self, SKILL_REACTIONS};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// The `--loop reactor` row: a civilian that stays docked.
pub static ROLE: RoleSpec = RoleSpec {
    name: "reactor",
    greeting: "o7 -- reacting for the builders",
    civilian: true,
    docked,
    in_space,
    banner,
    setup,
};

/// How often a reactor runs one [`run`] cycle: the builder's **[A]**.
pub const MARKET_EVERY: (Duration, Duration) = builder::MARKET_EVERY;

/// The most formulas a reactor holds **[A]**: one slot runs one job at a
/// time (the base `reactionSlotLimit` is 1, [`SLOTS`]), so a second and
/// third formula are for a chain (an intermediate and the composite it
/// feeds), not for width. Falsified by a reactor that holds three formulas
/// and never runs the second (too many) or whose chain never closes (too few).
pub const MAX_FORMULAS: usize = 3;

/// **How soon a formula must pay for itself**, game days **[A]**: a player
/// buys a 10-15 M ISK original when the margin of the runs it will do repays
/// it within a couple of months. Falsified by reactors that never buy at
/// the soak's prices (too short) or that buy one and starve (too long).
pub const PAYBACK_DAYS: f64 = 60.0;

/// How long one job a reactor installs runs, about **[A]**: a reactor lets a
/// slot run for a day (the formulas' own runs are three or six hours). Runs
/// = this over the time per run, at least one. Falsified by a slot idle for
/// hours after a job (too long) or installs crowding the market (too short).
pub const JOB_TARGET: Duration = Duration::from_secs(24 * 60 * 60);

/// Reaction slots a reactor uses: the character's own `reactionSlotLimit`
/// **[V-data]** (`reactions::CHARACTER_BASE_REACTION_SLOTS`); the slot skills
/// are not read **[A]**.
pub const SLOTS: usize = reactions::CHARACTER_BASE_REACTION_SLOTS as usize;

/// The share of the wallet one job's inputs may take, standing bids included
/// **[A]**: the builder's ([`builder::WALLET_SHARE`]).
pub const WALLET_SHARE: f64 = builder::WALLET_SHARE;

/// **The job's fee as a fraction of the inputs' Estimated Item Value**: the
/// SCC surcharge (4%) plus the security tax (5%), **[V]**
/// (`industry::cost_lines`; the same constants MN-A's install charges). The
/// bot does not know whether its station is in a safe-sec system, so it
/// reads the larger fee **[A]**: falsified by a reactor's journal that
/// charges under this on every job.
pub const FEE_RATE: f64 = industry::SCC_SURCHARGE_RATE + industry::INDUSTRY_SECURITY_TAX_RATE;

/// How many formula books a cycle reads, best payback first **[A]**: a
/// bound on the calls (each book is one `GetOrders`), not a rule.
pub const BOOKS_PER_CYCLE: usize = 4;

/// The skills a reaction needs: Reactions, to the level the formulas ask
/// (R-MOON §1.1: level 3 on every formula) **[V-data]**. The reactor
/// queues it the way [`crate::acquire`] does when an install is refused.
pub const REACTOR_SKILLS: &[(i64, i64)] = &[(SKILL_REACTIONS, 3)];

// ---- the row ----------------------------------------------------------------------

/// Docked: one [`Act::Market`] when its time has come, then wait.
fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if o.failures.values().any(|&n| n >= brain::GIVE_UP_AFTER) {
        return vec![Act::Reconnect];
    }
    m.homing = None;
    if let Some(next) = m.market_next
        && o.now < next
    {
        return vec![Act::Wait(next - o.now)];
    }
    let next = o.now + m.jitter(MARKET_EVERY);
    m.market_next = Some(next);
    vec![Act::Market, Act::Wait(next - o.now)]
}

/// In space: home, the miner's way.
fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    match &o.park {
        Some(park) => brain::go_home(m, o, park),
        None => vec![Act::Wait(brain::SHORT_WAIT)],
    }
}

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, reconnects {}",
        s.name, s.phase, s.reconnects
    )
}

/// A reactor reacts docked and needs no hull, skill or rounds from GM.
fn setup(_lacks: Lacks) -> Vec<String> {
    Vec::new()
}

// ---- pure rules ---------------------------------------------------------------------

/// The Reactions level a formula asks, 0 when it names none.
pub fn required_level(bp: &Blueprint) -> i64 {
    bp.skills
        .iter()
        .find(|&&(s, _)| s == SKILL_REACTIONS)
        .map_or(0, |&(_, l)| l)
}

/// The job's fee for `runs` runs of `bp`: the inputs' Estimated Item Value
/// (`industry::estimated_item_value`, at the server's `P_ref`) times
/// [`FEE_RATE`].
pub fn run_fee(table: &industry::Table, bp: &Blueprint, runs: i64) -> f64 {
    industry::estimated_item_value(table, bp) * runs as f64 * FEE_RATE
}

/// Runs a day one slot does of `bp` at the formula's required Reactions
/// level: the server's own time rule (`reactions::time_multiplier`).
pub fn runs_per_day(bp: &Blueprint) -> f64 {
    let level = required_level(bp);
    let mult = reactions::time_multiplier(|s| if s == SKILL_REACTIONS { level } else { 0 });
    let seconds = bp.time as f64 * mult;
    if seconds > 0.0 {
        86_400.0 / seconds * SLOTS as f64
    } else {
        0.0
    }
}

/// The runs a reactor plans for `bp`: [`JOB_TARGET`] over a run's time at
/// its level, at least one.
pub fn planned_runs(bp: &Blueprint) -> i64 {
    let per_day = runs_per_day(bp) / SLOTS as f64;
    ((JOB_TARGET.as_secs() as f64 / 86_400.0) * per_day)
        .floor()
        .max(1.0) as i64
}

/// `mkt` with the product's price lowered so that [`pricing::margin_bids`]
/// leaves room for the job's `fee` per run: the budget the margin rule gives
/// the inputs is the product's net of tax and broker over `1 + MARGIN`, so
/// the product comes down by `fee x (1 + MARGIN)` over the net share of the
/// run's units. `None` when nothing of the product is left.
pub fn fee_adjusted(
    bp: &Blueprint,
    mkt: &BTreeMap<i64, f64>,
    fee: f64,
) -> Option<BTreeMap<i64, f64>> {
    let product = mkt.get(&bp.product_type_id).copied()?;
    let rates = Rates::UNSKILLED;
    let keep = 1.0 - rates.sales_tax - rates.broker_fee;
    let units = bp.product_quantity.max(1) as f64;
    let lowered = product - fee * (1.0 + pricing::MARGIN) / (units * keep);
    if !(lowered.is_finite() && lowered > 0.0) {
        return None;
    }
    let mut out = mkt.clone();
    out.insert(bp.product_type_id, lowered);
    Some(out)
}

/// **The margin rule's bids for a reaction's inputs** (pure): the builder's
/// ([`pricing::margin_bids`]) at the fee-adjusted product price. `None`:
/// nothing to react at a profit.
pub fn reaction_bids(
    bp: &Blueprint,
    mkt: &BTreeMap<i64, f64>,
    refs: &BTreeMap<i64, f64>,
    fee: f64,
) -> Option<BTreeMap<i64, f64>> {
    pricing::margin_bids(bp, &fee_adjusted(bp, mkt, fee)?, refs)
}

/// What one run clears at `bids` and the product at `mkt`: the product sold
/// from a resting ask (tax and broker off), less the inputs bought with
/// resting bids (broker on top) and the job's `fee`. Negative is a loss.
pub fn profit_per_run(
    bp: &Blueprint,
    mkt: &BTreeMap<i64, f64>,
    bids: &BTreeMap<i64, f64>,
    fee: f64,
) -> Option<f64> {
    let rates = Rates::UNSKILLED;
    let product = mkt.get(&bp.product_type_id).copied()?;
    let out = rates.net_of_listing(product * bp.product_quantity as f64);
    let mut inputs = 0.0;
    for &(t, q) in &bp.materials {
        inputs += q as f64 * rates.cost_of_bid(*bids.get(&t)?);
    }
    Some(out - inputs - fee)
}

/// A formula worth a look: what a run earns and how many a day.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ranked {
    pub formula: i64,
    /// [`profit_per_run`] at the margin rule's bids.
    pub profit_run: f64,
    /// [`runs_per_day`].
    pub runs_day: f64,
}

impl Ranked {
    /// ISK a day this formula earns.
    pub fn daily(&self) -> f64 {
        self.profit_run * self.runs_day
    }

    /// Game days to earn back `price`; `None` when it earns nothing.
    pub fn payback_at(&self, price: f64) -> Option<f64> {
        let daily = self.daily();
        (daily > 0.0 && price.is_finite() && price >= 0.0).then(|| price / daily)
    }

    /// The most a formula may cost and still pay back in [`PAYBACK_DAYS`].
    pub fn max_price(&self) -> f64 {
        (self.daily() * PAYBACK_DAYS).max(0.0)
    }
}

/// **The formulas worth buying** (pure), at the reference prices `refs`
/// (cheap: no history read): every formula in `table` not in `held`, whose
/// inputs and product and own price all have a `P_ref`, that makes a profit
/// at the margin rule's bids and would pay back in [`PAYBACK_DAYS`] at its
/// own `P_ref`; shortest payback first, then by type id. `sold`: the types
/// an NPC asks anywhere (`econ_exchange::Table::asked_types`), so a formula
/// the data has no seller for (the biochemical ones, MN-F) is left out and
/// never costs a market read; `None` or empty: unknown, none left out.
pub fn rank_formulas(
    table: &industry::Table,
    refs: &BTreeMap<i64, f64>,
    held: &[i64],
    sold: Option<&BTreeSet<i64>>,
) -> Vec<Ranked> {
    let mut out: Vec<(f64, Ranked)> = Vec::new();
    for bp in &table.reactions {
        let id = bp.blueprint_type_id;
        if held.contains(&id) || sold.is_some_and(|s| !s.is_empty() && !s.contains(&id)) {
            continue;
        }
        let Some(&price) = refs.get(&id) else {
            continue;
        };
        let fee = run_fee(table, bp, 1);
        let Some(bids) = reaction_bids(bp, refs, refs, fee) else {
            continue;
        };
        let Some(profit_run) = profit_per_run(bp, refs, &bids, fee) else {
            continue;
        };
        let ranked = Ranked {
            formula: id,
            profit_run,
            runs_day: runs_per_day(bp),
        };
        if let Some(days) = ranked.payback_at(price).filter(|&d| d <= PAYBACK_DAYS) {
            out.push((days, ranked));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.formula.cmp(&b.1.formula)));
    out.into_iter().map(|(_, r)| r).collect()
}

/// May the reactor pay `ask` for a formula ranked `r` with `wallet`: it pays
/// back in time, and the wallet passes the builder's cushion rule
/// ([`builder::may_climb`]).
pub fn may_buy(r: &Ranked, ask: f64, wallet: f64, holds_any: bool) -> bool {
    r.payback_at(ask).is_some_and(|d| d <= PAYBACK_DAYS)
        && builder::may_climb(holds_any, wallet, ask)
}

/// The formula originals a reactor holds, wherever they are.
pub fn formulas_held(blueprints: &[BlueprintRow], table: &industry::Table) -> Vec<BlueprintRow> {
    blueprints
        .iter()
        .filter(|b| b.is_original() && table.is_formula(b.type_id))
        .copied()
        .collect()
}

/// The formulas to react from: held originals here, idle, in the order a
/// reactor works them -- those whose inputs the hangar covers for a run
/// first, then by type id.
pub fn candidates<'a>(
    blueprints: &'a [BlueprintRow],
    station: i64,
    table: &industry::Table,
    held: &BTreeMap<i64, i64>,
) -> Vec<&'a BlueprintRow> {
    let mut v: Vec<&BlueprintRow> = blueprints
        .iter()
        .filter(|b| {
            b.is_original()
                && b.job_id.is_none()
                && b.location_id == station
                && table.is_formula(b.type_id)
        })
        .collect();
    v.sort_by_key(|b| {
        let covered = table
            .find_reaction(b.type_id)
            .is_some_and(|bp| builder::runs_on_hand(bp, 1, 0, held) > 0);
        (!covered, b.type_id, b.item_id)
    });
    v
}

/// The types one of the held formulas eats: a stack of one is an input, not a
/// product to list.
pub fn chain_inputs(formulas: &[BlueprintRow], table: &industry::Table) -> BTreeSet<i64> {
    formulas
        .iter()
        .filter_map(|b| table.find_reaction(b.type_id))
        .flat_map(|bp| bp.materials.iter().map(|&(t, _)| t))
        .collect()
}

// ---- the cycle over the wire ----------------------------------------------------------

/// The server's reaction formulas, if the content has any.
pub fn table_of(content: Option<&eve_server::content::Content>) -> Option<&industry::Table> {
    content
        .and_then(|c| c.try_get::<industry::Table>())
        .filter(|t| !t.reactions.is_empty())
}

/// **One reactor cycle, docked** (the module header's four steps). Without
/// formulas in the content there is nothing to do. `trips`: with a map, a
/// formula no ask here sells is fetched from the nearest station that does.
pub fn run(
    conn: &mut Conn,
    content: Option<&eve_server::content::Content>,
    trips: Option<crate::travel::Trips<'_>>,
) -> Result<Cycle> {
    let station = station_of(conn, "reactor")?;
    let mut cycle = Cycle::default();
    let Some(table) = table_of(content) else {
        cycle
            .notes
            .push("no reaction formulas in the content".to_owned());
        return Ok(cycle);
    };
    let mut prices = Prices::read(conn)?;
    // T3-POLYMER-PRICE: gases and the polymers that eat them get a `P_ref`.
    crate::gas_price::fill(&mut prices, content);
    deliver(conn, &mut cycle)?;
    let Some(mut blueprints) = noted(conn.blueprints(), &mut cycle, "blueprints")? else {
        return Ok(cycle);
    };
    let products: BTreeSet<i64> = table.reactions.iter().map(|b| b.product_type_id).collect();
    let eaten = chain_inputs(&formulas_held(&blueprints, table), table);
    let is_product = |r: &crate::ItemRow| {
        !r.singleton && products.contains(&r.type_id) && !eaten.contains(&r.type_id)
    };
    list(
        conn,
        station,
        &mut prices,
        Some(table),
        &is_product,
        &mut cycle,
    )?;
    acquire_formula(
        conn,
        station,
        table,
        &prices,
        &blueprints,
        content,
        trips,
        &mut cycle,
    )?;
    if cycle.blueprint.is_some() {
        blueprints = conn.blueprints()?;
    }
    work(conn, station, table, &mut prices, &blueprints, &mut cycle)?;
    Ok(cycle)
}

/// Step 3: **buy a formula**, from the NPC's ask or any ask, when it pays
/// back ([`rank_formulas`], then [`may_buy`] at the real price).
#[allow(clippy::too_many_arguments)]
fn acquire_formula(
    conn: &mut Conn,
    station: i64,
    table: &industry::Table,
    prices: &Prices,
    blueprints: &[BlueprintRow],
    content: Option<&eve_server::content::Content>,
    trips: Option<crate::travel::Trips<'_>>,
    cycle: &mut Cycle,
) -> Result<()> {
    let held: Vec<i64> = formulas_held(blueprints, table)
        .iter()
        .map(|b| b.type_id)
        .collect();
    if held.len() >= MAX_FORMULAS {
        return Ok(());
    }
    let sold = content
        .and_then(|c| c.try_get::<eve_server::econ_exchange::Table>())
        .map(|t| t.asked_types());
    let ranked = rank_formulas(table, prices.refs(), &held, sold.as_ref());
    if ranked.is_empty() {
        if held.is_empty() {
            cycle.notes.push(format!(
                "no formula pays back in {PAYBACK_DAYS} days at these prices"
            ));
        }
        return Ok(());
    }
    let wallet = conn.cash_balance()?;
    let holds_any = !held.is_empty();
    for r in ranked.iter().take(BOOKS_PER_CYCLE) {
        let book = conn.market_orders(r.formula)?;
        let cap = Some(r.max_price());
        if let Some((ask, _)) = crate::acquire::plan(&book, station, cap, wallet, 1)
            .into_iter()
            .next()
        {
            if !may_buy(r, ask.price, wallet, holds_any) {
                continue;
            }
            if let Some(row) = noted(
                conn.buy(station, r.formula, ask.price, 1, 0, builder::BROKER_FEE),
                cycle,
                "formula",
            )? && row.vol_remaining < 1
            {
                cycle.bought.push((r.formula, 1, ask.price));
                cycle.blueprint = Some(r.formula);
                return Ok(());
            }
            continue;
        }
        let (Some(trips), Some(system)) = (trips, conn.session().solar_system_id2()) else {
            continue;
        };
        let nearest = crate::travel::sources(&book, trips, system, station, cap, 1)
            .into_iter()
            .find(|s| may_buy(r, s.price, wallet, holds_any));
        if let Some(source) = nearest {
            let haul = crate::travel::Haul::Cargo;
            if let Some(fetched) = noted(
                crate::travel::fetch(conn, trips, &source, r.formula, 1, cap, haul),
                cycle,
                "formula trip",
            )? {
                cycle.bought.extend(fetched.bought.iter().copied());
                if fetched.home > 0 {
                    cycle.blueprint = Some(r.formula);
                }
                cycle.notes.push(fetched.summary());
            }
            return Ok(());
        }
    }
    cycle
        .notes
        .push("no seller of a formula that pays back in reach".to_owned());
    Ok(())
}

/// Step 4: **react** from the best idle formula here: the inputs from asks
/// at no more than [`reaction_bids`], a job of the runs the hangar covers,
/// else one standing bid per (input, station) for what is short.
fn work(
    conn: &mut Conn,
    station: i64,
    table: &industry::Table,
    prices: &mut Prices,
    blueprints: &[BlueprintRow],
    cycle: &mut Cycle,
) -> Result<()> {
    let running = conn
        .industry_jobs()?
        .iter()
        .filter(|j| !j.completed() && j.activity_id == industry::REACTION)
        .count();
    if running >= SLOTS {
        return Ok(());
    }
    let open = conn.my_open_orders()?;
    let now = eve_server::pyutil::filetime_now();
    let hangar = held_by_type(&conn.hangar_items()?);
    for row in candidates(blueprints, station, table, &hangar) {
        let Some(bp) = table.find_reaction(row.type_id) else {
            continue;
        };
        let mut types: Vec<i64> = bp.materials.iter().map(|&(t, _)| t).collect();
        types.push(bp.product_type_id);
        let mkt = prices.mkt_of(conn, &types)?;
        let fee = run_fee(table, bp, 1);
        let Some(bids) = reaction_bids(bp, &mkt, prices.refs(), fee) else {
            cycle
                .notes
                .push(format!("formula {}: no margin at P_mkt", row.type_id));
            continue;
        };
        // Standing bids for this formula follow its price, job or no job.
        for own in open
            .iter()
            .filter(|o| o.order.bid && o.order.station_id == station)
        {
            if let Some(&target) = bids.get(&own.order.type_id) {
                settle(conn, own, target, now, cycle)?;
            }
        }
        let escrow: f64 = open
            .iter()
            .filter(|o| o.order.bid)
            .map(|o| o.order.price * o.order.vol_remaining as f64)
            .sum();
        let wallet = conn.cash_balance()?;
        let isk = ((wallet + escrow) * WALLET_SHARE - escrow).max(0.0);
        let mut held = held_by_type(&conn.hangar_items()?);
        let planned = planned_runs(bp);
        let affordable = builder::affordable_runs(bp, planned, 0, &held, &bids, isk)
            .max(builder::runs_on_hand(bp, planned, 0, &held));
        if affordable == 0 {
            cycle.notes.push(format!(
                "formula {}: one run's inputs exceed {isk:.2} ISK",
                row.type_id
            ));
            continue;
        }
        for (t, short) in builder::shortfall(bp, affordable, 0, &held) {
            let book = conn.market_orders(t)?;
            let isk = conn.cash_balance()?;
            for (ask, n) in crate::acquire::plan(&book, station, Some(bids[&t]), isk, short) {
                let Some(filled) = noted(
                    conn.buy(station, t, ask.price, n, 0, builder::BROKER_FEE),
                    cycle,
                    "inputs",
                )?
                else {
                    break;
                };
                let got = n - filled.vol_remaining.clamp(0, n);
                if got > 0 {
                    cycle.bought.push((t, got, got as f64 * ask.price));
                }
            }
        }
        held = held_by_type(&conn.hangar_items()?);
        let runs = builder::runs_on_hand(bp, affordable, 0, &held);
        if runs > 0 {
            install_reaction(conn, row, runs, cycle)?;
            return Ok(());
        }
        for (t, short) in builder::shortfall(bp, affordable, 0, &held) {
            let resting = open
                .iter()
                .any(|o| o.order.bid && o.order.type_id == t && o.order.station_id == station);
            if resting {
                continue;
            }
            let price = bids[&t];
            if noted(
                conn.buy(
                    station,
                    t,
                    price,
                    short,
                    builder::ORDER_DAYS,
                    builder::BROKER_FEE,
                ),
                cycle,
                "bid",
            )?
            .is_some()
            {
                cycle.bids.push((t, price, short));
            }
        }
        return Ok(());
    }
    Ok(())
}

/// Install `runs` of the formula `row` (activity 9); on a refusal, learn
/// Reactions the way [`crate::acquire`] does and note it.
pub(crate) fn install_reaction(
    conn: &mut Conn,
    row: &BlueprintRow,
    runs: i64,
    cycle: &mut Cycle,
) -> Result<()> {
    match builder::install_lab_job(conn, row, industry::REACTION, runs, None, None) {
        Ok(job_id) => {
            cycle.installed.push((job_id, row.type_id, runs));
            Ok(())
        }
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => {
            cycle.notes.push(format!("install {}: {e}", row.type_id));
            let kit = crate::acquire::Kit {
                skills: REACTOR_SKILLS,
                ..crate::acquire::Kit::default()
            };
            let none = crate::acquire::Lacking {
                ship: false,
                module: false,
                ammo: false,
                in_capsule: false,
            };
            if let Some(got) = noted(crate::acquire::run(conn, &kit, none), cycle, "skills")? {
                cycle.notes.extend(got.missing);
                if !got.learned.is_empty() {
                    cycle.notes.push(format!("queued {:?}", got.learned));
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids and numbers only.
    const FORMULA: i64 = 990_800_001;
    const SECOND: i64 = 990_800_002;
    const FUEL: i64 = 990_800_101;
    const GAS: i64 = 990_800_102;
    const GOO: i64 = 990_800_201;
    const ALLOY: i64 = 990_800_202;
    const STATION: i64 = 69_800_001;
    const FUEL_REF: f64 = 20.0;
    const GAS_REF: f64 = 3.0;
    const FORMULA_REF: f64 = 50_000.0;
    /// A run: 5 fuel and 100 gas make 25 goo, in an hour.
    const FUEL_PER_RUN: i64 = 5;
    const GAS_PER_RUN: i64 = 100;
    const PER_RUN: i64 = 25;
    const RUN_SECONDS: i64 = 3_600;

    fn prices_table() -> industry::Table {
        // The product at the sum of its inputs: 16 a unit of 25 is 400.
        table_with(16.0)
    }

    /// A table with goo priced at `goo_ref` a unit.
    fn table_with(goo_ref: f64) -> industry::Table {
        let prices = eve_server::econ_prices::Table::parse(&[
            format!("{FUEL}\t{FUEL_REF}\tbase\t{FUEL_REF}\t4\t99\t\t1"),
            format!("{GAS}\t{GAS_REF}\tbase\t{GAS_REF}\t4\t99\t\t1"),
            format!("{GOO}\t{goo_ref}\treaction\t0.0\t4\t428\t\t1"),
            format!("{ALLOY}\t200.0\treaction\t0.0\t4\t428\t\t1"),
            format!("{FORMULA}\t{FORMULA_REF}\tbase\t0.0\t9\t1888\t\t1"),
            format!("{SECOND}\t{FORMULA_REF}\tbase\t0.0\t9\t1888\t\t1"),
        ])
        .expect("prices");
        let (f, s) = (FORMULA, SECOND);
        let t = RUN_SECONDS;
        let l = SKILL_REACTIONS;
        industry::Table::parse(
            &[
                format!("{f}\t9\tactivity\t0\t0\t\t{t}\t1000"),
                format!("{f}\t9\tmaterial\t{FUEL}\t{FUEL_PER_RUN}\t\t{t}\t1000"),
                format!("{f}\t9\tmaterial\t{GAS}\t{GAS_PER_RUN}\t\t{t}\t1000"),
                format!("{f}\t9\tproduct\t{GOO}\t{PER_RUN}\t\t{t}\t1000"),
                format!("{f}\t9\tskill\t{l}\t3\t\t{t}\t1000"),
                format!("{s}\t9\tactivity\t0\t0\t\t{t}\t1000"),
                format!("{s}\t9\tmaterial\t{FUEL}\t{FUEL_PER_RUN}\t\t{t}\t1000"),
                format!("{s}\t9\tmaterial\t{GOO}\t10\t\t{t}\t1000"),
                format!("{s}\t9\tproduct\t{ALLOY}\t5\t\t{t}\t1000"),
                format!("{s}\t9\tskill\t{l}\t3\t\t{t}\t1000"),
            ],
            &prices,
        )
        .expect("table")
    }

    fn refs(table: &industry::Table) -> BTreeMap<i64, f64> {
        table.prices.clone()
    }

    fn row(item_id: i64, type_id: i64, runs: i64, job: Option<i64>) -> BlueprintRow {
        BlueprintRow {
            item_id,
            type_id,
            location_id: STATION,
            flag_id: 4,
            runs,
            material_efficiency: 0,
            job_id: job,
        }
    }

    #[test]
    fn the_fee_is_the_eiv_times_the_two_rates_and_the_runs() {
        let t = prices_table();
        let bp = t.find_reaction(FORMULA).expect("formula");
        let eiv = FUEL_PER_RUN as f64 * FUEL_REF + GAS_PER_RUN as f64 * GAS_REF;
        assert!((run_fee(&t, bp, 1) - eiv * FEE_RATE).abs() < 1e-9);
        assert!((run_fee(&t, bp, 3) - 3.0 * eiv * FEE_RATE).abs() < 1e-9);
        const { assert!(FEE_RATE > industry::SCC_SURCHARGE_RATE) };
    }

    #[test]
    fn runs_a_day_follow_the_servers_time_rule() {
        let t = prices_table();
        let bp = t.find_reaction(FORMULA).expect("formula");
        let level = required_level(bp);
        assert_eq!(level, 3, "the invented formula names its level");
        let mult = reactions::time_multiplier(|s| if s == SKILL_REACTIONS { level } else { 0 });
        let want = 86_400.0 / (RUN_SECONDS as f64 * mult) * SLOTS as f64;
        assert!((runs_per_day(bp) - want).abs() < 1e-9);
        // A job of JOB_TARGET holds that many runs, at least one.
        assert_eq!(planned_runs(bp), (want / SLOTS as f64).floor() as i64);
        let mut slow = bp.clone();
        slow.time = 10 * 86_400;
        assert_eq!(planned_runs(&slow), 1);
    }

    #[test]
    fn the_bids_leave_room_for_the_fee() {
        let t = prices_table();
        let bp = t.find_reaction(FORMULA).expect("formula");
        let mkt = refs(&t);
        let fee = run_fee(&t, bp, 1);
        let with_fee = reaction_bids(bp, &mkt, &mkt, fee).expect("a margin");
        let no_fee = reaction_bids(bp, &mkt, &mkt, 0.0).expect("a margin");
        for &(m, _) in &bp.materials {
            assert!(with_fee[&m] < no_fee[&m], "the fee lowers the bid for {m}");
        }
        // At those bids a run clears at least the margin over what it costs.
        let profit = profit_per_run(bp, &mkt, &with_fee, fee).expect("priced");
        assert!(profit > 0.0, "{profit}");
        let cost: f64 = bp
            .materials
            .iter()
            .map(|&(m, q)| q as f64 * Rates::UNSKILLED.cost_of_bid(with_fee[&m]))
            .sum::<f64>()
            + fee;
        assert!(
            profit >= cost * pricing::MARGIN - 1e-6,
            "{profit} on {cost}"
        );
    }

    #[test]
    fn a_fee_larger_than_the_product_leaves_no_bid() {
        let t = prices_table();
        let bp = t.find_reaction(FORMULA).expect("formula");
        let mkt = refs(&t);
        assert_eq!(reaction_bids(bp, &mkt, &mkt, 1.0e12), None);
        let mut unpriced = mkt.clone();
        unpriced.remove(&GAS);
        assert_eq!(reaction_bids(bp, &unpriced, &unpriced, 0.0), None);
    }

    #[test]
    fn a_dear_formula_is_ranked_out_and_a_cheap_one_in() {
        // Goo at its input value: the run clears only the margin rule's discount.
        let t = table_with(30.0);
        let ranked = rank_formulas(&t, &refs(&t), &[], None);
        let r = ranked.iter().find(|r| r.formula == FORMULA);
        let r = r.expect("a formula that pays back at its price");
        assert!(r.payback_at(FORMULA_REF).expect("earns") <= PAYBACK_DAYS);
        // Its price is the limit, not the table's.
        assert!(r.payback_at(r.max_price() * 1.01).expect("earns") > PAYBACK_DAYS);
        assert!(r.payback_at(r.max_price()).expect("earns") <= PAYBACK_DAYS + 1e-9);
        // The same formula at 10,000 times its price never pays back.
        let mut dear = refs(&t);
        dear.insert(FORMULA, FORMULA_REF * 10_000.0);
        assert!(
            rank_formulas(&t, &dear, &[], None)
                .iter()
                .all(|r| r.formula != FORMULA)
        );
    }

    #[test]
    fn a_held_formula_is_not_ranked_and_the_best_payback_comes_first() {
        let t = table_with(30.0);
        let all = rank_formulas(&t, &refs(&t), &[], None);
        let mut days: Vec<f64> = all
            .iter()
            .map(|r| r.payback_at(FORMULA_REF).expect("earns"))
            .collect();
        let sorted = {
            let mut d = days.clone();
            d.sort_by(f64::total_cmp);
            d
        };
        assert_eq!(days, sorted);
        days.clear();
        let rest = rank_formulas(&t, &refs(&t), &[FORMULA], None);
        assert!(rest.iter().all(|r| r.formula != FORMULA));
    }

    #[test]
    fn a_formula_no_npc_sells_is_left_out_of_the_ranking() {
        let t = table_with(30.0);
        let on_sale: BTreeSet<i64> = [FORMULA].into();
        let ranked = rank_formulas(&t, &refs(&t), &[], Some(&on_sale));
        assert!(ranked.iter().all(|r| r.formula == FORMULA) && !ranked.is_empty());
        let nothing: BTreeSet<i64> = [SECOND].into();
        let ranked = rank_formulas(&t, &refs(&t), &[], Some(&nothing));
        assert!(ranked.iter().all(|r| r.formula == SECOND));
        // An empty index is "unknown", not "nothing is sold".
        let unknown = BTreeSet::new();
        assert_eq!(
            rank_formulas(&t, &refs(&t), &[], Some(&unknown)),
            rank_formulas(&t, &refs(&t), &[], None)
        );
    }

    #[test]
    fn a_formula_with_an_unpriced_input_is_never_ranked() {
        let t = table_with(30.0);
        let mut r = refs(&t);
        r.remove(&GAS);
        assert!(
            rank_formulas(&t, &r, &[], None)
                .iter()
                .all(|k| k.formula != FORMULA)
        );
    }

    #[test]
    fn a_purchase_needs_the_payback_and_the_wallet_cushion() {
        let r = Ranked {
            formula: FORMULA,
            profit_run: 100.0,
            runs_day: 10.0,
        };
        let price = r.max_price();
        assert!(
            may_buy(&r, price, price, false),
            "the first formula, wallet = price"
        );
        assert!(
            !may_buy(&r, price, price - 1.0, false),
            "short of the price"
        );
        assert!(
            !may_buy(&r, price, price, true),
            "a second needs the cushion"
        );
        assert!(may_buy(&r, price, price * builder::CLIMB_CUSHION, true));
        assert!(
            !may_buy(&r, price * 1.5, price * 100.0, true),
            "does not pay back"
        );
        let loss = Ranked {
            profit_run: -1.0,
            ..r
        };
        assert!(!may_buy(&loss, 0.0, 1.0e9, false), "a loss never pays back");
    }

    #[test]
    fn only_idle_originals_here_are_worked_and_covered_ones_first() {
        let t = prices_table();
        let blueprints = [
            row(1, SECOND, -1, None),
            row(2, FORMULA, -1, None),
            row(3, FORMULA, -1, Some(7)),
            row(4, FORMULA, 5, None),
            BlueprintRow {
                location_id: STATION + 1,
                ..row(5, FORMULA, -1, None)
            },
        ];
        let mut held = BTreeMap::new();
        // Nothing on hand: the two idle originals here, by type id.
        let order: Vec<i64> = candidates(&blueprints, STATION, &t, &held)
            .iter()
            .map(|b| b.item_id)
            .collect();
        assert_eq!(order, [2, 1]);
        // The second formula's inputs on hand: it goes first.
        held.insert(FUEL, FUEL_PER_RUN);
        held.insert(GOO, 10);
        let order: Vec<i64> = candidates(&blueprints, STATION, &t, &held)
            .iter()
            .map(|b| b.item_id)
            .collect();
        assert_eq!(order, [1, 2]);
    }

    #[test]
    fn what_a_held_formula_eats_is_not_a_product_to_list() {
        let t = prices_table();
        let both = [row(1, FORMULA, -1, None), row(2, SECOND, -1, None)];
        let eaten = chain_inputs(&both, &t);
        assert!(eaten.contains(&GOO) && eaten.contains(&FUEL) && eaten.contains(&GAS));
        assert!(!eaten.contains(&ALLOY));
        // With only the first formula the goo is a product, as it always was.
        let first = chain_inputs(&both[..1], &t);
        assert!(!first.contains(&GOO));
        assert_eq!(formulas_held(&both, &t).len(), 2);
        assert!(formulas_held(&[row(9, 12, -1, None)], &t).is_empty());
    }

    #[test]
    fn docked_it_reacts_once_a_cycle_and_waits_between() {
        let mut m = Memory::new("Reactor1", brain::Policy::fast());
        let mut o = Observation::empty(std::time::Instant::now());
        o.place = brain::Place::Docked { station: STATION };
        let first = docked(&mut m, &o);
        assert_eq!(first[0], Act::Market);
        assert!(matches!(first[1], Act::Wait(_)));
        assert!(
            matches!(docked(&mut m, &o)[0], Act::Wait(_)),
            "inside the wait"
        );
        assert!(
            ROLE.civilian
                && setup(Lacks {
                    ship: true,
                    module: true,
                    ammo: true,
                    isk: true
                })
                .is_empty()
        );
    }
}
