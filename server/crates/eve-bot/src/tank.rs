//! **A fighter fits a tank** (PROG-PVE2): the class's shield extenders and
//! hardeners for the shield races, armour plates, a membrane, a repairer and a
//! hardener for the armour races, within the hull's slots, CPU and power grid.
//!
//! The ratter's fit was guns only ([`crate::fit_plan`]); the tank skills were a
//! plan row and nothing more. A player puts something in the medium and low
//! slots before undocking, so this does, once per undock beside the rigs
//! ([`crate::rig::outfit`]) and the insurance:
//!
//! 1. **Fit what it holds.** The tank modules in the hangar, in the style's
//!    order ([`plan_for`]), as many as the free slots and what is left of the
//!    hull's CPU and power grid allow ([`choose_fits`]), each by the "Fit to
//!    Active Ship" call; then online (the server onlines a module the budget
//!    allows by itself, [`eve_server::modules::on_fitted`]).
//! 2. **Buy one it lacks**, in the closed economy, once per [`BUY_EVERY`] and
//!    only with a slot and a budget to spare ([`buy_candidates`]): the first
//!    missing piece's types that fit, at most [`PROBE_MAX`] books read, the
//!    first with an ask at this station inside [`WALLET_SHARE`] of the
//!    wallet, with the skill it asks for learned first. **Nothing from
//!    nowhere**: no NPC posts a module (`econ_exchange::ASK_CATEGORIES`), so
//!    an ask here is a module some builder made or somebody dropped, and
//!    `hulls::arms_rungs` gives builders the modules to make.
//! 3. **Run the active ones in a fight** ([`engage`], [`disengage`]): the
//!    hardeners, and the repairer unless the bot fires lasers (a repairer and
//!    a pulse laser draw the same small capacitor **[A]**), from the first
//!    shot to the cease-fire, beside the overload of hardeners
//!    ([`crate::overload`]).
//!
//! # The style
//!
//! Amarr and Gallente are armour tanked, Caldari and Minmatar shield tanked
//! ([`crate::progression::Race::armour`], the skills the plan trains). The
//! pieces are the T1 modules by size **[V-data]** with the next size down as
//! the fallback when a hull's power grid will not take the first.
//!
//! The budget is the server's own ([`eve_server::modules`]'s `budget`: the sum
//! of the online modules' CPU and power against the hull's output). The bot
//! reads the types' base numbers, which the real fit only lowers (skills cut
//! the need), so it never asks for more than the server would give.
//! Everything here is **[A]** policy except the call shapes and the type
//! data.

use crate::fit_plan::Size;
use crate::hull_choice::{HULLS, Weapon, class_of};
use crate::{Conn, Error, ItemRow, Result};
use eve_server::content::Content;
use eve_server::typedata::{self, attr};
use std::time::{Duration, Instant};

/// `cpu` and `power` of a module (attributes 50 and 30) **[V-data]**.
pub const ATTR_CPU: i64 = 50;
pub const ATTR_POWER: i64 = 30;
/// A module's required skill (attribute 182) **[V-data]**.
pub const ATTR_SKILL: i64 = 182;
/// The low and medium slot flags **[V]** (`const.flagLoPower0..7`, `flagMedPower0..7`).
pub const LOW_FLAGS: std::ops::RangeInclusive<i64> = 11..=18;
pub const MED_FLAGS: std::ops::RangeInclusive<i64> = 19..=26;

/// How often a bot reads the market for a tank module **[A]**.
pub const BUY_EVERY: Duration = crate::acquire::ACQUIRE_EVERY;
/// The most of the wallet one tank module may cost **[A]**.
pub const WALLET_SHARE: f64 = 0.2;
/// The most books read in one buy round **[A]**.
pub const PROBE_MAX: usize = 6;

/// Which rack a module goes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Med,
    Low,
}

/// How a module works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Online and done: an extender, a plate, a membrane.
    Passive,
    /// Run in a fight: a hardener.
    Hardener,
    /// Run in a fight: a repairer.
    Repairer,
}

