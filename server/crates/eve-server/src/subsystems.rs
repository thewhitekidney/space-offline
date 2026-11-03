//! **Tech 3 subsystems and the slots they give** (lane `t3-d`, R-T3 packet
//! T3-D).
//!
//! A strategic cruiser (group 963, `IsModularShip`) is a hull with no slots
//! of its own. It has four subsystem slots, `flagSubSystemSlot0..3`
//! (125..128), one subsystem item (category 32) in each, and **its slots and
//! hardpoints are the sums of the fitted subsystems' modifier attributes**:
//! `hiSlotModifier` 1374, `medSlotModifier` 1375, `lowSlotModifier` 1376,
//! `turretHardpointModifier` 1368, `launcherHardPointModifier` 1369 [V]
//! (`shipfitting/fittingStuff.GetHardwareLayoutForShip`,
//! `baseDogmaStaticSvc.GetShipHardwareModifierAttribs`). Two sides need the
//! sum, and they agree by test:
//!
//! - the dogma engine, through the Python effects 3774 and 3773
//!   (`crate::dogma::Eval::subsystem_slots`), so the sheet's `hiSlots` is
//!   right and every consumer of the sheet follows;
//! - the fitting validator, which has a store and no registry, through
//!   [`hull_fit`].
//!
//! **What the client does and what it leaves to us.** It limits fitting to
//! the right slot counts itself, from its own copy of the data and the
//! fitted items it is told about, and raises `CantUnfitSubSystems`,
//! `NotEnoughSubSystemsNotify` and `CannotFitSubSystemNotShip` before any
//! call [V] (`fittingSvc.py:664-678`, `fittingMgmtCont.py:364`). The server
//! is the second line for a stale window or a second client.
//!
//! **A subsystem stays a stack in the hangar** (the T3-A open question,
//! `typedata::Installed::build`): only ships and blueprints are singletons
//! there. A fit takes one unit and assembles it, exactly as for a module
//! (`Edit::Assemble`); the dogma inputs read only singletons, so a stack in
//! the hangar gives nothing. Decided by the client: the assemble dialog
//! lists hangar items by `fitsToShipType` and `subSystemSlot` and never
//! reads the stack size [V] (`assembleModularShip.py`).
//!
//! # Marked
//! - **[A]** a fit into an occupied subsystem flag is refused
//!   (`SlotAlreadyOccupied`); the client's own swap question
//!   `SubSystemTypeAlreadyFitted` has no raise site we can read, so a swap is
//!   an unfit then a fit. Falsifier: the client offers the swap and sends an
//!   `Add` onto the occupied flag expecting it replaced.
//! - **[A]** unfitting a subsystem is refused while the hull's remaining
//!   slots could not hold what is fitted (`CustomNotify`): a hull with more
//!   modules than slots has no sheet answer. Falsifier: retail lets the
//!   subsystem out and leaves the modules offline.
//! - **[A]** assembling with other than four subsystems is a `CustomNotify`;
//!   the client's key `NotEnoughSubSystems` is in the key list but its
//!   arguments are not readable (`loc_args` is `[item] ship.name`).
//!   Falsifier: a client run that gets that far shows the message.
//! - **[V key, A raise site]** undock with fewer than four subsystems is
//!   `TooFewSubSystemsToUndock` (FSD 2853, no arguments), the only key of its
//!   name; nothing client-side raises it.

use crate::content::Content;
use crate::fitting::{Fit, HullFit, Refusal};
use crate::item::{Edit, Item, Slot, Store};
use crate::model::Character;

/// `groupStrategicCruiser` [V-data]: a modular hull.
pub const GROUP_STRATEGIC_CRUISER: i64 = 963;
/// `categorySubSystem` [V].
pub const CATEGORY_SUBSYSTEM: i64 = 32;
/// `flagSubSystemSlot0`: the visible subsystem flags are the four from here
/// (`numVisibleSubsystems`) [V].
pub const FIRST_FLAG: i64 = 125;
/// `numVisibleSubsystems` [V].
pub const VISIBLE: i64 = 4;

/// `subSystemSlot` 1366: the flag a subsystem type goes in [V-data].
pub const ATTR_SUBSYSTEM_SLOT: i64 = 1366;
/// `fitsToShipType` 1380: the one hull type a subsystem is for [V-data].
pub const ATTR_FITS_TO_SHIP_TYPE: i64 = 1380;

/// `(hull attribute, subsystem attribute)` of `slotModifier` (3774) [V]:
/// `hiSlots` 14 <- 1374, `medSlots` 13 <- 1375, `lowSlots` 12 <- 1376.
pub const SLOT_MODIFIERS: [(i64, i64); 3] = [(14, 1374), (13, 1375), (12, 1376)];
/// The same for `hardPointModifierEffect` (3773): `turretSlotsLeft` 102 <-
/// `turretHardpointModifier` 1368, `launcherSlotsLeft` 101 <-
/// `launcherHardPointModifier` 1369 [V].
pub const HARDPOINT_MODIFIERS: [(i64, i64); 2] = [(102, 1368), (101, 1369)];

