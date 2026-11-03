//! **Control of a structure** (lane U6C, issue #115; spec
//! `handback/u6-upwell-upkeep`).
//!
//! The client offers a structure's fuel bay, deed bay and service slots only
//! while `session.shipid == session.structureid` (`IsControllingStructure`):
//! `inventorysvc.GetInvLocationTreeData` adds the structure's tree node in that
//! case and no other, and the fitting window builds a structure controller for
//! the active ship. A pilot docked in his own ship sees none of it **[V]**.
//!
//! # The model
//!
//! The pilot stays docked (`station_id` is the structure, his hull is in the
//! hangar and is still `Character::ship_id`). Control is **only** the session
//! value and a row in [`super::Docked::controls`]:
//!
//! - `TakeControl` queues `{shipid: (hull, structure)}`; the client re-primes
//!   (`godma.ProcessSessionChange`) and asks `GetAllInfo` again, which
//!   [`crate::service::Registry`]'s `all_info` answers with the structure as
//!   the ship (`controlling`).
//! - `ReleaseControl` queues `{shipid: (structure, hull)}`.
//! - Control is never persisted. A log-in is a pilot in his hull, because the
//!   login's session says `Character::ship_id`; a stale row is dropped the
//!   next time anyone asks ([`controlling`] checks the pilot is still docked
//!   there).
//! - One pilot per structure. The client asks `GetStructurePilot` first and
//!   confirms over another pilot (`controlButton`), then calls `TakeControl`
//!   anyway, so the server hands the controls over and the old pilot gets
//!   his `shipid` back.
//!
//! # What this lane does not do
//!
//! Fuel, fitting a service module, onlining and the fuel burn are #66's. This
//! lists the bays ([`bay_rows`]) so the client has something to open.
//!
//! # [A]s (falsifier in each)
//!
//! - `structureControl.TakeControl` is answered `None` and the session change
//!   alone moves the client (the call is a bare `RemoteSvc` call, result
//!   discarded). Falsifier: `fci_upwell_control` shows `shipid` unchanged.
//! - `GetAllInfo` with the structure as the ship carries the structure's row
//!   in both `structureInfo` and `shipInfo` and a `shipState` of an unfitted
//!   ship. Falsifier: a traceback out of `PrimeLocation` / `_MakeShipActive`
//!   in the control scenario.
//! - A refusal to take control is `CustomNotify` and the profile's own error
//!   for a missing setting (as at a dock). Falsifier: CCP's dialog text.
//! - A structure whose controller logged off has no pilot (the lazy check in
//!   [`pilot_of`]); CCP may keep the seat. Falsifier: the owner.

use super::*;

/// `flagStructureFuel` **[V]** (`inventorycommon/const`).
pub const FLAG_FUEL: i64 = 172;
/// `flagStructureDeed` **[V]**.
pub const FLAG_DEED: i64 = 180;
/// `flagCargo`: the structure's ammo bay (`StructureAmmoBay`) **[V]**.
const FLAG_AMMO: i64 = 5;
/// `flagFighterBay` **[V]**.
const FLAG_FIGHTER: i64 = 158;
/// `flagMoonMaterialBay` **[V]**.
const FLAG_MOON: i64 = 186;
/// `flagServiceSlot0..7` **[V]**.
const SERVICE_SLOTS: std::ops::RangeInclusive<i64> = 164..=171;

/// Whether `flag` is one of a structure's own bays or slots, as opposed to the
/// hangar the docked inventory is otherwise read as.
pub fn is_bay_flag(flag: i64) -> bool {
    matches!(
        flag,
        FLAG_FUEL | FLAG_DEED | FLAG_AMMO | FLAG_FIGHTER | FLAG_MOON
    ) || SERVICE_SLOTS.contains(&flag)
}

/// The structure `actor` has the controls of: the row, and the pilot still
/// docked there. `None` for everyone else.
pub fn controlling(reg: &Registry, actor: i64) -> Option<i64> {
    let (&id, _) = reg
        .ext::<Docked>()
        .controls
        .iter()
        .find(|(_, pilot)| **pilot == actor)?;
    (docked_in(reg, actor) == Some(id)).then_some(id)
}

/// The pilot at `structure`'s controls: a connected one, still docked there.
pub fn pilot_of(reg: &Registry, structure: i64) -> Option<i64> {
    let pilot = *reg.ext::<Docked>().controls.get(&structure)?;
    let live = !reg.sessions().of_character(pilot).is_empty();
    (live && controlling(reg, pilot) == Some(structure)).then_some(pilot)
}

fn shipid(old: i64, new: i64) -> Vec<(PyValue, PyValue)> {
    crate::pyutil::pypairs(vec![("shipid", pair(PyValue::Int(old), PyValue::Int(new)))])
}