/// One piece of a tank: the types that fill it, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Piece {
    pub types: &'static [i64],
    pub slot: Slot,
    pub kind: Kind,
}

const fn piece(types: &'static [i64], slot: Slot, kind: Kind) -> Piece {
    Piece { types, slot, kind }
}

/// Shield Extender I small, medium, large; Multispectrum Shield Hardener I
/// **[V-data]**.
const SHIELD_S: [Piece; 3] = [
    piece(&[377], Slot::Med, Kind::Passive),
    piece(&[578], Slot::Med, Kind::Hardener),
    piece(&[377], Slot::Med, Kind::Passive),
];
const SHIELD_M: [Piece; 3] = [
    piece(&[3_829, 377], Slot::Med, Kind::Passive),
    piece(&[578], Slot::Med, Kind::Hardener),
    piece(&[3_829, 377], Slot::Med, Kind::Passive),
];
const SHIELD_L: [Piece; 3] = [
    piece(&[3_839, 3_829], Slot::Med, Kind::Passive),
    piece(&[578], Slot::Med, Kind::Hardener),
    piece(&[3_839, 3_829], Slot::Med, Kind::Passive),
];
/// 100mm / 400mm / 1600mm Steel Plates I (the next size down behind each),
/// Multispectrum Energized Membrane I, Small / Medium / Large Armor Repairer
/// I, Thermal Armor Hardener I **[V-data]**.
const ARMOUR_S: [Piece; 4] = [
    piece(&[11_293, 11_291], Slot::Low, Kind::Passive),
    piece(&[11_267], Slot::Low, Kind::Passive),
    piece(&[523], Slot::Low, Kind::Repairer),
    piece(&[11_277], Slot::Low, Kind::Hardener),
];
const ARMOUR_M: [Piece; 4] = [
    piece(&[11_297, 11_295], Slot::Low, Kind::Passive),
    piece(&[11_267], Slot::Low, Kind::Passive),
    piece(&[3_528, 523], Slot::Low, Kind::Repairer),
    piece(&[11_277], Slot::Low, Kind::Hardener),
];
const ARMOUR_L: [Piece; 4] = [
    piece(&[11_279, 11_299], Slot::Low, Kind::Passive),
    piece(&[11_267], Slot::Low, Kind::Passive),
    piece(&[3_538, 3_528], Slot::Low, Kind::Repairer),
    piece(&[11_277], Slot::Low, Kind::Hardener),
];

/// **The tank of a style and size**, in the order it is built.
pub fn plan_for(armour: bool, size: Size) -> &'static [Piece] {
    match (armour, size) {
        (false, Size::Small) => &SHIELD_S,
        (false, Size::Medium) => &SHIELD_M,
        (false, Size::Large) => &SHIELD_L,
        (true, Size::Small) => &ARMOUR_S,
        (true, Size::Medium) => &ARMOUR_M,
        (true, Size::Large) => &ARMOUR_L,
    }
}

