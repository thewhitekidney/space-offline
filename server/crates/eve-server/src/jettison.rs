//! **Jettison and scoop** (GAPS-2, X-GAPS item 2; lane note
//! `docs/lanes/gaps-2.md`): cargo out into a can in space, and a can back
//! into the hold.
//!
//! # Jettison -- `ship.Jettison(itemIDs)`
//!
//! `invItemFunctions.Jettison:56-72` [V]: after a confirmation (`ConfirmJettison`)
//! the client calls `sm.StartService('gameui').GetShipAccess().Jettison(
//! itemIDs)` and unpacks **two** results, `jettisonedToCanIDs,
//! jettisonItemList`; whatever is in `itemIDs` but in neither is "not
//! jettisoned" and raises `NotAllItemsWereJettisoned` with the types
//! (`:65-72`). The first list is the ids that went into a can; the second
//! ones launched as themselves (a container jettisoned whole). The client
//! says no more; the shapes -- two lists in a tuple -- are the answer EvEJS
//! records from a retail capture ("answered `([itemID], [])`") **[I]**
//! (facts only: its `Handle_Jettison`).
//!
//! What this server does:
//!
//! - **Which items**: the caller's own, in the active ship, in a flag of
//!   `const.jettisonableFlags` -- cargo and the specialised holds ([`JETTISONABLE_FLAGS`],
//!   read from `inventorycommon/const.pyc`) **[V]** -- and not singletons.
//!   **An assembled container (a singleton of groups 12, 340, 448, 649) is
//!   launched as itself** (#82, [`jettison_items`], `crate::mtu`): it keeps its
//!   id, its name, its password and everything in it, and goes into the
//!   **second** list. Any other singleton stays, and the client names it
//!   (`NotAllItemsWereJettisoned`).
//! - **The can** is a store item of [`crate::item::JETCAN_TYPES`] (type 23,
//!   "Cargo Container"), **owned by the pilot**, at the pilot's solar system
//!   and [`CAN_OFFSET`] metres from the ship, and a [`crate::combat::Wreck`]
//!   record, so the grid draws it (`AddBalls2` at the next reconcile), the
//!   solar system's `invbroker` lists it, "Loot All" and a drag take from it
//!   (`combat::plan_loot`), a taker without loot rights is a thief
//!   (`combat::has_loot_right`, `crimewatch::theft_from`) and it is destroyed
//!   with its contents when its clock runs out. Contents are located *in* the
//!   can at flag 0 (`hacking_loot::FLAG_IN_CAN`, the wreck's).
//! - **Its lifetime** is `combat::WRECK_LIFETIME` from creation (two hours,
//!   not extended by adding to it) **[I]**: EvEJS's `JETCAN_LIFETIME_MS`
//!   states "jetcans last exactly 2 hours from creation, regardless of
//!   whether items are added or removed", the same number as the wreck's
//!   existing **[A]**. Falsified by a retail can that outlives two hours.
//! - **The cooldown**: one new can per [`COOLDOWN`] (three minutes) per
//!   pilot **[I]** (EvEJS `JETTISON_COOLDOWN_MS`); inside it a jettison adds
//!   to the pilot's newest can when that is within loot range, and otherwise
//!   raises `ShpJettisonPending` with the time left (key and `eta` parameter
//!   [V], `findings/user-errors.tsv`; the string's form is EvEJS's, "2
//!   minutes and 44 seconds"). The cooldown is read off the can's own
//!   record (`expires_at - WRECK_LIFETIME` is its birth), so no state is
//!   kept and a restart, which removes every can, clears it too.
//! - **Capacity**: the can's own, 55,000 m3. What does not fit stays in the
//!   hold and is named by the client's own error.
//!
//! # Scoop -- `ship.Scoop(objectID[, password])`
//!
//! `menusvc.RealScoop:3727-3758` [V]: the client checks illegality and its
//! safety setting, then calls `Scoop(objectID)` -- or `Scoop(objectID,
//! password)` after a `ShpScoopSecureCC` raise, which asks for the password
//! of a secure container. The menu offers it for a cargo or freight
//! container that is free in space (`celestialCheckers.IsScoopable:1965-1991`)
//! [V]. The answer is dropped.
//!
//! A can in space (a [`crate::combat::Wreck`] record of a cargo-container
//! group) within loot range goes into the active ship's cargo hold **whole**,
//! contents inside it and now the scooper's. The hold must have room for the
//! can *as the store counts it*: the type's own volume, 55,000 m3 for a
//! jetcan [V] -- the server's `used_volume_in` does not count what is inside
//! a singleton, and the client's own figures for the type say the same -- so
//! only a very large hold can scoop a jetcan. **[A]** that this is what
//! retail does; falsified by a retail scoop of a jetcan into a small hold.
//! A can that is not the scooper's is theirs to take with the loot rules of a
//! wreck: no rights, a suspect where there are suspects. There are no
//! passwords: nothing sets one, so none is asked for.
//!
//! The kinds of scoop that need things this server does not have --
//! `ScoopToFleetHangar`, `ScoopToInfrastructureHold`, `ScoopToMobileDepotHold`,
//! `ScoopToSMA`, `LaunchFromContainer` -- are not here.

