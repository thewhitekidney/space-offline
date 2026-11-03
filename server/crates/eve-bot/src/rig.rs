//! **Bots fit rigs, and builders make them** (X-RIG, `docs/lanes/x-rig.md`).
//!
//! The server's half is `eve_server::rigs`: a rig goes into a rig slot only
//! if its size is the hull's `rigSize`, its `upgradeCost` fits what is left
//! of the hull's calibration (`upgradeCapacity`), and unfitting one destroys
//! it. A rig is never bought for fitting's sake: it is bought, built or
//! dropped by somebody first.
//!
//! ## Pilots (every career, one hook)
//!
//! [`outfit`] runs just before a bot undocks (`driver::Bot::execute`'s
//! `Act::Undock` arm, beside the insurance), in whatever hull it flies:
//!
//! 1. **Fit what it holds.** The rigs in the hangar that suit its career
//!    ([`groups_for`]), of its hull's size, best first, as many as the free
//!    slots and the calibration allow ([`choose_fits`]), each by the "Fit to
//!    Active Ship" call (`MultiAdd` with `flag=0`, `Conn::fit_auto`) -- the
//!    server picks the slot. A refusal is a note.
//! 2. **Buy one it lacks**, in the closed economy, once per [`BUY_EVERY`]
//!    and only with a slot and calibration to spare: the career's rig types
//!    that fit ([`buy_candidates`]), at most [`PROBE_MAX`] books read, the
//!    first with an ask at this station inside [`WALLET_SHARE`] of the
//!    wallet. **Nothing from nowhere**: no NPC posts a module
//!    (`econ_exchange::ASK_CATEGORIES`), so an ask here is a rig some
//!    builder made or somebody dropped. The buy is `acquire::buy_more`, so
//!    its price cap is the consumers' ([`crate::pricing`]).
//!
//! Skills are not checked: the server does not ask for the Rigging skills at
//! fit time (`eve_server::rigs`), and the career's bonus is the dogma
//! engine's.
//!
//! ## Builders
//!
//! - **A rung** ([`RIG_RUNG`]): three small T1 rig blueprints, climbed last
//!   (after the starter rung) like any other, so the salvage the ratters and
//!   hackers sell has a buyer. Every published rig blueprint takes group 754
//!   salvage (`content_salvage.py --check`).
//! - **Rig copies they own** ([`build_copies`]): a blueprint copy of a rig
//!   with runs left, here and idle -- one a hacker's can dropped and a
//!   contract brought (HK3, X-CONTRACT) -- is built from whenever a
//!   manufacturing slot is free, exactly as a Tech II copy is
//!   (`builder::build_t2`): buy the salvage short from asks here at the
//!   margin rule's price, only if they cover it all, then install. The
//!   builder's contract offers keep rig blueprints ([`GROUP_RIG_BLUEPRINT`]):
//!   a copy that builds is not surplus.
//!
//! Everything here is **[A] policy** except the call shapes; no number from
//! here appears in a test as an expected value.

use crate::brain::RoleRef;
use crate::builder::{Cycle, MANUFACTURE_SLOTS};
use crate::pricing::Prices;
use crate::verbs::BlueprintRow;
use crate::{Conn, Error, ItemRow, Result};
use eve_server::content::Content;
use eve_server::industry;
use eve_server::rigs::{self, HullRigs, RigType};
use std::time::{Duration, Instant};

/// `categoryModule` **[V]**.
pub const CATEGORY_MODULE: i64 = 7;
/// The rig groups **[V-data]** (`typedata`: every published rig, by group):
/// armor, shield, energy, hybrid and projectile weapons, drones, launchers,
/// electronic superiority, core (navigation, cargo), targeting, scanning,
/// salvage, anchoring... only the ones a career uses are named.
pub const GROUP_ARMOR: i64 = 773;
pub const GROUP_SHIELD: i64 = 774;
pub const GROUP_PROJECTILE: i64 = 777;
/// Navigation and cargo (`Cargohold Optimization`, `Hyperspatial Velocity`).
pub const GROUP_CORE: i64 = 782;
/// Scanning (`Emission Scope Sharpener`, the hacking rigs).
pub const GROUP_SCANNING: i64 = 1233;
/// The blueprint group every rig blueprint is in **[V-data]** (667 of 667).
pub const GROUP_RIG_BLUEPRINT: i64 = 787;