/// Every type of every tank, once: what a builder is offered
/// ([`crate::hulls::arms_rungs`]).
pub fn products() -> Vec<i64> {
    let mut out = Vec::new();
    for armour in [false, true] {
        for size in [Size::Small, Size::Medium, Size::Large] {
            for p in plan_for(armour, size) {
                out.extend_from_slice(p.types);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The kind of a tank module type, if it is one.
pub fn kind_of(type_id: i64) -> Option<Kind> {
    for armour in [false, true] {
        for size in [Size::Small, Size::Medium, Size::Large] {
            if let Some(p) = plan_for(armour, size)
                .iter()
                .find(|p| p.types.contains(&type_id))
            {
                return Some(p.kind);
            }
        }
    }
    None
}

/// What a hull gives: its free racks and its fitting output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hull {
    pub low: i64,
    pub med: i64,
    pub cpu: f64,
    pub power: f64,
}

/// A module's fitting need, `(cpu, power)`.
pub type Cost = (f64, f64);

/// What is left of a hull: free slots by rack, CPU and power, given the
/// fitted modules `(type, flag)`.
pub fn room(hull: &Hull, fitted: &[(i64, i64)], cost: &dyn Fn(i64) -> Option<Cost>) -> Room {
    let in_flags = |r: &std::ops::RangeInclusive<i64>| {
        fitted.iter().filter(|(_, f)| r.contains(f)).count() as i64
    };
    let (mut cpu, mut power) = (hull.cpu, hull.power);
    for (t, flag) in fitted {
        if eve_server::item::is_ship_fitting_flag(*flag)
            && let Some((c, p)) = cost(*t)
        {
            cpu -= c;
            power -= p;
        }
    }
    Room {
        low: hull.low - in_flags(&LOW_FLAGS),
        med: hull.med - in_flags(&MED_FLAGS),
        cpu,
        power,
    }
}

/// What [`room`] returns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Room {
    pub low: i64,
    pub med: i64,
    pub cpu: f64,
    pub power: f64,
}

impl Room {
    fn slots(&self, slot: Slot) -> i64 {
        match slot {
            Slot::Low => self.low,
            Slot::Med => self.med,
        }
    }

    fn fits(&self, slot: Slot, (cpu, power): Cost) -> bool {
        self.slots(slot) > 0 && cpu <= self.cpu + 1e-9 && power <= self.power + 1e-9
    }

    fn take(&mut self, slot: Slot, (cpu, power): Cost) {
        match slot {
            Slot::Low => self.low -= 1,
            Slot::Med => self.med -= 1,
        }
        self.cpu -= cpu;
        self.power -= power;
    }
}

/// **The pieces of the plan the ship does not have yet** (pure): each fitted
/// module of a piece's types fills one piece, in plan order.
pub fn missing(plan: &'static [Piece], fitted: &[i64]) -> Vec<&'static Piece> {
    let mut left: Vec<i64> = fitted.to_vec();
    let mut out = Vec::new();
    for p in plan {
        match left.iter().position(|t| p.types.contains(t)) {
            Some(i) => {
                left.remove(i);
            }
            None => out.push(p),
        }
    }
    out
}

/// **Which hangar modules to fit** (pure), `(item, type)`, one entry per
/// unit: for each missing piece in plan order the first of its types the
/// hangar holds that the free slot and the CPU and power left take.
pub fn choose_fits(
    plan: &'static [Piece],
    hull: &Hull,
    fitted: &[(i64, i64)],
    hangar: &[ItemRow],
    cost: &dyn Fn(i64) -> Option<Cost>,
) -> Vec<(i64, i64)> {
    let mut room = room(hull, fitted, cost);
    let types: Vec<i64> = fitted
        .iter()
        .filter(|(_, f)| LOW_FLAGS.contains(f) || MED_FLAGS.contains(f))
        .map(|(t, _)| *t)
        .collect();
    let mut stock: Vec<(i64, i64, i64)> = hangar
        .iter()
        .filter(|r| r.category_id == crate::rig::CATEGORY_MODULE && !r.is_fitted())
        .map(|r| (r.item_id, r.type_id, r.quantity.max(1)))
        .collect();
    let mut out = Vec::new();
    for p in missing(plan, &types) {
        for &t in p.types {
            let Some(c) = cost(t) else { continue };
            if !room.fits(p.slot, c) {
                continue;
            }
            let Some(s) = stock.iter_mut().find(|(_, st, n)| *st == t && *n > 0) else {
                continue;
            };
            s.2 -= 1;
            out.push((s.0, t));
            room.take(p.slot, c);
            break;
        }
    }
    out
}

/// **The types worth a look in the market** (pure): the first missing piece
/// that the free slot and budget take, its fitting types in order, at most
/// [`PROBE_MAX`]. Empty when the rack is full or the budget spent.
pub fn buy_candidates(
    plan: &'static [Piece],
    hull: &Hull,
    fitted: &[(i64, i64)],
    cost: &dyn Fn(i64) -> Option<Cost>,
) -> Vec<i64> {
    let room = room(hull, fitted, cost);
    let types: Vec<i64> = fitted
        .iter()
        .filter(|(_, f)| LOW_FLAGS.contains(f) || MED_FLAGS.contains(f))
        .map(|(t, _)| *t)
        .collect();
    for p in missing(plan, &types) {
        let ok: Vec<i64> = p
            .types
            .iter()
            .copied()
            .filter(|t| cost(*t).is_some_and(|c| room.fits(p.slot, c)))
            .take(PROBE_MAX)
            .collect();
        if !ok.is_empty() {
            return ok;
        }
    }
    Vec::new()
}

/// The hull's racks and output off the type data.
pub fn hull_of(content: &Content, type_id: i64) -> Option<Hull> {
    let row = typedata::installed_in(content).table().get(type_id)?;
    Some(Hull {
        low: row.attribute(attr::LOW_SLOTS)? as i64,
        med: row.attribute(attr::MED_SLOTS)? as i64,
        cpu: row.attribute(attr::CPU_OUTPUT)?,
        power: row.attribute(attr::POWER_OUTPUT)?,
    })
}

/// A module type's base fitting need off the type data.
pub fn cost_in(content: &Content, type_id: i64) -> Option<Cost> {
    let row = typedata::installed_in(content).table().get(type_id)?;
    Some((
        row.attribute(ATTR_CPU).unwrap_or(0.0),
        row.attribute(ATTR_POWER).unwrap_or(0.0),
    ))
}

/// The skill a module asks for (attribute 182), if it names one.
pub fn skill_of(content: &Content, type_id: i64) -> Option<i64> {
    typedata::installed_in(content)
        .table()
        .get(type_id)?
        .attribute(ATTR_SKILL)
        .map(|s| s as i64)
}

/// Whether the plan of the bot's hull class is armour: its race's style.
pub fn armour_style(race: crate::progression::Race) -> bool {
    race.armour()
}

/// What a bot remembers about its tank: [`Memory::tank`](crate::brain::Memory).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct State {
    /// No market read for a tank module before this.
    pub next_buy: Option<Instant>,
}

/// **Before undocking**: fit the tank modules the hangar holds, then (closed
/// economy, once per [`BUY_EVERY`]) buy one the plan lacks and fit it. Only
/// for a table hull. The note is empty when there was nothing to do.
pub fn outfit(
    conn: &mut Conn,
    content: &Content,
    race: crate::progression::Race,
    closed: bool,
    st: &mut State,
    now: Option<Instant>,
) -> Result<String> {
    let (Some(ship), Some(station)) = (conn.session().ship_id(), conn.session().station_id())
    else {
        return Ok(String::new());
    };
    let hangar = conn.hangar_items()?;
    let Some(hull_type) = hangar.iter().find(|r| r.item_id == ship).map(|r| r.type_id) else {
        return Ok(String::new());
    };
    let (Some(class), Some(hull)) = (class_of(HULLS, hull_type), hull_of(content, hull_type))
    else {
        return Ok(String::new());
    };
    let plan = plan_for(armour_style(race), crate::fit_plan::size_of_class(class));
    let cost = |t: i64| cost_in(content, t);
    let mut notes: Vec<String> = Vec::new();

    let fitted_now = |conn: &mut Conn| -> Result<Vec<(i64, i64)>> {
        Ok(conn
            .fitted()?
            .iter()
            .map(|r| (r.type_id, r.flag_id))
            .collect())
    };
    let mut fitted = fitted_now(conn)?;
    let fits =
        |conn: &mut Conn, fitted: &mut Vec<(i64, i64)>, notes: &mut Vec<String>| -> Result<()> {
            let hangar = conn.hangar_items()?;
            for (item, type_id) in choose_fits(plan, &hull, fitted, &hangar, &cost) {
                match conn.fit_auto(&[item]) {
                    Ok(rows) => {
                        notes.push(format!("fitted {type_id}"));
                        // The server onlines what the budget allows; ask for the
                        // rest, softly.
                        for r in rows {
                            if let Err(e @ (Error::Io(_) | Error::Protocol(_))) =
                                conn.set_online(r.item_id)
                            {
                                return Err(e);
                            }
                        }
                        *fitted = fitted_now(conn)?;
                    }
                    Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                    Err(e) => {
                        notes.push(format!("tank {item}: {e}"));
                        break;
                    }
                }
            }
            Ok(())
        };
    fits(conn, &mut fitted, &mut notes)?;

    let due = now.is_none_or(|n| st.next_buy.is_none_or(|t| n >= t));
    if closed && due {
        let wanted = buy_candidates(plan, &hull, &fitted, &cost);
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
                let have = fitted.iter().filter(|(x, _)| *x == t).count() as i64;
                // SKILLGATE: every requirement of the module, not attribute
                // 182 at level 1 (a plate asks for Hull Upgrades 3).
                let skills = crate::t3::lacking_skills(content, &[t], &Default::default());
                let got = crate::acquire::buy_more(conn, &skills, &[(t, have + 1)], None)?;
                if got.bought.is_empty() {
                    continue;
                }
                notes.push(format!("bought {t}"));
                fits(conn, &mut fitted, &mut notes)?;
                break;
            }
        }
    }
    if notes.is_empty() {
        return Ok(String::new());
    }
    Ok(format!("tank: {}", notes.join("; ")))
}

