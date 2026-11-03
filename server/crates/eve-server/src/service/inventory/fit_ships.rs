//! `ship.FitShips`: the multi-fit window's "Fit N ships" (lane fitfix,
//! FITFIX-1, issue #175).
//!
//! **What the client sends [V].** `multiFitWnd.DoFitShips` ->
//! `fittingSvc.DoFitManyShips` (`fittingSvc.py:487`) calls, on the station's
//! bound `ship` object (`gameui.GetShipAccess()`):
//!
//! ```text
//! FitShips(shipTypeID=, fitInfo=, itemLocationID=, cargoItemsByType=,
//!          fitRigs=, name=, numToFit=, itemContainerID=)
//! ```
//!
//! `fitInfo` is the `KeyVal` a saved fitting's `GetKeyValForApplyingFit`
//! builds (`shipfitting/fitting.py:117`), the same one `FitFitting` takes:
//! `modulesByFlag {flag: typeID}`, and `{typeID: quantity **per ship**}` for
//! `chargesByType`, `dronesByType`, `fightersByTypeID`, `iceByType` and
//! `implantsByTypeID`. `cargoItemsByType` is the rigs the pilot chose not to
//! fit (`{typeID: n}` per ship). `itemLocationID` is the station. The window
//! counted its stock before it called (`multiFitWnd.py:263-321`), against
//! the hangar when `itemContainerID` is `None` and against that container
//! when it is not (`fittingSvc.GetQtyInLocationByTypeIDs`); hulls always come
//! from the hangar's packaged stacks.
//!
//! **What the client reads back [V].** `failedInfo = FitShips(...)`; a falsy
//! answer is success, else `failedToLoad, failedShipID = failedInfo`: a
//! `[(typeID, qty)]` list it prints as the "missing items" dialog
//! (`ShowFailedToLoadMsg`), and the id of a hull it renames to the bad-fit
//! label (`_RenameShip`, `SetLabel`). The window itself learns of the result
//! through the `OnItemChange` every move pushes (`PrepareForMultiFitCall`
//! registers for it to count progress).
//!
//! Every placement goes through the single-ship machinery: `Edit::Assemble`
//! for the hull, [`fit_one_as`] for a module or rig, [`to_cargo`] and the
//! drone and fighter bays' own loaders for the rest, so every rule the
//! drag-and-drop path enforces (slots, hardpoints, calibration, skills,
//! capacity) holds here, and each leg pushes the item rows it always does.
//!
//! Assumptions, each with its falsifier:
//!
//! - **[A]** every hull gets the one `name` the window sent. Falsifier: TQ
//!   numbering them.
//! - **[A]** a hull on which any module could not be placed is the
//!   `failedShipID` (the first such one); the rest are not named. Falsifier:
//!   TQ renaming only a hull it could not load at all. (No test pins which
//!   hull is named, only the shape.)
//! - **[A]** items in a container are first moved to the hangar, one unit at
//!   a time as they are needed, and stay there if their fit is refused.
//!   Falsifier: TQ leaving them in the container.
//! - **[A]** a strategic cruiser is refused whole: it needs its four
//!   subsystems chosen on assembly, which the window has no way to say.
//!   Falsifier: TQ assembling them from the fitting's subsystem flags.
//! - **[A]** modules are fitted lowest flag first and rigs only when
//!   `fitRigs` is set; subsystem flags are skipped (as `FitFitting` does).

use super::fit_apply::{int_map, pairs};
use super::*;
use std::collections::BTreeMap;

use super::fit_apply::{RIG_FLAGS, SUBSYSTEM_FLAGS};

/// The call, decoded.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Order {
    pub ship_type_id: i64,
    pub station_id: Option<i64>,
    pub name: Option<String>,
    pub num_to_fit: i64,
    pub container: Option<i64>,
    pub fit_rigs: bool,
    /// `{flag: typeID}`.
    pub modules: BTreeMap<i64, i64>,
    /// Cargo per ship: charges, ice, implants, and the rigs not fitted.
    pub cargo: BTreeMap<i64, i64>,
    pub drones: BTreeMap<i64, i64>,
    pub fighters: BTreeMap<i64, i64>,
}