/// The combat careers' rig groups, best first **[A]**: armor tank, then
/// projectile damage, then shield. Falsified by combat pilots whose fit
/// leaves a slot empty while a suiting rig sells at their station.
pub const COMBAT_GROUPS: &[i64] = &[GROUP_ARMOR, GROUP_PROJECTILE, GROUP_SHIELD];
/// The haulers' and miners' **[A]**: the hold and the engines.
pub const CARGO_GROUPS: &[i64] = &[GROUP_CORE];
/// The explorers' **[A]**: the scanning rigs, then the engines.
pub const EXPLORER_GROUPS: &[i64] = &[GROUP_SCANNING, GROUP_CORE];

/// The builders' rig rung **[A]**: Small Projectile Collision Accelerator I,
/// Small Trimark Armor Pump I and Small Cargohold Optimization I
/// (blueprints **[V-data]**), one for each of the three rig groups the
/// careers fit most. After the starter rung (`builder::STARTER_RUNG`), so a
/// poor builder reaches its first job first. Falsified by a G3 soak whose
/// builders never hold one (no station in reach sells it) or hold it and
/// never build (no salvage on the book).
pub const RIG_RUNG: &[i64] = &[31_681, 30_988, 31_118];

/// How often a bot reads the market for a rig, in the closed economy
/// **[A]**: as `acquire::ACQUIRE_EVERY`, asks come and go.
pub const BUY_EVERY: Duration = crate::acquire::ACQUIRE_EVERY;
/// The most of the wallet one rig may cost **[A]**: a rig is a luxury
/// beside the hull and the rounds. Falsified by pilots who never afford a
/// rig that sells at their station (too low) or fly broke (too high).
pub const WALLET_SHARE: f64 = 0.25;
/// The most books read in one buy round **[A]**: bounds the market calls a
/// pilot makes at an undock. Falsified by pilots that miss a rig that sells
/// because the career has more suiting types than this.
pub const PROBE_MAX: usize = 12;

/// What a bot remembers about rigs: [`Memory::rig`](crate::brain::Memory).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct State {
    /// No market read for a rig before this.
    pub next_buy: Option<Instant>,
}

/// The rig groups a career fits, best first; empty for a career that does
/// not fly a rigged hull (builders, reactors, PI, capitals, freighters).
pub fn groups_for(role: RoleRef) -> &'static [i64] {
    let is = |r: RoleRef| std::ptr::eq(role, r);
    if is(&crate::roles::MINE)
        || is(&crate::courier::ROLE)
        || is(&crate::trader::ROLE)
        || is(&crate::ice::ROLE)
    {
        CARGO_GROUPS
    } else if is(&crate::builder::ROLE)
        || is(&crate::reactor::ROLE)
        || is(&crate::capital::ROLE)
        || is(&crate::capital::ALT_ROLE)
        || is(&crate::hauler::ROLE)
        || is(&crate::pi::ROLE)
    {
        &[]
    } else if is(&crate::hacker::ROLE) || is(&crate::huffer::ROLE) || is(&crate::probe::ROLE) {
        EXPLORER_GROUPS
    } else {
        COMBAT_GROUPS
    }
}

/// How much room a hull has left: free rig slots and calibration, given the
/// types already fitted.
pub fn room(
    hull: &HullRigs,
    fitted: &[i64],
    rig_of: &dyn Fn(i64) -> Option<RigType>,
) -> (i64, f64) {
    let used: f64 = fitted
        .iter()
        .filter_map(|&t| rig_of(t))
        .map(|r| r.cost)
        .sum();
    (hull.slots - fitted.len() as i64, hull.calibration - used)
}

