//! **The generic rules of a heavy module: fuel per cycle, no early stop, no
//! docking** (NW-1, issue #156). Siege, triage and bastion modules and the
//! industrial cores (effect group `SiegeMode`/`TriageMode`) were free before
//! this: their modifiers applied from data, and nothing burned, nothing
//! refused a click and nothing stopped a dock.
//!
//! | Rule | Data | Key | Args |
//! |---|---|---|---|
//! | fuel per cycle | `consumptionType` 713, `consumptionQuantity` 714 on the module **[V-data]** | `ModuleRequiresFuel` | `fuel`, `fueled` (names), as the cynosural field's |
//! | no early stop | `disallowEarlyDeactivation` 906 **[V-data]** | `EffectStillActive` | `moduleTypeID`, `timeLeft` **[V]** (key and arg names: `shipmodulebutton.DeactivateEffect`) |
//! | no docking | `disallowDocking` 2354 on the module (and added to the ship by its effect) **[V-data]** | `CantDockWhileModuleActive` | `moduleType` |
//!
//! The first cycle is paid at the click and each later one as it starts
//! **[A]** (as the cynosural field does; falsifier: TQ takes the fuel at
//! the end of the cycle). A module with no 713 (the bastion) burns nothing.
//!
//! **`timeLeft` is in blue ticks (100 ns)** **[V]**: the client sleeps
//! `SleepSim(timeLeft / const.MSEC)` (`DelayButtonUnlockForDeactivate`), and
//! `SleepSim` takes milliseconds, `const.MSEC` being 10,000 ticks.
//!
//! **What this does not touch**: the cynosural generator, the Entosis Link
//! and the jump portal generator pay their fuel in their own modules
//! ([`covered_elsewhere`]); the cloak has its own early-deactivation reading
//! (`crate::cloak`) and ends at once. The early-stop rule (906) is generic:
//! it applies to every module carrying it, cyno and portal included.

use crate::cyno::{ATTR_FUEL_QUANTITY, ATTR_FUEL_TYPE, FUEL_FLAGS};
use crate::item::{Edit, Item};
use crate::modules::{Running, type_arg_for, user_error};
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;

/// `disallowEarlyDeactivation` **[V-data]**.
pub const ATTR_DISALLOW_EARLY_DEACTIVATION: i64 = 906;
/// `disallowDocking` **[V-data]**.
pub const ATTR_DISALLOW_DOCKING: i64 = 2_354;

/// The five effects that hold the ship still while they run: a siege
/// module's `moduleBonusSiegeModule`, a triage module's, a bastion module's,
/// and the two industrial cores' (`speedFactor` -100 and a mass multiplier
/// **[V-data]**), whose guid is `effects.SiegeMode`/`TriageMode`.
pub const EFFECT_SIEGE: i64 = 6_582;
pub const EFFECT_TRIAGE: i64 = 6_581;
pub const EFFECT_BASTION: i64 = 6_658;
pub const EFFECT_INDUSTRIAL_CORE: i64 = 4_575;
pub const EFFECT_COMPACT_CORE: i64 = 8_119;

/// Whether `effect_id` is one of the five [`EFFECT_SIEGE`] family.
pub fn is_siege_like(effect_id: i64) -> bool {
    matches!(
        effect_id,
        EFFECT_SIEGE
            | EFFECT_TRIAGE
            | EFFECT_BASTION
            | EFFECT_INDUSTRIAL_CORE
            | EFFECT_COMPACT_CORE
    )
}

/// Effects whose fuel is paid by another module of this crate, unchanged.
/// **A later doomsday lane that pays its own fuel adds its effects here, or
/// they pay twice.**
fn covered_elsewhere(effect_id: i64) -> bool {
    crate::cyno::is_cyno(effect_id)
        || crate::jump_portal::is_portal(effect_id)
        || effect_id == crate::entosis::EFFECT_ENTOSIS_LINK
}

/// A module type's own value of an attribute: the type data's, else the
/// module row's.
fn own(w: &Registry, type_id: i64, a: i64) -> Option<f64> {
    crate::typedata::attribute_in(w.content(), type_id, a)
        .or_else(|| crate::modules::find_in(w.content(), type_id).and_then(|t| t.attribute(a)))
}

/// `(fuel type, quantity per cycle)` a module burns, if it burns any.
pub fn fuel_of(
    w: &Registry,
    ship_id: i64,
    module_id: i64,
    effect_id: i64,
    type_id: i64,
) -> Option<(i64, i64)> {
    if covered_elsewhere(effect_id) {
        return None;
    }
    let ty = own(w, type_id, ATTR_FUEL_TYPE)
        .map(|v| v.round() as i64)
        .filter(|t| *t > 0)?;
    // The module's live 714 (skills and rigs move it), else the type's, as
    // `capacitor_need` reads 6.
    let qty = crate::dogma::module_attribute(w, ship_id, module_id, ATTR_FUEL_QUANTITY)
        .or_else(|| own(w, type_id, ATTR_FUEL_QUANTITY))
        .map(|v| v.round() as i64)
        .filter(|q| *q > 0)
        .unwrap_or(1);
    Some((ty, qty))
}

/// The one stack that pays a cycle: the pilot's own, in the fuel bay or the
/// cargo hold, holding at least `qty`; the bay first, the smallest
/// sufficient stack first.
fn stack(w: &Registry, character_id: i64, ship_id: i64, fuel: i64, qty: i64) -> Option<Item> {
    FUEL_FLAGS.iter().find_map(|flag| {
        w.store()
            .items()
            .iter()
            .filter(|i| {
                i.location_id == ship_id
                    && i.flag_id == *flag
                    && i.type_id == fuel
                    && i.owner_id == character_id
                    && i.quantity >= qty
            })
            .min_by_key(|i| (i.quantity, i.item_id))
            .cloned()
    })
}