fn kw<'a>(req: &'a Request<'_>, name: &str) -> Option<&'a PyValue> {
    crate::pyutil::pairs_get(req.kwargs, name.as_bytes())
}

fn int_of(v: Option<&PyValue>) -> Option<i64> {
    match v {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    }
}

/// Read the keyword arguments; `None` when the shape is not the client's.
pub(super) fn decode(req: &Request<'_>) -> Option<Order> {
    let ship_type_id = int_of(kw(req, "shipTypeID"))?;
    let num_to_fit = int_of(kw(req, "numToFit"))?;
    let fit_info = kw(req, "fitInfo")?;
    let at = |n: &str| crate::contracts::attr(fit_info, n);
    let mut modules = BTreeMap::new();
    // `{flag: typeID}`: the key is the flag.
    for (flag, type_id) in at("modulesByFlag").map(pairs).unwrap_or_default() {
        if let (PyValue::Int(f), PyValue::Int(t)) = (flag, type_id) {
            modules.insert(*f, *t);
        }
    }
    let mut cargo = int_map(at("chargesByType"));
    for (t, q) in int_map(at("iceByType"))
        .into_iter()
        .chain(int_map(at("implantsByTypeID")))
        .chain(int_map(kw(req, "cargoItemsByType")))
    {
        *cargo.entry(t).or_insert(0) += q;
    }
    let name = match kw(req, "name") {
        Some(PyValue::Unicode(s)) => Some(s.clone()),
        Some(PyValue::Bytes(b)) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    };
    Some(Order {
        ship_type_id,
        station_id: int_of(kw(req, "itemLocationID")),
        name,
        num_to_fit,
        container: int_of(kw(req, "itemContainerID")),
        fit_rigs: matches!(kw(req, "fitRigs"), Some(PyValue::Bool(true)))
            || matches!(kw(req, "fitRigs"), Some(PyValue::Int(n)) if *n != 0),
        modules,
        cargo,
        drones: int_map(at("dronesByType")),
        fighters: int_map(at("fightersByTypeID")),
    })
}

/// The pilot's items of `type_id` at the source (the hangar, or the chosen
/// container), lowest id first.
fn stock(reg: &Registry, order: &Order, owner: i64, station: i64, type_id: i64) -> Vec<i64> {
    let (location, flag) = match order.container {
        Some(c) => (c, None),
        None => (station, Some(crate::model::FLAG_HANGAR)),
    };
    let mut ids: Vec<i64> = reg
        .store()
        .contents_of(location)
        .filter(|i| {
            i.type_id == type_id
                && i.owner_id == owner
                && flag.is_none_or(|f| i.flag_id == f)
                && !crate::item::is_ship_fitting_flag(i.flag_id)
        })
        .map(|i| i.item_id)
        .collect();
    ids.sort_unstable();
    ids
}