/// **Which hangar rigs to fit** (pure), by item id, one entry per unit (a
/// stack's id repeats): of the hull's size and a career group, best first --
/// a type not yet fitted before a repeat, then the career's group order,
/// then the dearest calibration -- while a slot and the calibration are left.
pub fn choose_fits(
    hull: &HullRigs,
    fitted: &[i64],
    hangar: &[ItemRow],
    groups: &[i64],
    rig_of: &dyn Fn(i64) -> Option<RigType>,
    group_of: &dyn Fn(i64) -> Option<i64>,
) -> Vec<i64> {
    let (mut slots, mut left) = room(hull, fitted, rig_of);
    let mut chosen_types: Vec<i64> = fitted.to_vec();
    // One candidate per unit held.
    let mut units: Vec<(i64, RigType, usize)> = Vec::new();
    for row in hangar.iter().filter(|r| r.category_id == CATEGORY_MODULE) {
        let Some(rig) = rig_of(row.type_id).filter(|r| r.size == hull.size) else {
            continue;
        };
        let Some(rank) = group_of(row.type_id).and_then(|g| groups.iter().position(|x| *x == g))
        else {
            continue;
        };
        for _ in 0..row.quantity.max(1) {
            units.push((row.item_id, rig, rank));
        }
    }
    let mut out = Vec::new();
    while slots > 0 {
        let pick = units
            .iter()
            .enumerate()
            .filter(|(_, (_, rig, _))| rig.cost <= left)
            .min_by(|(_, a), (_, b)| {
                let seen = |t: i64| chosen_types.iter().filter(|c| **c == t).count();
                (
                    seen(a.1.type_id),
                    a.2,
                    std::cmp::Reverse(a.1.cost.to_bits()),
                )
                    .cmp(&(
                        seen(b.1.type_id),
                        b.2,
                        std::cmp::Reverse(b.1.cost.to_bits()),
                    ))
                    .then(a.1.type_id.cmp(&b.1.type_id))
            })
            .map(|(i, _)| i);
        let Some(i) = pick else { break };
        let (item, rig, _) = units.remove(i);
        out.push(item);
        chosen_types.push(rig.type_id);
        slots -= 1;
        left -= rig.cost;
    }
    out
}

/// **The types worth looking for in the market** (pure): the career's rig
/// types of the hull's size that fit what is left, in order -- the career's
/// group order, then the dearest calibration, then type id -- at most
/// [`PROBE_MAX`].
pub fn buy_candidates(
    hull: &HullRigs,
    fitted: &[i64],
    groups: &[i64],
    all: &[(RigType, i64)],
    rig_of: &dyn Fn(i64) -> Option<RigType>,
) -> Vec<i64> {
    let (slots, left) = room(hull, fitted, rig_of);
    if slots <= 0 {
        return Vec::new();
    }
    let mut v: Vec<(usize, &RigType)> = all
        .iter()
        .filter(|(r, _)| r.size == hull.size && r.cost <= left)
        .filter_map(|(r, g)| groups.iter().position(|x| x == g).map(|rank| (rank, r)))
        .collect();
    v.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(b.1.cost.total_cmp(&a.1.cost))
            .then(a.1.type_id.cmp(&b.1.type_id))
    });
    v.into_iter()
        .take(PROBE_MAX)
        .map(|(_, r)| r.type_id)
        .collect()
}