fn type_name(w: &Registry, type_id: i64) -> String {
    crate::item::find_in(w.content(), type_id)
        .map_or_else(|| format!("type {type_id}"), |t| t.name.to_owned())
}

/// `(ModuleRequiresFuel, {fuel, fueled})`, as the stop's error.
fn fuel_error(w: &Registry, type_id: i64, fuel: i64) -> PyValue {
    PyValue::Tuple(vec![
        crate::pyutil::bstr("ModuleRequiresFuel"),
        PyValue::Dict(crate::pyutil::pypairs(vec![
            ("fuel", crate::pyutil::bstr(&type_name(w, fuel))),
            ("fueled", crate::pyutil::bstr(&type_name(w, type_id))),
        ])),
    ])
}

/// **The activation check** (`modules::activate`, beside the cyno's): the
/// first cycle's fuel must be in one stack. A module with no fuel passes.
pub fn check(
    w: &Registry,
    character_id: i64,
    ship_id: i64,
    module_id: i64,
    effect_id: i64,
    type_id: i64,
) -> Result<(), ServiceError> {
    let Some((fuel, qty)) = fuel_of(w, ship_id, module_id, effect_id, type_id) else {
        return Ok(());
    };
    if stack(w, character_id, ship_id, fuel, qty).is_some() {
        return Ok(());
    }
    Err(user_error(
        "ModuleRequiresFuel",
        vec![
            ("fuel", crate::pyutil::bstr(&type_name(w, fuel))),
            ("fueled", crate::pyutil::bstr(&type_name(w, type_id))),
        ],
    ))
}

/// **Pay one cycle** (the click, then each later cycle as it starts). `Err`
/// is the stop's `error` and the module must stop; a module that burns
/// nothing always passes.
pub fn pay(w: &mut Registry, r: &Running) -> Result<(), PyValue> {
    burn(
        w,
        r.character_id,
        r.ship_id,
        r.module_id,
        r.effect.effect_id,
        r.type_id,
    )
}

/// [`pay`] for a module that is not in the cycle table (`crate::superweapon`'s
/// Lance, fired by `superWeaponMgr`, not by `dogmaIM.Activate`).
pub fn burn(
    w: &mut Registry,
    character_id: i64,
    ship_id: i64,
    module_id: i64,
    effect_id: i64,
    type_id: i64,
) -> Result<(), PyValue> {
    let Some((fuel, qty)) = fuel_of(w, ship_id, module_id, effect_id, type_id) else {
        return Ok(());
    };
    let Some(held) = stack(w, character_id, ship_id, fuel, qty) else {
        return Err(fuel_error(w, type_id, fuel));
    };
    let edit = if held.quantity == qty {
        Edit::Destroy {
            item_id: held.item_id,
        }
    } else {
        Edit::Consume {
            item_id: held.item_id,
            quantity: qty,
        }
    };
    match w.apply_items(&[edit]) {
        Ok(_) => Ok(()),
        Err(e) => {
            crate::errln!("[module_fuel] fuel for {character_id} not burned: {e:?}");
            Err(fuel_error(w, type_id, fuel))
        }
    }
}

/// **The early-stop refusal** (`modules::deactivate`): a module with 906 set
/// stops at its cycle boundary and the click is answered with
/// `EffectStillActive{moduleTypeID, timeLeft}` so the client keeps the button
/// locked for that long. `None` for a module that may be stopped at will.
/// `boundary` is when it will stop.
pub fn still_active(w: &Registry, r: &Running, boundary: i64, now: i64) -> Option<ServiceError> {
    own(w, r.type_id, ATTR_DISALLOW_EARLY_DEACTIVATION).filter(|v| *v > 0.0)?;
    Some(user_error(
        "EffectStillActive",
        vec![
            (
                "moduleTypeID",
                type_arg_for("EffectStillActive", "moduleTypeID", r.type_id),
            ),
            ("timeLeft", PyValue::Int((boundary - now).max(0))),
        ],
    ))
}

/// **The dock guard** (beside `cyno::refuse_dock`, in `CmdDock` and
/// `structure_dock::dock`): a ship whose `disallowDocking` (2354) reads above
/// zero on the dogma sheet does not dock, `CantDockWhileModuleActive`. The
/// sheet is the ship's own, so a running module's effect (which adds 2354 to
/// the ship) is what raises it, and a burst pilot docks as he should. The
/// error names the lowest-numbered running module of the ship that carries
/// 2354.
pub fn refuse_dock(w: &Registry, actor: Option<i64>) -> Result<(), ServiceError> {
    // NW-9a: a titan that fired a doomsday does not dock while it is held.
    crate::superweapon::dock_refusal(w, actor)?;
    let Some(ship_id) = actor.and_then(|a| w.character(a)).map(|c| c.ship_id) else {
        return Ok(());
    };
    let Some(sheet) = crate::dogma::sheet(w, ship_id) else {
        return Ok(());
    };
    if sheet.ship(ATTR_DISALLOW_DOCKING) <= 0.0 {
        return Ok(());
    }
    let blocker = w
        .modules()
        .running_all()
        .into_iter()
        .filter(|r| r.ship_id == ship_id)
        .filter(|r| own(w, r.type_id, ATTR_DISALLOW_DOCKING).is_some_and(|v| v > 0.0))
        .map(|r| (r.module_id, r.type_id))
        .min();
    match blocker {
        Some((_, t)) => Err(user_error(
            "CantDockWhileModuleActive",
            vec![(
                "moduleType",
                type_arg_for("CantDockWhileModuleActive", "moduleType", t),
            )],
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests;