use crate::combat::{self, LOOT_RANGE, LOOT_RANGE_SLACK, WRECK_LIFETIME, Wreck};
use crate::damage::BlueTime;
use crate::destiny;
use crate::fitting::{FLAG_CARGO, Refusal};
use crate::item::{Edit, Item, Slot};
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;

/// The jetcan's type: "Cargo Container" (`crate::item::JETCAN_TYPES`) [V].
pub const JETCAN_TYPE: i64 = 23;

/// `groupCargoContainer` [V] (`repackaging/__init__.py:26`).
pub const GROUP_CARGO_CONTAINER: i64 = 12;

/// `inventorycommon/const.jettisonableFlags` [V]: cargo, general mining,
/// gas, mineral, ammo, planetary commodities, asteroid, ice, structure fuel,
/// colony resources and expedition holds (flags 5, 134, 135, 136, 143, 149,
/// 172, 181, 182, 185, 188).
pub const JETTISONABLE_FLAGS: [i64; 11] = [5, 134, 135, 136, 143, 149, 172, 181, 182, 185, 188];

/// One new can per pilot per this long, in blue ticks (100 ns). **[I]**
/// three minutes, EvEJS's `JETTISON_COOLDOWN_MS`; falsified by a retail
/// second jettison inside three minutes that makes a second can.
pub const COOLDOWN: BlueTime = 3 * 60 * 10_000_000;

/// Where a can appears, metres from the ship along +x. **[A]**: nothing in
/// the client says; far enough that it is not inside the hull, near enough
/// that the pilot is in loot range. Falsified by a can drawn on top of the
/// ship.
pub const CAN_OFFSET: f64 = 100.0;

/// `flagNone`, the flag of an item lying in a can: the wreck's
/// ([`combat::FLAG_IN_WRECK`]).
const FLAG_IN_CAN: i64 = combat::FLAG_IN_WRECK;

/// A container item the store holds loose in a solar system: a can whose
/// record this run does not have. [`combat`]'s first sweep after a start
/// destroys these with the wrecks (a restart takes everything out of space).
pub fn is_can_in_space(i: &Item) -> bool {
    i.group_id == GROUP_CARGO_CONTAINER
        && i.flag_id == combat::FLAG_IN_SPACE
        && (30_000_000..33_000_000).contains(&i.location_id)
}

fn raise(key: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, Vec::new()))
}

fn notify(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
    ))
}

/// "2 minutes and 44 seconds", the `ShpJettisonPending` `{eta}`: whole
/// seconds rounded up, at least one.
pub fn eta_text(ticks: BlueTime) -> String {
    let secs = (ticks.max(0) + 9_999_999) / 10_000_000;
    let secs = secs.max(1);
    let (m, s) = (secs / 60, secs % 60);
    let plural = |n: i64, unit: &str| format!("{n} {unit}{}", if n == 1 { "" } else { "s" });
    match (m, s) {
        (0, s) => plural(s, "second"),
        (m, 0) => plural(m, "minute"),
        (m, s) => format!("{} and {}", plural(m, "minute"), plural(s, "second")),
    }
}

/// When a can was made, off its record.
fn born(wr: &Wreck) -> BlueTime {
    wr.expires_at.saturating_sub(WRECK_LIFETIME)
}

/// The pilot's cans in `system`, newest first.
fn cans_of(reg: &Registry, owner: i64, system: i64) -> Vec<Wreck> {
    let mut v: Vec<Wreck> = reg
        .combat()
        .wrecks()
        .filter(|w| w.owner_id == owner && w.solar_system_id == system)
        .filter(|w| {
            reg.item(w.wreck_id)
                .is_some_and(|i| i.group_id == GROUP_CARGO_CONTAINER)
        })
        .copied()
        .collect();
    v.sort_by_key(|w| std::cmp::Reverse(born(w)));
    v
}

/// The pilot, the ship they fly and where it is now.
struct Pilot {
    ship_id: i64,
    ship_type: i64,
    corp: i64,
    system: i64,
    at: [f64; 3],
}

fn pilot(reg: &mut Registry, actor: Option<i64>) -> Result<(i64, Pilot), ServiceError> {
    let Some(me) = actor else {
        return Err(notify("No character is selected."));
    };
    let Some(c) = reg.character(me).cloned() else {
        return Err(notify("No character is selected."));
    };
    reg.advance_character(me);
    let Some(p) = reg
        .roster()
        .get(me)
        .filter(|p| p.ship_id == c.ship_id)
        .copied()
    else {
        return Err(notify("You must be in space to do that."));
    };
    Ok((
        me,
        Pilot {
            ship_id: c.ship_id,
            ship_type: c.ship_type_id,
            corp: c.corporation_id,
            system: p.solar_system_id,
            at: p.motion.position,
        },
    ))
}