// ---- the fight --------------------------------------------------------------------

/// The activation effect (category 1, not offensive) of a module type.
pub fn activation_effect(content: &Content, type_id: i64) -> Option<(String, i64)> {
    let table = typedata::installed_in(content).table();
    table.get(type_id)?.effects.iter().find_map(|e| {
        let row = table.effect(*e)?;
        (row.category == 1 && !row.is_offensive).then(|| (row.name.clone(), row.effect_id))
    })
}

/// Whether a kind runs in a fight for a bot firing `weapon` **[A]**: hardeners
/// always, a repairer unless the bot fires lasers.
pub fn runs_for(kind: Kind, weapon: Weapon) -> bool {
    match kind {
        Kind::Passive => false,
        Kind::Hardener => true,
        Kind::Repairer => weapon != Weapon::Laser,
    }
}

fn running(conn: &mut Conn, content: &Content, weapon: Weapon) -> Result<Vec<(i64, String, i64)>> {
    let Some(ship) = conn.session().ship_id() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for r in conn
        .ship_items(ship, None)?
        .into_iter()
        .filter(ItemRow::is_fitted)
    {
        let Some(kind) = kind_of(r.type_id) else {
            continue;
        };
        if !runs_for(kind, weapon) {
            continue;
        }
        if let Some((name, id)) = activation_effect(content, r.type_id) {
            out.push((r.item_id, name, id));
        }
    }
    Ok(out)
}