/// Whether a flag is one of the four visible subsystem flags.
pub fn is_subsystem_flag(flag_id: i64) -> bool {
    (FIRST_FLAG..FIRST_FLAG + VISIBLE).contains(&flag_id)
}

/// Whether a ship group is a modular hull.
pub fn is_modular_group(group_id: i64) -> bool {
    group_id == GROUP_STRATEGIC_CRUISER
}

/// The assembled subsystems fitted in `ship_id`, in the visible flags.
pub fn fitted(store: &Store, ship_id: i64) -> impl Iterator<Item = &Item> {
    store.contents_of(ship_id).filter(|i| {
        i.singleton && i.category_id == CATEGORY_SUBSYSTEM && is_subsystem_flag(i.flag_id)
    })
}

/// How many of the four flags hold a subsystem.
pub fn fitted_count(store: &Store, ship_id: i64) -> usize {
    fitted(store, ship_id).count()
}

/// The flag a subsystem type goes in: its `subSystemSlot`, if that is a
/// visible flag.
pub fn slot_flag(content: &Content, type_id: i64) -> Option<i64> {
    let flag = crate::typedata::attribute_in(content, type_id, ATTR_SUBSYSTEM_SLOT)? as i64;
    is_subsystem_flag(flag).then_some(flag)
}

/// The hull type a subsystem type is for (`fitsToShipType`).
pub fn fits_ship_type(content: &Content, type_id: i64) -> Option<i64> {
    let t = crate::typedata::attribute_in(content, type_id, ATTR_FITS_TO_SHIP_TYPE)? as i64;
    (t != 0).then_some(t)
}

/// **The fitting numbers of a ship, subsystems counted.** For an ordinary
/// hull its own row ([`crate::fitting::hull_in`]); for a modular one that row
/// (no slots) plus every fitted subsystem's modifiers, except `except` (an
/// item about to leave). `None` when the table has no row for the hull.
pub fn hull_fit(
    content: &Content,
    store: &Store,
    ship_id: i64,
    except: Option<i64>,
) -> Option<HullFit> {
    let ship = store.get(ship_id)?;
    let mut h = *crate::fitting::hull_in(content, ship.type_id)?;
    if !is_modular_group(ship.group_id) {
        return Some(h);
    }
    let read = |type_id: i64, attribute: i64| {
        crate::typedata::attribute_in(content, type_id, attribute).unwrap_or(0.0) as i64
    };
    for s in fitted(store, ship_id).filter(|i| Some(i.item_id) != except) {
        h.hi += read(s.type_id, SLOT_MODIFIERS[0].1);
        h.med += read(s.type_id, SLOT_MODIFIERS[1].1);
        h.low += read(s.type_id, SLOT_MODIFIERS[2].1);
        h.turrets += read(s.type_id, HARDPOINT_MODIFIERS[0].1);
        h.launchers += read(s.type_id, HARDPOINT_MODIFIERS[1].1);
    }
    Some(h)
}

/// **Fit a subsystem** (the branch `fitting::plan_fit_in` takes for a
/// category-32 item): into the active, modular ship, in its own flag, one
/// unit assembled. `flag_id` is the call's flag or
/// [`crate::fitting::FLAG_AUTOFIT`]; any other flag than the subsystem's own
/// is `NoSuitableSlotsForThisSubSystem`.
pub fn plan_fit(
    content: &Content,
    store: &Store,
    c: Option<&Character>,
    ship_id: i64,
    item_id: i64,
    flag_id: i64,
) -> Result<Fit, Refusal> {
    let c = c.ok_or(Refusal::NoCharacter)?;
    let station = c.station_id.ok_or(Refusal::NotDocked)?;
    if ship_id != c.ship_id {
        return Err(Refusal::NotActiveShip);
    }
    let ship = store.get(ship_id).ok_or(Refusal::NotActiveShip)?;
    let item = store.get(item_id).ok_or(Refusal::NotAvailable)?;
    let in_hangar = item.location_id == station && item.flag_id == crate::model::FLAG_HANGAR;
    let in_ship = item.location_id == ship_id && is_subsystem_flag(item.flag_id);
    if item.owner_id != c.character_id || !(in_hangar || in_ship) {
        return Err(Refusal::NotAvailable);
    }
    check_for_ship(content, ship, item)?;
    let own = slot_flag(content, item.type_id).ok_or(Refusal::NoSubSystemSlot {
        type_id: item.type_id,
    })?;
    if flag_id != crate::fitting::FLAG_AUTOFIT && flag_id != own {
        return Err(Refusal::NoSubSystemSlot {
            type_id: item.type_id,
        });
    }
    if store
        .contents_of(ship_id)
        .any(|i| i.item_id != item_id && i.flag_id == own)
    {
        return Err(Refusal::Occupied);
    }
    let to = Slot::new(c.character_id, ship_id, own);
    let plan = if item.singleton {
        vec![Edit::Move { item_id, to }]
    } else {
        vec![Edit::Assemble { item_id, to }]
    };
    Ok(Fit { plan, flag_id: own })
}