fn surface_distance(reg: &Registry, p: &Pilot, at: [f64; 3]) -> f64 {
    let radius = destiny::hull::of_in(reg.content(), p.ship_type).radius;
    crate::space_state::distance(p.at, at) - f64::from(radius) - f64::from(destiny::WRECK_RADIUS)
}

/// `ship.Jettison(itemIDs)`: the ids that went into a can. See the module
/// docs for every rule.
pub fn jettison(
    reg: &mut Registry,
    actor: Option<i64>,
    item_ids: &[i64],
) -> Result<Vec<i64>, ServiceError> {
    let (me, p) = pilot(reg, actor)?;
    let wanted: Vec<Item> = item_ids
        .iter()
        .filter_map(|id| reg.store().get(*id))
        .filter(|i| {
            i.owner_id == me
                && i.location_id == p.ship_id
                && JETTISONABLE_FLAGS.contains(&i.flag_id)
                && !i.singleton
        })
        .cloned()
        .collect();
    if wanted.is_empty() {
        return Err(raise("NothingToJettison"));
    }
    let now = crate::pyutil::filetime_now();
    let mine = cans_of(reg, me, p.system);
    let newest = mine.first().copied();
    let (can_id, fresh) = match newest {
        Some(wr)
            if now.saturating_sub(born(&wr)) < COOLDOWN
                && surface_distance(reg, &p, wr.position) <= LOOT_RANGE + LOOT_RANGE_SLACK =>
        {
            // Inside the cooldown: keep filling the pilot's own can.
            (wr.wreck_id, false)
        }
        Some(wr) if now.saturating_sub(born(&wr)) < COOLDOWN => {
            return Err(ServiceError::Raise(eve_proto::user_error(
                "ShpJettisonPending",
                crate::pyutil::pypairs(vec![(
                    "eta",
                    PyValue::Unicode(eta_text(COOLDOWN - now.saturating_sub(born(&wr)))),
                )]),
            )));
        }
        _ => {
            let at = [p.at[0] + CAN_OFFSET, p.at[1], p.at[2]];
            let create = [Edit::Create {
                type_id: JETCAN_TYPE,
                quantity: 1,
                to: Slot::new(me, p.system, combat::FLAG_IN_SPACE),
            }];
            let id = reg
                .apply_items_quiet(&create)
                .ok()
                .and_then(|changes| {
                    changes.iter().find_map(|c| match c {
                        crate::item::Change::Created { item_id, .. } => Some(*item_id),
                        _ => None,
                    })
                })
                .ok_or_else(|| notify("The server could not make a container."))?;
            combat::add_wreck(
                reg,
                Wreck {
                    wreck_id: id,
                    type_id: JETCAN_TYPE,
                    solar_system_id: p.system,
                    position: at,
                    ship_id: p.ship_id,
                    owner_id: me,
                    corp_id: p.corp,
                    fleet_id: combat::fleet_of(reg, me),
                    abandoned: false,
                    expires_at: now.saturating_add(WRECK_LIFETIME),
                },
            );
            (id, true)
        }
    };
    // What the can can still hold: its capacity less what is in it.
    let capacity = crate::item::find_in(reg.content(), JETCAN_TYPE).map_or(0.0, |t| t.capacity);
    let mut free = capacity
        - reg
            .store()
            .used_volume_in(reg.content(), can_id, FLAG_IN_CAN);
    let mut moved = Vec::new();
    let mut edits = Vec::new();
    for i in &wanted {
        let volume = crate::item::find_in(reg.content(), i.type_id).map_or(0.0, |t| t.volume)
            * i.quantity as f64;
        if volume > free {
            continue;
        }
        free -= volume;
        moved.push(i.item_id);
        edits.push(Edit::Move {
            item_id: i.item_id,
            to: Slot::new(me, can_id, FLAG_IN_CAN),
        });
    }
    if edits.is_empty() {
        if fresh {
            // Nothing fit and the can is empty: it was never worth making.
            combat::remove_wreck(reg, can_id);
        }
        return Ok(Vec::new());
    }
    reg.apply_items(&edits).map_err(|e| {
        crate::outln!("[jettison] the plan was refused: {e}");
        notify("The server could not move that.")
    })?;
    Ok(moved)
}