/// **At the first shot**: run every hardener (and repairer) that is online and
/// not running. The modules the server accepted; a refusal is not an error.
pub fn engage(conn: &mut Conn, content: &Content, weapon: Weapon) -> Result<Vec<i64>> {
    let mut on = Vec::new();
    for (module, name, id) in running(conn, content, weapon)? {
        if conn.active_effects(module)?.contains(&id) {
            continue;
        }
        match conn.activate(module, (&name, id), None, crate::REPEAT_UNTIL_STOPPED) {
            Ok(_) => on.push(module),
            Err(Error::Refused(_)) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(on)
}

/// **At the cease-fire**: stop what [`engage`] started.
pub fn disengage(conn: &mut Conn, content: &Content, weapon: Weapon) -> Result<Vec<i64>> {
    let mut off = Vec::new();
    for (module, name, id) in running(conn, content, weapon)? {
        if !conn.active_effects(module)?.contains(&id) {
            continue;
        }
        match conn.deactivate(module, &name) {
            Ok(_) => off.push(module),
            Err(Error::Refused(_)) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(off)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented items; the types are the plan's own ids (the plan is the
    // subject), the numbers are invented.
    const EXT_S: i64 = 377;
    const EXT_M: i64 = 3_829;
    const HARDENER: i64 = 578;
    const PLATE_L: i64 = 11_279;
    const PLATE_M_DOWN: i64 = 11_299;
    const MEMBRANE: i64 = 11_267;

    fn item(item_id: i64, type_id: i64, quantity: i64) -> ItemRow {
        ItemRow {
            item_id,
            type_id,
            owner_id: 1,
            location_id: 2,
            flag_id: 4,
            quantity,
            group_id: 0,
            category_id: crate::rig::CATEGORY_MODULE,
            singleton: false,
        }
    }

    fn cost(t: i64) -> Option<Cost> {
        match t {
            EXT_S => Some((10.0, 5.0)),
            EXT_M => Some((20.0, 50.0)),
            HARDENER => Some((30.0, 1.0)),
            PLATE_L => Some((10.0, 500.0)),
            PLATE_M_DOWN => Some((10.0, 200.0)),
            MEMBRANE => Some((10.0, 1.0)),
            _ => None,
        }
    }

    fn hull(low: i64, med: i64, cpu: f64, power: f64) -> Hull {
        Hull {
            low,
            med,
            cpu,
            power,
        }
    }

    #[test]
    fn the_plan_is_the_races_style_and_the_hulls_size() {
        let shield = plan_for(false, Size::Medium);
        assert!(shield.iter().all(|p| p.slot == Slot::Med));
        let armour = plan_for(true, Size::Large);
        assert!(armour.iter().all(|p| p.slot == Slot::Low));
        // Each style has a passive piece, a hardener, and armour a repairer.
        assert!(shield.iter().any(|p| p.kind == Kind::Hardener));
        assert!(armour.iter().any(|p| p.kind == Kind::Repairer));
        // A bigger size brings a bigger first choice and never a smaller.
        assert_ne!(
            plan_for(true, Size::Small)[0].types[0],
            plan_for(true, Size::Large)[0].types[0]
        );
    }

    #[test]
    fn every_piece_has_a_kind_and_a_rack_and_the_products_cover_them() {
        let products = products();
        for armour in [false, true] {
            for size in [Size::Small, Size::Medium, Size::Large] {
                for p in plan_for(armour, size) {
                    assert!(!p.types.is_empty());
                    for t in p.types {
                        assert!(products.contains(t));
                        assert!(kind_of(*t).is_some());
                    }
                }
            }
        }
        assert_eq!(kind_of(1), None);
        assert_eq!(kind_of(HARDENER), Some(Kind::Hardener));
    }

    #[test]
    fn a_fitted_module_fills_its_piece_once() {
        let plan = plan_for(false, Size::Small);
        assert_eq!(missing(plan, &[]).len(), plan.len());
        // One extender fitted: the second extender piece is still missing.
        let left = missing(plan, &[EXT_S]);
        assert_eq!(left.len(), plan.len() - 1);
        assert_eq!(left.iter().filter(|p| p.kind == Kind::Passive).count(), 1);
        assert!(missing(plan, &[EXT_S, EXT_S, HARDENER]).is_empty());
    }

    #[test]
    fn the_hangars_modules_are_fitted_in_plan_order_within_slots_and_budget() {
        let plan = plan_for(false, Size::Small);
        // Two medium racks free, plenty of budget: an extender, then the
        // hardener; the third piece has no rack.
        let h = hull(3, 2, 100.0, 100.0);
        let hangar = [
            item(10, HARDENER, 1),
            item(11, EXT_S, 2),
            item(12, EXT_M, 1),
        ];
        let got = choose_fits(plan, &h, &[], &hangar, &cost);
        assert_eq!(got, vec![(11, EXT_S), (10, HARDENER)]);
    }

    #[test]
    fn the_power_grid_stops_a_piece_and_the_smaller_alternative_takes_over() {
        let plan = plan_for(true, Size::Large);
        // The large plate asks 500; 300 is left: the next size down fits.
        let h = hull(4, 0, 100.0, 300.0);
        let hangar = [item(20, PLATE_L, 1), item(21, PLATE_M_DOWN, 1)];
        let got = choose_fits(plan, &h, &[], &hangar, &cost);
        assert_eq!(got, vec![(21, PLATE_M_DOWN)]);
        // With the room for it, the first choice wins.
        let big = hull(4, 0, 100.0, 900.0);
        let got = choose_fits(plan, &big, &[], &hangar, &cost);
        assert_eq!(got.first(), Some(&(20, PLATE_L)));
    }

    #[test]
    fn what_is_fitted_uses_its_slot_and_its_share_of_the_budget() {
        let plan = plan_for(false, Size::Small);
        // One extender already in a medium rack (flag 19).
        let fitted = [(EXT_S, 19)];
        let h = hull(3, 3, 100.0, 100.0);
        let r = room(&h, &fitted, &cost);
        assert_eq!((r.low, r.med), (3, 2));
        assert_eq!((r.cpu, r.power), (90.0, 95.0));
        let hangar = [item(11, EXT_S, 1), item(10, HARDENER, 1)];
        // The extender piece is filled: only the hardener and the other
        // extender are missing, in that order.
        let got = choose_fits(plan, &h, &fitted, &hangar, &cost);
        assert_eq!(got, vec![(10, HARDENER), (11, EXT_S)]);
        // No rack, nothing.
        let full = hull(3, 1, 100.0, 100.0);
        assert!(choose_fits(plan, &full, &fitted, &hangar, &cost).is_empty());
    }

    #[test]
    fn a_stack_of_two_fits_twice_and_no_more_than_it_holds() {
        let plan = plan_for(false, Size::Small);
        let h = hull(0, 3, 1_000.0, 1_000.0);
        let hangar = [item(11, EXT_S, 2)];
        let got = choose_fits(plan, &h, &[], &hangar, &cost);
        assert_eq!(got, vec![(11, EXT_S), (11, EXT_S)]);
    }

    #[test]
    fn the_buy_list_is_the_first_missing_piece_that_the_hull_can_take() {
        let plan = plan_for(true, Size::Large);
        let h = hull(4, 0, 100.0, 300.0);
        // The plate piece: the large one does not fit, the next size does.
        assert_eq!(buy_candidates(plan, &h, &[], &cost), vec![PLATE_M_DOWN]);
        // The plate fitted: the membrane is next.
        let fitted = [(PLATE_M_DOWN, 11)];
        assert_eq!(buy_candidates(plan, &h, &fitted, &cost), vec![MEMBRANE]);
        // No rack left: nothing to look for.
        let full = hull(1, 0, 100.0, 300.0);
        assert!(buy_candidates(plan, &full, &fitted, &cost).is_empty());
        // A piece whose types have no data is skipped, not guessed.
        let nodata = |_t: i64| None;
        assert!(buy_candidates(plan, &h, &[], &nodata).is_empty());
    }

    #[test]
    fn hardeners_run_in_a_fight_and_a_repairer_not_for_a_laser_bot() {
        assert!(!runs_for(Kind::Passive, Weapon::Projectile));
        assert!(runs_for(Kind::Hardener, Weapon::Laser));
        assert!(runs_for(Kind::Repairer, Weapon::Hybrid));
        assert!(!runs_for(Kind::Repairer, Weapon::Laser));
    }

    #[test]
    fn the_activation_effect_is_the_types_category_one_effect() {
        let rows = format!(
            "{}\
             type\t990900001\t77\t7\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t990900101,16\t\tFixture Hardener\n\
             type\t990900002\t329\t7\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t16\t\tFixture Plate\n\
             effect\t990900101\t1\t0\t0\t0\t0\t0\t0\tfixtureHardening\t\n\
             effect\t16\t4\t0\t0\t0\t0\t0\t0\tonline\t\n",
            typedata::FIXTURE,
        );
        let mut c = Content::fixture();
        let table = typedata::Table::parse(&rows).expect("rows parse");
        c.insert("typedata", typedata::Installed::build(table));
        assert_eq!(
            activation_effect(&c, 990_900_001),
            Some(("fixtureHardening".to_owned(), 990_900_101))
        );
        assert_eq!(activation_effect(&c, 990_900_002), None);
        assert_eq!(activation_effect(&c, 1), None);
        assert_eq!(skill_of(&c, 990_900_001), None);
    }
}