/// **Before undocking**: fit the rigs the hangar holds, then (closed
/// economy, once per [`BUY_EVERY`]) buy one the career lacks and fit it.
/// The note is empty when there was nothing to do.
pub fn outfit(
    conn: &mut Conn,
    content: &Content,
    role: RoleRef,
    closed: bool,
    st: &mut State,
    now: Option<Instant>,
) -> Result<String> {
    let groups = groups_for(role);
    let (Some(ship), Some(station)) = (conn.session().ship_id(), conn.session().station_id())
    else {
        return Ok(String::new());
    };
    if groups.is_empty() {
        return Ok(String::new());
    }
    let hangar = conn.hangar_items()?;
    let Some(hull_type) = hangar.iter().find(|r| r.item_id == ship).map(|r| r.type_id) else {
        return Ok(String::new());
    };
    let Some(hull) = rigs::hull_rigs_in(content, hull_type).filter(|h| h.slots > 0) else {
        return Ok(String::new());
    };
    let rig_of = |t: i64| rigs::rig_in(content, t);
    let group_of = |t: i64| rigs::group_in(content, t);
    let mut notes: Vec<String> = Vec::new();

    let fitted_types = |conn: &mut Conn| -> Result<Vec<i64>> {
        Ok(conn
            .fitted()?
            .iter()
            .filter(|r| rigs::is_rig_flag(r.flag_id))
            .map(|r| r.type_id)
            .collect())
    };
    let mut fitted = fitted_types(conn)?;
    let fits = |conn: &mut Conn, fitted: &mut Vec<i64>, notes: &mut Vec<String>| -> Result<()> {
        let hangar = conn.hangar_items()?;
        for item in choose_fits(&hull, fitted, &hangar, groups, &rig_of, &group_of) {
            match conn.fit_auto(&[item]) {
                Ok(rows) => {
                    notes.push(format!("fitted rig {}", rows[0].type_id));
                    *fitted = fitted_types(conn)?;
                }
                Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                Err(e) => {
                    notes.push(format!("rig {item}: {e}"));
                    break;
                }
            }
        }
        Ok(())
    };
    fits(conn, &mut fitted, &mut notes)?;

    let due = now.is_none_or(|n| st.next_buy.is_none_or(|t| n >= t));
    if closed && due {
        let all = rigs::published_in(content);
        let wanted = buy_candidates(&hull, &fitted, groups, &all, &rig_of);
        if !wanted.is_empty() {
            st.next_buy = now.map(|n| n + BUY_EVERY);
            let isk = conn.cash_balance()?.max(0.0) * WALLET_SHARE;
            for t in wanted {
                let book = match conn.market_orders(t) {
                    Ok(b) => b,
                    Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                    Err(_) => continue,
                };
                if crate::acquire::plan(&book, station, None, isk, 1).is_empty() {
                    continue;
                }
                let got = crate::acquire::buy_more(conn, &[], &[(t, 1)], None)?;
                if got.bought.is_empty() {
                    continue;
                }
                notes.push(format!("bought rig {t}"));
                fits(conn, &mut fitted, &mut notes)?;
                break;
            }
        }
    }
    if notes.is_empty() {
        return Ok(String::new());
    }
    Ok(format!("rigs: {}", notes.join("; ")))
}

/// **The rig copies a builder can build from** (pure): copies here, with
/// runs left and no job, whose product `is_rig`, by type id.
pub fn copies<'a>(
    blueprints: &'a [BlueprintRow],
    station: i64,
    recipes: &industry::Table,
    is_rig: &dyn Fn(i64) -> bool,
) -> Vec<&'a BlueprintRow> {
    let mut v: Vec<&BlueprintRow> = blueprints
        .iter()
        .filter(|b| {
            !b.is_original()
                && b.runs > 0
                && b.job_id.is_none()
                && b.location_id == station
                && recipes
                    .find(b.type_id)
                    .is_some_and(|bp| is_rig(bp.product_type_id))
        })
        .collect();
    v.sort_by_key(|b| (b.type_id, b.item_id));
    v
}