/// `ship.Jettison(itemIDs)` with containers launched whole (#82): `(to_can,
/// launched)`, the two lists the client unpacks. An assembled container of the
/// pilot's, in a jettisonable flag of the active ship, goes into space as
/// itself, one [`CAN_OFFSET`] further along for each; everything else is
/// [`jettison`]'s. With nothing launched [`jettison`]'s refusal stands; with
/// something launched what did not go is left to the client to name.
pub fn jettison_items(
    reg: &mut Registry,
    actor: Option<i64>,
    item_ids: &[i64],
) -> Result<(Vec<i64>, Vec<i64>), ServiceError> {
    let (me, p) = pilot(reg, actor)?;
    let whole: Vec<Item> = item_ids
        .iter()
        .filter_map(|id| reg.store().get(*id))
        .filter(|i| {
            i.owner_id == me
                && i.location_id == p.ship_id
                && JETTISONABLE_FLAGS.contains(&i.flag_id)
                && crate::containers::is_container(i)
        })
        .cloned()
        .collect();
    let mut launched = Vec::new();
    for (n, c) in whole.iter().enumerate() {
        let at = [p.at[0] + CAN_OFFSET * (n as f64 + 1.0), p.at[1], p.at[2]];
        let parked = crate::mtu::Parked {
            owner: me,
            corp: p.corp,
            ship_id: p.ship_id,
            system: p.system,
            at,
        };
        match crate::mtu::park(reg, c.item_id, &parked, crate::mtu::CONTAINER_LIFETIME, 0) {
            Ok(id) => launched.push(id),
            Err(e) => crate::outln!("[jettison] container {} stayed: {e:?}", c.item_id),
        }
    }
    let rest: Vec<i64> = item_ids
        .iter()
        .copied()
        .filter(|id| !whole.iter().any(|c| c.item_id == *id))
        .collect();
    let to_can = if rest.is_empty() && !launched.is_empty() {
        Vec::new()
    } else {
        match jettison(reg, actor, &rest) {
            Ok(v) => v,
            Err(_) if !launched.is_empty() => Vec::new(),
            Err(e) => return Err(e),
        }
    };
    Ok((to_can, launched))
}

/// `ship.Scoop(objectID[, password])`: a can into the active ship's cargo.
pub fn scoop(reg: &mut Registry, actor: Option<i64>, object_id: i64) -> Result<(), ServiceError> {
    scoop_with(reg, actor, object_id, None, FLAG_CARGO)
}

/// [`scoop`] with the password the client sent after `ShpScoopSecureCC`.
pub fn scoop_with(
    reg: &mut Registry,
    actor: Option<i64>,
    object_id: i64,
    password: Option<&str>,
    flag: i64,
) -> Result<(), ServiceError> {
    // A secure container asks for its password (`menusvc.RealScoop`, #82): the
    // one it has now, or one the pilot already entered for it.
    if let (Some(me), Some(c)) = (actor, reg.item(object_id).cloned())
        && crate::containers::is_container(&c)
        && !crate::containers::scoop_open(reg, me, &c, password)
    {
        return Err(raise("ShpScoopSecureCC"));
    }
    scoop_into(reg, actor, object_id, flag)
}

/// `ship.ScoopToFleetHangar(objectID)`: a can into the active ship's fleet
/// hangar (flag 155, `hasFleetHangars` hulls only: capacity attribute 912).
/// Only the owner may scoop into it [A: fleet and corp hangar access rules
/// are out of scope; falsifier: a fleet mate's scoop into your hangar].
pub fn scoop_to_fleet_hangar(
    reg: &mut Registry,
    actor: Option<i64>,
    object_id: i64,
) -> Result<(), ServiceError> {
    scoop_into(reg, actor, object_id, crate::ship_config::FLAG_FLEET_HANGAR)
}

/// [`scoop`] into the hold at `flag` of the active ship.
pub fn scoop_into(
    reg: &mut Registry,
    actor: Option<i64>,
    object_id: i64,
    flag: i64,
) -> Result<(), ServiceError> {
    let (me, p) = pilot(reg, actor)?;
    if flag != FLAG_CARGO && crate::item::hold_capacity_in(reg.content(), p.ship_type, flag) <= 0.0
    {
        return Err(notify("Your ship has no such hold."));
    }
    let Some(wr) = reg.combat().wreck(object_id).copied() else {
        return Err(notify("That is gone."));
    };
    // #172: a Mobile Warp Disruptor, scooped only when unanchored.
    if crate::anchor::is_unit(reg, object_id) {
        return crate::anchor::scoop_unit(reg, me, object_id, flag);
    }
    // A Mobile Depot or MTU: its owner's scoop (#82).
    if let Some(unit) = reg
        .item(object_id)
        .filter(|i| crate::mtu::is_unit(reg.content(), i))
        .cloned()
    {
        return crate::mtu::scoop_unit(reg, me, &unit, flag);
    }
    let Some(can) = reg
        .item(object_id)
        .filter(|i| i.group_id == GROUP_CARGO_CONTAINER || crate::containers::is_container(i))
        .cloned()
    else {
        return Err(notify("You cannot scoop that."));
    };
    if wr.solar_system_id != p.system {
        return Err(notify("That is gone."));
    }
    if surface_distance(reg, &p, wr.position) > LOOT_RANGE + LOOT_RANGE_SLACK {
        return Err(Refusal::TooFar {
            maxdist: LOOT_RANGE,
        }
        .to_error());
    }
    crate::fitting::check_room_in_hold(reg.content(), reg.store(), p.ship_id, can.type_id, 1, flag)
        .map_err(|r| r.to_error())?;
    let mut edits = vec![Edit::Move {
        item_id: can.item_id,
        to: Slot::new(me, p.ship_id, flag),
    }];
    // The contents stay inside it, and are the scooper's now.
    let mut open = vec![can.item_id];
    while let Some(container) = open.pop() {
        for i in reg.store().contents_of(container) {
            edits.push(Edit::Move {
                item_id: i.item_id,
                to: Slot::new(me, i.location_id, i.flag_id),
            });
            open.push(i.item_id);
        }
    }
    let thief = !combat::has_loot_right(reg, me, &wr);
    reg.apply_items(&edits).map_err(|e| {
        crate::outln!("[jettison] the scoop was refused: {e}");
        notify("The server could not move that.")
    })?;
    combat::release_wreck(reg, object_id);
    crate::mtu::forget(reg, object_id);
    if thief {
        crate::crimewatch::theft_from(
            reg,
            me,
            Some(wr.owner_id),
            wr.solar_system_id,
            crate::pyutil::filetime_now(),
        );
    }
    Ok(())
}