/// The ship is a modular hull and the subsystem is for exactly its type
/// (`CannotFitSubSystemNotShip` with the client's three arguments, `fittingSvc.py:674`).
fn check_for_ship(content: &Content, ship: &Item, sub: &Item) -> Result<(), Refusal> {
    let valid = fits_ship_type(content, sub.type_id).unwrap_or(0);
    if !is_modular_group(ship.group_id) || valid != ship.type_id {
        return Err(Refusal::SubSystemNotShip {
            type_id: sub.type_id,
            valid_ship_type_id: valid,
            ship_type_id: ship.type_id,
        });
    }
    Ok(())
}

/// **Take a subsystem out**: refused while what remains of the hull's slots
/// and hardpoints could not hold the modules fitted (a plan check for
/// `fitting::plan_unfit`). `Ok` for anything that is not a fitted subsystem.
pub fn check_unfit(content: &Content, store: &Store, item: &Item) -> Result<(), Refusal> {
    if item.category_id != CATEGORY_SUBSYSTEM || !is_subsystem_flag(item.flag_id) {
        return Ok(());
    }
    let Some(left) = hull_fit(content, store, item.location_id, Some(item.item_id)) else {
        return Ok(());
    };
    let mut used = [0i64; 3];
    let mut hardpoints = [0i64; 2];
    for m in store.contents_of(item.location_id) {
        if !crate::item::is_ship_fitting_flag(m.flag_id) || crate::item::is_loaded_charge(m) {
            continue;
        }
        if let Some((rack, _)) = crate::fitting::Rack::of_flag(m.flag_id) {
            used[rack as usize] += 1;
        }
        match crate::modules::find_in(content, m.type_id).and_then(|t| t.hardpoint) {
            Some(crate::fitting::Hardpoint::Turret) => hardpoints[0] += 1,
            Some(crate::fitting::Hardpoint::Launcher) => hardpoints[1] += 1,
            None => {}
        }
    }
    use crate::fitting::Rack;
    let over = used[Rack::Low as usize] > left.low
        || used[Rack::Medium as usize] > left.med
        || used[Rack::High as usize] > left.hi
        || hardpoints[0] > left.turrets
        || hardpoints[1] > left.launchers;
    if over {
        return Err(Refusal::Unsupported(
            "Remove the modules that need this subsystem's slots first.",
        ));
    }
    Ok(())
}

/// What `ship.AssembleShip(shipID, subSystems=[...])` was asked for, checked
/// and planned. `ship` is the hull (packaged or assembled, in the pilot's
/// hangar at `station`), `subs` the item ids the dialog returned.
///
/// **Exactly four, one per visible flag**, each the pilot's, in this
/// hangar, a subsystem of this hull's type; nothing is planned otherwise.
/// Returns the four `(item, flag)` pairs in flag order. The caller turns
/// them into edits once it knows the ship's id (a packaged stack of hulls
/// assembles to a new one).
pub fn check_assemble(
    content: &Content,
    store: &Store,
    c: &Character,
    station: i64,
    ship: &Item,
    subs: &[i64],
) -> Result<Vec<(i64, i64)>, Refusal> {
    let mut by_flag: Vec<(i64, i64)> = Vec::with_capacity(VISIBLE as usize);
    for &id in subs {
        let item = store.get(id).ok_or(Refusal::NotAvailable)?;
        if item.owner_id != c.character_id
            || item.location_id != station
            || item.flag_id != crate::model::FLAG_HANGAR
            || item.category_id != CATEGORY_SUBSYSTEM
        {
            return Err(Refusal::NotAvailable);
        }
        check_for_ship(content, ship, item)?;
        let flag = slot_flag(content, item.type_id).ok_or(Refusal::NoSubSystemSlot {
            type_id: item.type_id,
        })?;
        if by_flag.iter().any(|(_, f)| *f == flag) {
            return Err(Refusal::Occupied);
        }
        by_flag.push((id, flag));
    }
    if by_flag.len() as i64 != VISIBLE {
        return Err(Refusal::Unsupported(
            "A strategic cruiser needs one subsystem in each of its four slots.",
        ));
    }
    by_flag.sort_by_key(|(_, f)| *f);
    Ok(by_flag)
}

/// The edits that put the checked subsystems into `ship_id`.
pub fn assemble_edits(owner: i64, ship_id: i64, subs: &[(i64, i64)]) -> Vec<Edit> {
    subs.iter()
        .map(|&(item_id, flag)| Edit::Assemble {
            item_id,
            to: Slot::new(owner, ship_id, flag),
        })
        .collect()
}

/// Whether the active ship may undock: any ship but a modular hull with
/// fewer than four subsystems fitted.
pub fn may_undock(store: &Store, ship_id: i64) -> bool {
    match store.get(ship_id) {
        Some(ship) if is_modular_group(ship.group_id) => {
            fitted_count(store, ship_id) as i64 >= VISIBLE
        }
        _ => true,
    }
}

#[cfg(test)]
pub(crate) mod tests;