/// `qty` units of the item `id`, made available in the station hangar: the
/// item itself when it is already there, else the whole item or a split of it
/// moved out of the container. Returns the hangar item's id.
fn to_hangar(
    reg: &mut Registry,
    owner: i64,
    station: i64,
    id: i64,
    qty: i64,
) -> Result<i64, ServiceError> {
    let Some(item) = reg.item(id).cloned() else {
        return Err(custom_notify("That item is not here."));
    };
    if item.location_id == station && item.flag_id == crate::model::FLAG_HANGAR {
        return Ok(id);
    }
    let to = crate::item::Slot::new(owner, station, crate::model::FLAG_HANGAR);
    let edit = if qty >= item.quantity || item.singleton {
        crate::item::Edit::Move { item_id: id, to }
    } else {
        crate::item::Edit::Split {
            item_id: id,
            quantity: qty,
            to,
        }
    };
    let changes = reg.apply_items(&[edit]).map_err(plan_refused)?;
    Ok(changes
        .iter()
        .find_map(|ch| match ch {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .unwrap_or(id))
}

/// One unit of `type_id` for a slot: an id [`fit_one_as`] can take from the
/// hangar, or `None` when the source has none.
fn one_unit(
    reg: &mut Registry,
    order: &Order,
    owner: i64,
    station: i64,
    type_id: i64,
) -> Option<i64> {
    let id = *stock(reg, order, owner, station, type_id).first()?;
    to_hangar(reg, owner, station, id, 1).ok()
}

/// The next packaged hull of the ordered type in the hangar, lowest id first.
fn next_hull(reg: &Registry, owner: i64, station: i64, type_id: i64) -> Option<crate::item::Item> {
    reg.store()
        .contents_of(station)
        .filter(|i| {
            i.type_id == type_id
                && i.owner_id == owner
                && i.flag_id == crate::model::FLAG_HANGAR
                && !i.singleton
                && i.category_id == crate::model::CATEGORY_SHIP
        })
        .min_by_key(|i| i.item_id)
        .cloned()
}

/// Undo [`to_hangar`] for a unit a hold refused: the whole item goes back to
/// `origin`; a split piece goes back and folds into the stack it came off.
fn put_back(reg: &mut Registry, id: i64, hangar_id: i64, origin: Option<crate::item::Slot>) {
    let Some(origin) = origin else { return };
    if reg
        .apply_items(&[crate::item::Edit::Move {
            item_id: hangar_id,
            to: origin,
        }])
        .is_err()
    {
        return;
    }
    if hangar_id != id {
        let _ = reg.apply_items(&[crate::item::Edit::Merge {
            from: hangar_id,
            into: id,
        }]);
    }
}

/// Bring a hold of the new hull up to `wanted` (`{typeID: quantity}`): what
/// could not be moved (no stock, no room) is added to `missing`.
fn load_hold(
    reg: &mut Registry,
    order: &Order,
    (owner, station, ship_id, flag): (i64, i64, i64, i64),
    wanted: &BTreeMap<i64, i64>,
    missing: &mut BTreeMap<i64, i64>,
) {
    for (&type_id, &qty) in wanted {
        let mut remaining = qty;
        for id in stock(reg, order, owner, station, type_id) {
            if remaining <= 0 {
                break;
            }
            let Some(have) = reg.item(id).map(|i| i.quantity) else {
                continue;
            };
            let take = remaining.min(have);
            let origin = reg.item(id).map(|i| i.slot());
            let Ok(hangar_id) = to_hangar(reg, owner, station, id, take) else {
                continue;
            };
            let moved = if flag == crate::drones::FLAG_DRONE_BAY {
                crate::drones::to_bay(reg, Some(owner), ship_id, hangar_id, Some(take))
            } else if flag == crate::fighters::FLAG_FIGHTER_BAY {
                crate::fighters::to_bay(reg, Some(owner), ship_id, hangar_id, Some(take))
            } else {
                to_cargo(reg, Some(owner), ship_id, hangar_id, Some(take), flag)
            };
            if moved.is_ok() {
                remaining -= take;
            } else if order.container.is_some() {
                // The hold refused it: the unit goes back where it came from
                // and this type is done, so the next stack is not moved out
                // over the hulls (FITFIX-FIX F1).
                put_back(reg, id, hangar_id, origin);
                break;
            }
        }
        if remaining > 0 {
            *missing.entry(type_id).or_insert(0) += remaining;
        }
    }
}

/// `ship.FitShips`, on the station's `ship` object.
pub(super) fn fit_ships(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let order = decode(req).ok_or_else(|| {
        module_bad_args(
            req,
            "(shipTypeID=, fitInfo=, itemLocationID=, cargoItemsByType=, fitRigs=, name=, \
             numToFit=, itemContainerID=)",
        )
    })?;
    let owner = req
        .actor
        .ok_or_else(|| custom_notify("No character is selected."))?;
    let c = reg
        .character(owner)
        .cloned()
        .ok_or_else(|| custom_notify("No character is selected."))?;
    crate::fitting::refuse_in_space(reg, &c).map_err(|r| r.to_error())?;
    let station = reg
        .docked_station(owner)
        .ok_or_else(|| crate::fitting::Refusal::NotDocked.to_error())?;
    if order.station_id.is_some_and(|s| s != station) {
        return Err(custom_notify("That is not the station you are docked in."));
    }
    if order.num_to_fit < 1 {
        return Ok(PyValue::None);
    }
    if let Some(cid) = order.container {
        let ok = reg.item(cid).is_some_and(|i| {
            i.owner_id == owner
                && crate::containers::is_container(i)
                && i.location_id == station
                && i.flag_id == crate::model::FLAG_HANGAR
        });
        if !ok {
            return Err(custom_notify("That container is not in your hangar."));
        }
    }
    let hull_type = crate::item::find_in(reg.content(), order.ship_type_id)
        .ok_or_else(|| custom_notify("This server does not know that ship."))?;
    if hull_type.category_id != crate::model::CATEGORY_SHIP {
        return Err(custom_notify("Only ships can be fitted this way."));
    }
    if crate::subsystems::is_modular_group(hull_type.group_id) {
        return Err(custom_notify(
            "A strategic cruiser is assembled on its own, with its subsystems.",
        ));
    }

    let mark = reg.pending_len();
    let mut missing: BTreeMap<i64, i64> = BTreeMap::new();
    let mut failed_ship = 0;
    for _ in 0..order.num_to_fit {
        let Some(hull) = next_hull(reg, owner, station, order.ship_type_id) else {
            break;
        };
        // The hull: the packaged unit becomes an assembled ship (the same id
        // when it was a stack of one, a new one off a larger stack).
        let changes = reg.apply_items(&[crate::item::Edit::Assemble {
            item_id: hull.item_id,
            to: hull.slot(),
        }]);
        // A refusal mid-batch ends the batch; what was done is still
        // announced once below (FITFIX-FIX F3).
        let Ok(changes) = changes else { break };
        let ship_id = changes
            .iter()
            .find_map(|ch| match ch {
                crate::item::Change::Assembled { item_id, .. }
                | crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .unwrap_or(hull.item_id);
        if let Some(n) = &order.name {
            let _ = crate::item_names::set_label(reg, Some(owner), ship_id, n);
        }
        // Fit as if this hull were the pilot's own (see `fit_one_as`).
        let as_pilot = crate::model::Character {
            ship_id,
            ship_type_id: order.ship_type_id,
            ..c.clone()
        };
        let mut hull_failed = false;
        for (&flag, &type_id) in &order.modules {
            if SUBSYSTEM_FLAGS.contains(&flag) || (RIG_FLAGS.contains(&flag) && !order.fit_rigs) {
                continue;
            }
            let placed = one_unit(reg, &order, owner, station, type_id)
                .and_then(|id| fit_one_as(reg, Some(as_pilot.clone()), ship_id, id, flag).ok());
            if placed.is_none() {
                *missing.entry(type_id).or_insert(0) += 1;
                hull_failed = true;
            }
        }
        // Rigs the pilot kept out of the slots are in `cargo` already.
        let at = |flag| (owner, station, ship_id, flag);
        load_hold(
            reg,
            &order,
            at(crate::fitting::FLAG_CARGO),
            &order.cargo,
            &mut missing,
        );
        load_hold(
            reg,
            &order,
            at(crate::drones::FLAG_DRONE_BAY),
            &order.drones,
            &mut missing,
        );
        load_hold(
            reg,
            &order,
            at(crate::fighters::FLAG_FIGHTER_BAY),
            &order.fighters,
            &mut missing,
        );
        if hull_failed && failed_ship == 0 {
            failed_ship = ship_id;
        }
    }
    // One cache drop for the whole batch, after every row: see
    // `Registry::coalesce_invalidations`.
    reg.coalesce_invalidations(mark);

    missing.retain(|_, q| *q > 0);
    if missing.is_empty() {
        return Ok(PyValue::None);
    }
    Ok(PyValue::Tuple(vec![
        PyValue::List(
            missing
                .into_iter()
                .map(|(t, q)| PyValue::Tuple(vec![PyValue::Int(t), PyValue::Int(q)]))
                .collect(),
        ),
        PyValue::Int(failed_ship),
    ]))
}