/// **Build from a rig copy** (the builder's, after its ladder step): with a
/// manufacturing slot free, the first copy ([`copies`]) whose salvage the
/// hangar covers for a run -- after buying what is short from asks here at
/// the margin rule's price, all of it or none -- is installed. `true`: a job
/// installed. A copy with no salvage on the book is a note.
pub fn build_copies(
    conn: &mut Conn,
    station: i64,
    recipes: &industry::Table,
    content: &Content,
    prices: &mut Prices,
    trips: Option<crate::travel::Trips<'_>>,
    cycle: &mut Cycle,
) -> Result<bool> {
    let running = conn
        .industry_jobs()?
        .iter()
        .filter(|j| !j.completed() && j.activity_id == industry::MANUFACTURING)
        .count();
    if running >= MANUFACTURE_SLOTS {
        return Ok(false);
    }
    let blueprints = conn.blueprints()?;
    let is_rig = |t: i64| rigs::is_rig(content, t);
    for bp_row in copies(&blueprints, station, recipes, &is_rig) {
        let Some(bp) = recipes.find(bp_row.type_id) else {
            continue;
        };
        let planned = bp_row.runs.min(crate::builder::planned_runs(bp));
        let me = bp_row.material_efficiency;
        let mut held = crate::builder::held_by_type(&conn.hangar_items()?);
        if crate::builder::runs_on_hand(bp, planned, me, &held) == 0 {
            let mut types: Vec<i64> = bp.materials.iter().map(|&(t, _)| t).collect();
            types.push(bp.product_type_id);
            let mkt = prices.mkt_of(conn, &types)?;
            if let Some(bids) = crate::pricing::margin_bids(bp, &mkt, prices.refs()) {
                crate::builder::buy_whole_set(
                    conn,
                    station,
                    &crate::builder::shortfall(bp, 1, me, &held),
                    &bids,
                    cycle,
                )?;
                held = crate::builder::held_by_type(&conn.hangar_items()?);
            }
        }
        let runs = crate::builder::runs_on_hand(bp, planned, me, &held);
        if runs > 0 {
            crate::builder::install(conn, bp_row, runs, trips, cycle)?;
            return Ok(true);
        }
        cycle.notes.push(format!(
            "rig copy {}: salvage short (none on the book at the margin)",
            bp_row.type_id
        ));
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids.
    const SMALL: i64 = 1;
    const T_ARMOR: i64 = 990_820_001;
    const T_ARMOR2: i64 = 990_820_002;
    const T_PROJ: i64 = 990_820_003;
    const T_CORE: i64 = 990_820_004;
    const T_MEDIUM: i64 = 990_820_005;
    const T_BIG_COST: i64 = 990_820_006;

    fn rig(type_id: i64, cost: f64, size: i64) -> RigType {
        RigType {
            type_id,
            cost,
            size,
        }
    }

    fn table() -> Vec<(RigType, i64)> {
        vec![
            (rig(T_ARMOR, 50.0, SMALL), GROUP_ARMOR),
            (rig(T_ARMOR2, 100.0, SMALL), GROUP_ARMOR),
            (rig(T_PROJ, 60.0, SMALL), GROUP_PROJECTILE),
            (rig(T_CORE, 40.0, SMALL), GROUP_CORE),
            (rig(T_MEDIUM, 10.0, 2), GROUP_ARMOR),
            (rig(T_BIG_COST, 500.0, SMALL), GROUP_ARMOR),
        ]
    }

    fn rig_of(t: i64) -> Option<RigType> {
        table()
            .into_iter()
            .find(|(r, _)| r.type_id == t)
            .map(|(r, _)| r)
    }

    fn group_of(t: i64) -> Option<i64> {
        table()
            .into_iter()
            .find(|(r, _)| r.type_id == t)
            .map(|(_, g)| g)
    }

    fn hull(slots: i64, calibration: f64) -> HullRigs {
        HullRigs {
            slots,
            calibration,
            size: SMALL,
        }
    }

    fn held(item_id: i64, type_id: i64, quantity: i64) -> ItemRow {
        ItemRow {
            item_id,
            type_id,
            owner_id: 1,
            location_id: 2,
            flag_id: 4,
            quantity,
            group_id: 0,
            category_id: CATEGORY_MODULE,
            singleton: false,
        }
    }

    #[test]
    fn fits_the_careers_rigs_in_group_order_within_slots_and_calibration() {
        let h = hull(3, 200.0);
        let hangar = [
            held(10, T_ARMOR, 1),
            held(11, T_ARMOR2, 1),
            held(12, T_PROJ, 1),
            held(13, T_CORE, 1),
        ];
        // The armor group first (the dearest of it first); the projectile
        // rig no longer fits what is left; the core rig is no combat group.
        let got = choose_fits(&h, &[], &hangar, COMBAT_GROUPS, &rig_of, &group_of);
        assert_eq!(got, vec![11, 10]);
        // With room for all, the projectile rig comes too, never the core.
        let roomy = hull(3, 500.0);
        let got = choose_fits(&roomy, &[], &hangar, COMBAT_GROUPS, &rig_of, &group_of);
        assert_eq!(got, vec![11, 10, 12]);
    }

    #[test]
    fn the_calibration_stops_the_choice() {
        let h = hull(3, 120.0);
        let hangar = [
            held(11, T_ARMOR2, 1),
            held(12, T_PROJ, 1),
            held(10, T_ARMOR, 1),
        ];
        let got = choose_fits(&h, &[], &hangar, COMBAT_GROUPS, &rig_of, &group_of);
        let cost = |i: i64| match i {
            10 => 50.0,
            11 => 100.0,
            _ => 60.0,
        };
        assert!(got.iter().map(|&i| cost(i)).sum::<f64>() <= h.calibration);
        assert!(!got.is_empty());
    }

    #[test]
    fn slots_already_used_and_calibration_already_spent_are_counted() {
        let h = hull(2, 200.0);
        let hangar = [held(10, T_ARMOR, 1), held(12, T_PROJ, 1)];
        // One slot left.
        let got = choose_fits(&h, &[T_ARMOR2], &hangar, COMBAT_GROUPS, &rig_of, &group_of);
        assert_eq!(got.len(), 1);
        // No calibration left for either.
        let got = choose_fits(&h, &[T_ARMOR2], &hangar, COMBAT_GROUPS, &rig_of, &|t| {
            group_of(t)
        });
        assert!(got.len() <= 1);
        let tight = hull(2, 100.0);
        assert!(
            choose_fits(
                &tight,
                &[T_ARMOR2],
                &hangar,
                COMBAT_GROUPS,
                &rig_of,
                &group_of
            )
            .is_empty()
        );
    }

    #[test]
    fn the_wrong_size_and_the_wrong_group_are_left_alone() {
        let h = hull(3, 500.0);
        let hangar = [held(10, T_MEDIUM, 1), held(13, T_CORE, 1)];
        assert!(choose_fits(&h, &[], &hangar, COMBAT_GROUPS, &rig_of, &group_of).is_empty());
        assert_eq!(
            choose_fits(&h, &[], &hangar, CARGO_GROUPS, &rig_of, &group_of),
            vec![13]
        );
        // A hangar row that is not a module is never a rig.
        let mut not_module = held(14, T_ARMOR, 1);
        not_module.category_id = 6;
        assert!(choose_fits(&h, &[], &[not_module], COMBAT_GROUPS, &rig_of, &group_of).is_empty());
    }

    #[test]
    fn a_stack_fits_one_unit_per_slot_and_a_new_type_comes_before_a_repeat() {
        let h = hull(3, 500.0);
        let got = choose_fits(
            &h,
            &[],
            &[held(10, T_ARMOR, 5)],
            COMBAT_GROUPS,
            &rig_of,
            &group_of,
        );
        assert_eq!(got, vec![10, 10, 10], "three units off the stack");
        let hangar = [held(10, T_ARMOR, 5), held(12, T_PROJ, 1)];
        let got = choose_fits(&h, &[], &hangar, COMBAT_GROUPS, &rig_of, &group_of);
        assert!(
            got.contains(&12),
            "the second type is taken before a third armor"
        );
    }

    #[test]
    fn nothing_is_chosen_for_a_full_hull() {
        let h = hull(1, 500.0);
        assert!(
            choose_fits(
                &h,
                &[T_CORE],
                &[held(10, T_ARMOR, 1)],
                COMBAT_GROUPS,
                &rig_of,
                &group_of
            )
            .is_empty()
        );
    }

    #[test]
    fn buy_candidates_are_the_careers_types_that_still_fit() {
        let h = hull(3, 120.0);
        let got = buy_candidates(&h, &[], COMBAT_GROUPS, &table(), &rig_of);
        assert_eq!(got, vec![T_ARMOR2, T_ARMOR, T_PROJ]);
        // The calibration left drops the dear ones.
        let got = buy_candidates(&h, &[T_ARMOR], COMBAT_GROUPS, &table(), &rig_of);
        assert_eq!(got, vec![T_ARMOR, T_PROJ]);
        // No slot, no candidates.
        assert!(
            buy_candidates(
                &hull(1, 500.0),
                &[T_ARMOR],
                COMBAT_GROUPS,
                &table(),
                &rig_of
            )
            .is_empty()
        );
        // The cargo career wants only the core group.
        assert_eq!(
            buy_candidates(&h, &[], CARGO_GROUPS, &table(), &rig_of),
            vec![T_CORE]
        );
    }

    #[test]
    fn the_probe_is_bounded() {
        let many: Vec<(RigType, i64)> = (0..(PROBE_MAX as i64 * 3))
            .map(|i| (rig(990_830_000 + i, 1.0, SMALL), GROUP_ARMOR))
            .collect();
        let h = hull(3, 500.0);
        let r = |t: i64| many.iter().find(|(r, _)| r.type_id == t).map(|(r, _)| *r);
        assert_eq!(
            buy_candidates(&h, &[], COMBAT_GROUPS, &many, &r).len(),
            PROBE_MAX
        );
    }

    #[test]
    fn careers_get_their_groups() {
        assert_eq!(groups_for(&crate::roles::MINE), CARGO_GROUPS);
        assert_eq!(groups_for(&crate::courier::ROLE), CARGO_GROUPS);
        assert_eq!(groups_for(&crate::roles::ESCORT), COMBAT_GROUPS);
        assert_eq!(groups_for(&crate::hacker::ROLE), EXPLORER_GROUPS);
        assert!(groups_for(&crate::builder::ROLE).is_empty());
        assert!(groups_for(&crate::pi::ROLE).is_empty());
    }

    fn row(item_id: i64, type_id: i64, runs: i64, job: Option<i64>, at: i64) -> BlueprintRow {
        BlueprintRow {
            item_id,
            type_id,
            location_id: at,
            flag_id: 4,
            runs,
            material_efficiency: 0,
            job_id: job,
        }
    }

    #[test]
    fn copies_are_idle_copies_here_of_a_rig_with_runs_left() {
        let recipes = industry::Table::parse(
            &[
                "991\t1\tproduct\t990840001\t1\t\t600\t10",
                "991\t1\tmaterial\t990840100\t5\t\t600\t10",
                "992\t1\tproduct\t990840002\t1\t\t600\t10",
                "992\t1\tmaterial\t990840100\t5\t\t600\t10",
            ],
            &eve_server::econ_prices::Table::default(),
        )
        .expect("invented recipes parse");
        let is_rig = |t: i64| t == 990_840_001;
        let here = 7;
        let bps = [
            row(1, 991, 10, None, here),    // a rig copy: yes
            row(2, 991, -1, None, here),    // an original: no
            row(3, 991, 0, None, here),     // spent: no
            row(4, 991, 10, Some(5), here), // in a job: no
            row(5, 991, 10, None, 8),       // elsewhere: no
            row(6, 992, 10, None, here),    // not a rig: no
        ];
        let got: Vec<i64> = copies(&bps, here, &recipes, &is_rig)
            .iter()
            .map(|b| b.item_id)
            .collect();
        assert_eq!(got, vec![1]);
    }

    #[test]
    fn the_rig_rung_is_three_blueprints_and_the_group_is_the_clients() {
        assert_eq!(RIG_RUNG.len(), 3);
        assert_eq!(GROUP_RIG_BLUEPRINT, 787);
    }
}