/// `structureControl.TakeControl(structureID)`: every gate, then the controls.
///
/// The client calls it only docked in `structureID`, not already in control,
/// and offers the button when the profile lets him (`MayTakeControl`, setting
/// 17); so these are the server repeating them.
pub fn take(reg: &mut Registry, actor: i64, structure_id: i64) -> Result<(), ServiceError> {
    let Some(s) = structure::get(reg.deltas(), structure_id) else {
        return Err(not_dockable());
    };
    if dogma_host(reg, actor) != Some(structure_id) {
        return Err(not_dockable());
    }
    if controlling(reg, actor) == Some(structure_id) {
        return Ok(());
    }
    if !sp::has_access(reg, &s, actor, sp::SETTING_DEFENSE_CAN_CONTROL_STRUCTURE) {
        return Err(ServiceError::Raise(eve_proto::user_error(
            sp::access_error(sp::SETTING_DEFENSE_CAN_CONTROL_STRUCTURE),
            Vec::new(),
        )));
    }
    let Some(hull) = reg.character(actor).map(|c| c.ship_id) else {
        return Err(not_dockable());
    };
    // Never two rows for one pilot (a seat left behind elsewhere).
    drop_control(reg, actor, false);
    // The client confirmed over the pilot it showed; he is put back in his hull.
    if let Some(other) = reg.ext::<Docked>().controls.get(&structure_id).copied() {
        drop_control(reg, other, true);
        let who = reg
            .character(actor)
            .map_or_else(|| actor.to_string(), |c| c.name.clone());
        reg.push(crate::shutdown::ShutdownMessage {
            text: format!("{who} has taken the controls of this structure."),
            to: vec![other],
        });
    }
    reg.ext_mut::<Docked>().controls.insert(structure_id, actor);
    reg.queue_session_change(actor, shipid(hull, structure_id));
    Ok(())
}

/// `structureControl.ReleaseControl()` and `EjectFromStructure(shipID)` for a
/// docked pilot: the controls go and his hull is his ship again. A pilot who
/// does not hold them is left alone, as the client's own guard does.
pub fn release(reg: &mut Registry, actor: i64, ship_id: Option<i64>) -> Result<(), ServiceError> {
    let hull = reg.character(actor).map(|c| c.ship_id);
    // Only the hull he has: swapping hulls is `ship.Board`'s.
    if ship_id.is_some() && ship_id != hull {
        return Err(notify("Your active ship changed. Try again."));
    }
    drop_control(reg, actor, true);
    Ok(())
}

/// A pilot's controls are over. `session` queues the `shipid` change back to
/// his hull; leave it off when the caller's own change already names the ship
/// (a revival), or the session is closing.
pub(crate) fn drop_control(reg: &mut Registry, actor: i64, session: bool) {
    if let Some(change) = drop_control_change(reg, actor)
        && session
    {
        reg.queue_session_change(actor, change);
    }
}

/// [`drop_control`] without the push: the row goes, and the `shipid` change
/// back to the hull is returned for the caller to queue (alone or merged with
/// a change of its own).
pub(crate) fn drop_control_change(
    reg: &mut Registry,
    actor: i64,
) -> Option<Vec<(PyValue, PyValue)>> {
    let (id, _) = reg
        .ext::<Docked>()
        .controls
        .iter()
        .find(|(_, pilot)| **pilot == actor)
        .map(|(id, p)| (*id, *p))?;
    reg.ext_mut::<Docked>().controls.remove(&id);
    let hull = reg.character(actor).map(|c| c.ship_id)?;
    Some(shipid(id, hull))
}

/// `structureControl.BoardStructure`: from space, for a flex structure. An
/// Upwell structure is docked in, so this is refused with a reason.
pub fn board() -> ServiceError {
    notify("This structure cannot be boarded from space.")
}

/// What a `List(flag=...)` on the inventory of a structure `actor` controls
/// answers: the bay or slot's own rows that the owning corporation holds. `None` when the call is not one of those, and
/// the caller reads the inventory as the hangar, as before.
pub fn bay_rows(
    reg: &mut Registry,
    actor: Option<i64>,
    container: Option<i64>,
    flag: Option<i64>,
) -> Option<Vec<PyValue>> {
    let (actor, id, flag) = (actor?, container?, flag?);
    if !is_bay_flag(flag) || controlling(reg, actor) != Some(id) {
        return None;
    }
    // The setting is rechecked: a profile may have changed since he sat down.
    // On a refusal the seat goes and he is put back in his hull.
    let s = structure::get(reg.deltas(), id)?;
    if !sp::has_access(reg, &s, actor, sp::SETTING_DEFENSE_CAN_CONTROL_STRUCTURE) {
        drop_control(reg, actor, true);
        return None;
    }
    Some(
        reg.store()
            .contents_of(id)
            .filter(|i| i.flag_id == flag && i.owner_id == s.owner_corp)
            .map(|i| reg.item_row_value(i))
            .collect(),
    )
}