/// `ship.Jettison`'s answer: `(jettisonedToCanIDs, jettisonItemList)`.
pub fn jettison_value(to_can: Vec<i64>) -> PyValue {
    jettison_result(to_can, Vec::new())
}

/// [`jettison_value`] with the containers that launched as themselves in the
/// second list.
pub fn jettison_result(to_can: Vec<i64>, launched: Vec<i64>) -> PyValue {
    let list = |v: Vec<i64>| PyValue::List(v.into_iter().map(PyValue::Int).collect());
    PyValue::Tuple(vec![list(to_can), list(launched)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Content;
    use crate::model::{Character, SandboxConfig};
    use crate::typedata;
    use std::sync::Arc;

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;
    const ALICE_SHIP: i64 = 100_000_001;
    const BOB_SHIP: i64 = 100_000_002;
    const SYSTEM: i64 = 30_000_001;
    /// Invented rows: a hauler with a very big hold, a stackable that takes
    /// a lot of room and a plain stackable. **Not the client's numbers.**
    const HAULER: i64 = 990_920_001;
    const BULKY: i64 = 990_920_002;
    const PLAIN_HULL: i64 = 990_920_003;
    /// A stackable of the fixture's item table (the wallet tests' one).
    const TRIT: i64 = 34;
    const ROWS: &str = "\
type\t990920001\t99920\t6\t1\t1\t50000.0\t200000.0\t10000000.0\t100.0\t0\t\t912=60000.0\tFixture Hauler
type\t990920003\t99920\t6\t1\t1\t50000.0\t200000.0\t10000000.0\t100.0\t0\t\t\tFixture Plain Hull\ntype\t990920002\t99921\t4\t1\t1\t30000.0\t0.0\t0.0\t1.0\t0\t\t\tFixture Bulky Goods
";

    fn world() -> Registry {
        world_with(HAULER)
    }

    /// Bob flies `bob_hull`; Alice flies the hauler.
    fn world_with(bob_hull: i64) -> Registry {
        let mut content = Content::fixture();
        let text = format!("{}{ROWS}", typedata::FIXTURE);
        content.insert(
            "typedata",
            typedata::Installed::build(typedata::Table::parse(&text).expect("rows parse")),
        );
        let pilot = |id: i64, name: &str, ship: i64, hull: i64| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ship_type_id: hull,
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                pilot(ALICE, "Alice", ALICE_SHIP, HAULER),
                pilot(BOB, "Bob", BOB_SHIP, bob_hull),
            ],
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(content));
        let mut items = reg.store().items().to_vec();
        for i in items.iter_mut() {
            if i.item_id == ALICE_SHIP || i.item_id == BOB_SHIP {
                i.type_id = if i.item_id == BOB_SHIP {
                    bob_hull
                } else {
                    HAULER
                };
                i.group_id = 99_920;
            }
        }
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 200_000,
            ..crate::store::Snapshot::default()
        });
        let _ = reg.take_pending();
        for (who, ship) in [(ALICE, ALICE_SHIP), (BOB, BOB_SHIP)] {
            reg.roster_mut()
                .arrive(crate::space_state::Presence::arriving(
                    who,
                    ship,
                    SYSTEM,
                    crate::space_state::Motion::arrived([0.0, 0.0, 0.0], [0.0; 3]),
                ));
        }
        reg
    }

    fn make(reg: &mut Registry, type_id: i64, quantity: i64, to: Slot) -> i64 {
        let changes = reg
            .apply_items(&[Edit::Create {
                type_id,
                quantity,
                to,
            }])
            .expect("a known type");
        let _ = reg.take_pending();
        changes
            .iter()
            .find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .expect("created")
    }

    fn cargo(reg: &mut Registry, owner: i64, ship: i64, type_id: i64, n: i64) -> i64 {
        make(reg, type_id, n, Slot::new(owner, ship, FLAG_CARGO))
    }

    fn only_can(reg: &Registry) -> Wreck {
        let v: Vec<Wreck> = reg
            .combat()
            .wrecks()
            .filter(|w| w.type_id == JETCAN_TYPE)
            .copied()
            .collect();
        assert_eq!(v.len(), 1, "one can: {v:?}");
        v[0]
    }

    /// The `UserError` key a refusal raises.
    fn user_error_key(e: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            return format!("{e:?}");
        };
        let PyValue::Tuple(h) = &**header else {
            return format!("{e:?}");
        };
        let Some(PyValue::Tuple(call)) = h.get(1) else {
            return format!("{e:?}");
        };
        match call.first() {
            Some(PyValue::Bytes(k)) => String::from_utf8_lossy(k).into_owned(),
            _ => format!("{e:?}"),
        }
    }

    /// The cargo goes into a new can: the pilot's, in the system, with a
    /// record that lives two hours, the cargo located in it at flag 0.
    #[test]
    fn a_jettison_makes_the_pilots_can_and_fills_it() {
        let mut reg = world();
        let a = cargo(&mut reg, ALICE, ALICE_SHIP, TRIT, 500);
        let b = make(
            &mut reg,
            TRIT,
            7,
            Slot::new(ALICE, ALICE_SHIP, JETTISONABLE_FLAGS[1]),
        );
        let before = crate::pyutil::filetime_now();
        let moved = jettison(&mut reg, Some(ALICE), &[a, b]).expect("jettisoned");
        assert_eq!(moved, vec![a, b]);
        let wr = only_can(&reg);
        let can = reg.item(wr.wreck_id).expect("the can is a store item");
        assert_eq!(
            (can.owner_id, can.location_id, can.group_id, can.singleton),
            (ALICE, SYSTEM, GROUP_CARGO_CONTAINER, true)
        );
        assert_eq!((wr.owner_id, wr.solar_system_id), (ALICE, SYSTEM));
        assert!(
            wr.expires_at >= before + WRECK_LIFETIME,
            "two hours from now"
        );
        assert!(
            (wr.position[0] - CAN_OFFSET).abs() < 1e-6,
            "beside the ship: {:?}",
            wr.position
        );
        for id in [a, b] {
            let i = reg.item(id).expect("still there");
            assert_eq!((i.location_id, i.flag_id), (wr.wreck_id, FLAG_IN_CAN));
            assert_eq!(i.owner_id, ALICE);
        }
        // The hold is empty of it.
        assert_eq!(reg.store().contents_of(ALICE_SHIP).count(), 0);
        // The server's answer shape.
        let PyValue::Tuple(t) = jettison_value(moved) else {
            panic!("a tuple")
        };
        assert_eq!(
            t,
            vec![
                PyValue::List(vec![PyValue::Int(a), PyValue::Int(b)]),
                PyValue::List(vec![])
            ]
        );
    }

    /// What is not the caller's, not in the ship, not in a jettisonable
    /// flag or a singleton does not go; with nothing left the client's own
    /// `NothingToJettison`; with something left the rest goes and the
    /// client names what did not (`NotAllItemsWereJettisoned`).
    #[test]
    fn only_what_the_pilot_may_jettison_goes() {
        let mut reg = world();
        let mine = cargo(&mut reg, ALICE, ALICE_SHIP, TRIT, 10);
        let bobs = cargo(&mut reg, BOB, BOB_SHIP, TRIT, 10);
        let fitted = make(&mut reg, TRIT, 1, Slot::new(ALICE, ALICE_SHIP, 27));
        let e = jettison(&mut reg, Some(ALICE), &[bobs, fitted, 12_345]).unwrap_err();
        assert!(user_error_key(&e).contains("NothingToJettison"), "{e:?}");
        assert!(reg.combat().wrecks().next().is_none(), "no can for nothing");
        let moved = jettison(&mut reg, Some(ALICE), &[mine, bobs, fitted]).expect("some");
        assert_eq!(moved, vec![mine]);
        assert_eq!(reg.item(bobs).unwrap().location_id, BOB_SHIP);
        assert_eq!(reg.item(fitted).unwrap().location_id, ALICE_SHIP);
    }

    /// Docked, or with no character: refused, nothing made.
    #[test]
    fn a_jettison_needs_a_pilot_in_space() {
        let mut reg = world();
        let a = cargo(&mut reg, ALICE, ALICE_SHIP, TRIT, 10);
        reg.roster_mut().leave(ALICE);
        assert!(jettison(&mut reg, Some(ALICE), &[a]).is_err());
        assert!(jettison(&mut reg, None, &[a]).is_err());
        assert_eq!(reg.item(a).unwrap().location_id, ALICE_SHIP);
    }

    /// The can's capacity: what does not fit stays, in no list.
    #[test]
    fn the_can_holds_only_its_capacity() {
        let mut reg = world();
        // 30,000 m3 each: one fits a 55,000 m3 can, two do not.
        let one = cargo(&mut reg, ALICE, ALICE_SHIP, BULKY, 1);
        let two = cargo(&mut reg, ALICE, ALICE_SHIP, BULKY, 1);
        let moved = jettison(&mut reg, Some(ALICE), &[one, two]).expect("one fits");
        assert_eq!(moved, vec![one]);
        assert_eq!(reg.item(two).unwrap().location_id, ALICE_SHIP);
    }

    /// Inside the cooldown a second jettison adds to the same can; out of
    /// its range it is refused with the time left; after the cooldown it
    /// makes a second can.
    #[test]
    fn the_cooldown_reuses_the_can_then_refuses_then_lets_go() {
        let mut reg = world();
        let a = cargo(&mut reg, ALICE, ALICE_SHIP, TRIT, 10);
        jettison(&mut reg, Some(ALICE), &[a]).expect("first");
        let first = only_can(&reg);
        let b = cargo(&mut reg, ALICE, ALICE_SHIP, TRIT, 20);
        jettison(&mut reg, Some(ALICE), &[b]).expect("second, same can");
        assert_eq!(only_can(&reg).wreck_id, first.wreck_id);
        assert_eq!(reg.item(b).unwrap().location_id, first.wreck_id);

        // Fly off: the can is out of reach and the cooldown is running.
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                ALICE,
                ALICE_SHIP,
                SYSTEM,
                crate::space_state::Motion::arrived([50_000.0, 0.0, 0.0], [0.0; 3]),
            ));
        let c = cargo(&mut reg, ALICE, ALICE_SHIP, TRIT, 30);
        let e = jettison(&mut reg, Some(ALICE), &[c]).unwrap_err();
        let text = user_error_key(&e);
        assert!(text.contains("ShpJettisonPending"), "{text}");
        assert_eq!(reg.item(c).unwrap().location_id, ALICE_SHIP);

        // Age the can past the cooldown: a second can.
        let mut aged = first;
        aged.expires_at = aged.expires_at - COOLDOWN - 1;
        combat::add_wreck(&mut reg, aged);
        jettison(&mut reg, Some(ALICE), &[c]).expect("a new can");
        assert_eq!(
            reg.combat()
                .wrecks()
                .filter(|w| w.type_id == JETCAN_TYPE)
                .count(),
            2
        );
    }

    #[test]
    fn the_eta_reads_like_the_clients_template_wants() {
        let s = 10_000_000;
        assert_eq!(eta_text(44 * s), "44 seconds");
        assert_eq!(eta_text(60 * s), "1 minute");
        assert_eq!(eta_text(164 * s), "2 minutes and 44 seconds");
        assert_eq!(eta_text(60 * s - 1), "1 minute");
        assert_eq!(eta_text(0), "1 second");
    }

    /// GAP-LOOT: a can takes the jettisoner's fleet when it is made.
    #[test]
    fn a_can_carries_the_jettisoners_fleet() {
        let mut reg = world();
        let f = reg.fleets_mut().create(ALICE).expect("fleet");
        reg.fleets_mut()
            .init(ALICE, f, Some(670), None, Some(SYSTEM))
            .expect("init");
        reg.fleets_mut()
            .invite(ALICE, f, BOB, None, None, None)
            .expect("invite");
        reg.fleets_mut()
            .accept_invite(BOB, f, Some(670), None, None)
            .expect("accept");
        let a = cargo(&mut reg, ALICE, ALICE_SHIP, TRIT, 5);
        jettison(&mut reg, Some(ALICE), &[a]).expect("a can");
        let wr = only_can(&reg);
        assert_eq!((wr.fleet_id, wr.abandoned), (Some(f), false));
        let (_, slim) = combat::wreck_parts(&reg, &wr);
        assert_eq!(slim.fleet_id, Some(f));
    }

    /// Scoop: a can within reach goes into the hold whole, its contents
    /// still inside and now the scooper's, and the ball leaves space.
    #[test]
    fn a_scoop_takes_the_can_and_what_is_in_it() {
        let mut reg = world();
        // Bob jettisons; Alice scoops (a theft where there are suspects, not
        // a refusal).
        let bob_stack = cargo(&mut reg, BOB, BOB_SHIP, TRIT, 40);
        jettison(&mut reg, Some(BOB), &[bob_stack]).expect("Bob's can");
        let wr = only_can(&reg);
        scoop(&mut reg, Some(ALICE), wr.wreck_id).expect("scooped");
        let can = reg.item(wr.wreck_id).expect("the can is kept");
        assert_eq!(
            (can.owner_id, can.location_id, can.flag_id),
            (ALICE, ALICE_SHIP, FLAG_CARGO)
        );
        let inside = reg.item(bob_stack).expect("the stack");
        assert_eq!(
            (inside.owner_id, inside.location_id),
            (ALICE, wr.wreck_id),
            "inside the can, the scooper's"
        );
        assert!(
            reg.combat().wreck(wr.wreck_id).is_none(),
            "no longer in space"
        );
        // And it can be scooped no more.
        assert!(scoop(&mut reg, Some(ALICE), wr.wreck_id).is_err());
    }

    /// The fleet hangar: the can goes to flag 155 of a hull that has one, a
    /// hull without is refused, and a full hangar raises the cargo-room key.
    #[test]
    fn a_can_scoops_to_the_fleet_hangar_of_a_hull_that_has_one() {
        let mut reg = world();
        let a = cargo(&mut reg, ALICE, ALICE_SHIP, TRIT, 10);
        jettison(&mut reg, Some(ALICE), &[a]).expect("a can");
        let wr = only_can(&reg);
        scoop_to_fleet_hangar(&mut reg, Some(ALICE), wr.wreck_id).expect("scooped");
        let can = reg.item(wr.wreck_id).expect("kept");
        assert_eq!(
            (can.location_id, can.flag_id),
            (ALICE_SHIP, crate::ship_config::FLAG_FLEET_HANGAR)
        );
        assert!(reg.combat().wreck(wr.wreck_id).is_none());
        // The hangar is now nearly full: a second can does not fit.
        let b = cargo(&mut reg, ALICE, ALICE_SHIP, TRIT, 10);
        jettison(&mut reg, Some(ALICE), &[b]).expect("another can");
        let wr2 = only_can(&reg);
        let e = scoop_to_fleet_hangar(&mut reg, Some(ALICE), wr2.wreck_id).unwrap_err();
        assert!(user_error_key(&e).contains("NotEnoughCargoSpace"), "{e:?}");
        assert!(reg.combat().wreck(wr2.wreck_id).is_some(), "still there");
    }

    #[test]
    fn a_hull_without_a_fleet_hangar_cannot_scoop_to_it() {
        let mut reg = world_with(PLAIN_HULL);
        let a = cargo(&mut reg, ALICE, ALICE_SHIP, TRIT, 10);
        jettison(&mut reg, Some(ALICE), &[a]).expect("a can");
        let wr = only_can(&reg);
        assert!(scoop_to_fleet_hangar(&mut reg, Some(BOB), wr.wreck_id).is_err());
        assert!(reg.combat().wreck(wr.wreck_id).is_some(), "still there");
    }

    /// Too far, no room, not a can.
    #[test]
    fn a_scoop_refuses_what_is_out_of_reach_or_will_not_fit() {
        let mut reg = world();
        let a = cargo(&mut reg, ALICE, ALICE_SHIP, TRIT, 10);
        jettison(&mut reg, Some(ALICE), &[a]).expect("a can");
        let wr = only_can(&reg);
        // Bob is a long way off.
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                BOB,
                BOB_SHIP,
                SYSTEM,
                crate::space_state::Motion::arrived([90_000.0, 0.0, 0.0], [0.0; 3]),
            ));
        let e = scoop(&mut reg, Some(BOB), wr.wreck_id).unwrap_err();
        assert!(user_error_key(&e).contains("NotCloseEnoughToLoot"), "{e:?}");
        // Fill Alice's hold: the can's 55,000 m3 no longer fits.
        let _ = make(&mut reg, BULKY, 5, Slot::new(ALICE, ALICE_SHIP, FLAG_CARGO));
        let e = scoop(&mut reg, Some(ALICE), wr.wreck_id).unwrap_err();
        assert!(user_error_key(&e).contains("NotEnoughCargoSpace"), "{e:?}");
        assert!(reg.combat().wreck(wr.wreck_id).is_some(), "still there");
        // Something that is no can.
        assert!(scoop(&mut reg, Some(ALICE), ALICE_SHIP).is_err());
    }

    /// A restart takes every can out of space: the first combat tick
    /// destroys a can no record holds, contents with it; a can with a record
    /// is left alone.
    #[test]
    fn a_restart_sweeps_cans_it_has_no_record_of() {
        let mut reg = world();
        let orphan = make(
            &mut reg,
            JETCAN_TYPE,
            1,
            Slot::new(ALICE, SYSTEM, combat::FLAG_IN_SPACE),
        );
        let inside = make(&mut reg, TRIT, 5, Slot::new(ALICE, orphan, FLAG_IN_CAN));
        let a = cargo(&mut reg, ALICE, ALICE_SHIP, TRIT, 10);
        jettison(&mut reg, Some(ALICE), &[a]).expect("a live can");
        let live = only_can(&reg).wreck_id;
        // Hangar containers are not in space and are not swept.
        let station = make(
            &mut reg,
            JETCAN_TYPE,
            1,
            Slot::new(ALICE, 60_000_001, crate::model::FLAG_HANGAR),
        );
        combat::tick(&mut reg, crate::pyutil::filetime_now());
        assert!(reg.item(orphan).is_none() && reg.item(inside).is_none());
        assert!(reg.item(live).is_some() && reg.item(a).is_some());
        assert!(reg.item(station).is_some());
    }
}
