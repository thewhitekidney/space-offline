//! A saved fitting is applied to the active ship, and a ship is stripped
//! (lane gap-inventory, GAP-FITAPPLY, from EVEJS-GAP svcB §3.1).
//!
//! **What the client sends [V].** `fittingSvc.LoadFitting` calls, on the
//! ship's bound inventory (`fittingSvc.py:534-608`):
//!
//! ```text
//! FitFitting(shipID, shipTypeID, itemsToFit, stationID, fitting,
//!            cargoItemsByType, fitRigs)
//! ```
//!
//! `itemsToFit` is a `defaultdict(set)` of `{typeID: {itemIDs}}`, hangar
//! stacks only; `fitting` is a `KeyVal(chargesByType, dronesByType,
//! fightersByTypeID, iceByType, modulesByFlag, implantsByTypeID)`, each a
//! `{typeID: full quantity}` dict but `modulesByFlag`, which is `{flag:
//! typeID}`; `cargoItemsByType` is `{typeID: n}` (the rigs the pilot chose
//! not to fit). The answer is a list of `(typeID, qty)` pairs for the units
//! that could not be placed, `[]` when everything went: both readers loop
//! `for typeID, qty in failedToLoad`.
//!
//! `StripFitting()` takes no arguments and its answer is ignored
//! (`invCache.py:758-767`).
//!
//! **Not atomic**, as on Tranquility: a partial fit stays partial and the
//! client shows its "missing items" dialog. Every leg goes through the
//! ordinary [`fit_one`], [`unfit_one`] and [`to_cargo`] (and the drone and
//! fighter bays' own), so each pushes its `OnItemChange` as usual.
//!
//! Assumptions, each with its falsifier:
//!
//! - **[A]** the three shapes of `itemsToFit` ([`decode`]): a plain dict, or
//!   the reduce form with the pairs in its `dict` items; values a set object,
//!   a list or a tuple. Falsifier: a capture of the call.
//! - **[A]** a slot held by another module is unfitted to the hangar first.
//!   Falsifier: TQ leaving the occupant in place.
//! - **[A]** a slot refused once is tried again after the others.
//!   Falsifier: none known; harmless when wrong.
//! - **[A]** subsystems are not stripped. Falsifier: TQ stripping a T3C's.
//! - **[A]** no cargo-overflow refusal when stripping. Falsifier: a TQ
//!   refusal key for it.

use super::*;
use std::collections::{BTreeMap, BTreeSet};

/// The rig slots, `flagRigSlot0..2` [V].
pub(super) const RIG_FLAGS: std::ops::RangeInclusive<i64> = 92..=94;
/// The subsystem slots, `flagSubSystemSlot0..3` [V].
pub(super) const SUBSYSTEM_FLAGS: std::ops::RangeInclusive<i64> = 125..=128;

/// A saved fitting's wishes, as `FitFitting` carries them.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Wish {
    pub ship_id: i64,
    pub ship_type_id: i64,
    /// `{typeID: {itemIDs}}`, the hangar stacks the client offers.
    pub pool: BTreeMap<i64, BTreeSet<i64>>,
    /// `{flag: typeID}`.
    pub modules: BTreeMap<i64, i64>,
    /// Quantities wanted in the cargo hold: charges, ice, implants.
    pub cargo: BTreeMap<i64, i64>,
    pub drones: BTreeMap<i64, i64>,
    pub fighters: BTreeMap<i64, i64>,
    /// The rigs the pilot declined to fit, for the cargo hold.
    pub rigs_to_cargo: BTreeMap<i64, i64>,
    pub fit_rigs: bool,
}

/// The ints of a Python `set` (the reduce form around `__builtin__.set`),
/// a list or a tuple.
pub(super) fn int_set(v: &PyValue, out: &mut BTreeSet<i64>) {
    match v {
        PyValue::Int(i) => {
            out.insert(*i);
        }
        PyValue::List(items) | PyValue::Tuple(items) => {
            for item in items {
                int_set(item, out);
            }
        }
        PyValue::ObjectEx { header, .. } => int_set(header, out),
        _ => {}
    }
}

/// The `(key, value)` pairs of a dict, whichever way it came over the wire.
/// A marshal dict stores `(value, key)`; a reduce-form object (a
/// `defaultdict`) carries `(key, value)` in its `__setitem__` items.
pub(super) fn pairs(v: &PyValue) -> Vec<(&PyValue, &PyValue)> {
    match v {
        PyValue::Dict(entries) => entries.iter().map(|(val, key)| (key, val)).collect(),
        PyValue::ObjectEx { dict, .. } => dict.iter().map(|(key, val)| (key, val)).collect(),
        _ => Vec::new(),
    }
}

pub(super) fn int_map(v: Option<&PyValue>) -> BTreeMap<i64, i64> {
    let mut out = BTreeMap::new();
    for (k, val) in v.map(pairs).unwrap_or_default() {
        if let (PyValue::Int(k), PyValue::Int(val)) = (k, val)
            && *val > 0
        {
            *out.entry(*k).or_insert(0) += *val;
        }
    }
    out
}

fn set_map(v: Option<&PyValue>) -> BTreeMap<i64, BTreeSet<i64>> {
    let mut out: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    for (k, val) in v.map(pairs).unwrap_or_default() {
        if let PyValue::Int(k) = k {
            int_set(val, out.entry(*k).or_default());
        }
    }
    out
}

/// Read the seven arguments; `None` when the shape is not the client's.
pub(super) fn decode(args: &[PyValue]) -> Option<Wish> {
    let (PyValue::Int(ship_id), PyValue::Int(ship_type_id)) = (args.first()?, args.get(1)?) else {
        return None;
    };
    let fitting = args.get(4)?;
    let at = |n: &str| crate::contracts::attr(fitting, n);
    let modules_by_flag = at("modulesByFlag")?;
    let mut modules = BTreeMap::new();
    // `{flag: typeID}`: the key is the flag.
    for (flag, type_id) in pairs(modules_by_flag) {
        if let (PyValue::Int(f), PyValue::Int(t)) = (flag, type_id) {
            modules.insert(*f, *t);
        }
    }
    let mut cargo = int_map(at("chargesByType"));
    for (t, q) in int_map(at("iceByType"))
        .into_iter()
        .chain(int_map(at("implantsByTypeID")))
    {
        *cargo.entry(t).or_insert(0) += q;
    }
    Some(Wish {
        ship_id: *ship_id,
        ship_type_id: *ship_type_id,
        pool: set_map(args.get(2)),
        modules,
        cargo,
        drones: int_map(at("dronesByType")),
        fighters: int_map(at("fightersByTypeID")),
        rigs_to_cargo: int_map(args.get(5)),
        fit_rigs: matches!(args.get(6), Some(PyValue::Bool(true)))
            || matches!(args.get(6), Some(PyValue::Int(n)) if *n != 0),
    })
}

/// The pilot's items of `type_id` the client offered that are still in the
/// station hangar, lowest id first.
fn offered(reg: &Registry, owner: i64, station: i64, wish: &Wish, type_id: i64) -> Vec<i64> {
    wish.pool
        .get(&type_id)
        .into_iter()
        .flatten()
        .copied()
        .filter(|id| {
            reg.store().get(*id).is_some_and(|i| {
                i.type_id == type_id
                    && i.owner_id == owner
                    && i.location_id == station
                    && i.flag_id == crate::model::FLAG_HANGAR
            })
        })
        .collect()
}

/// Fit one module of `type_id` at `flag` from the offered stacks. `false`
/// when none could go there.
fn fit_into(
    reg: &mut Registry,
    owner: i64,
    station: i64,
    wish: &Wish,
    flag: i64,
    type_id: i64,
) -> bool {
    for id in offered(reg, owner, station, wish, type_id) {
        if fit_one(reg, Some(owner), wish.ship_id, id, flag).is_ok() {
            return true;
        }
    }
    false
}

/// Units of `type_id` the ship holds at `flag`.
fn units_at(reg: &Registry, ship_id: i64, flag: i64, type_id: i64) -> i64 {
    reg.store()
        .contents_of(ship_id)
        .filter(|i| i.flag_id == flag && i.type_id == type_id)
        .map(|i| i.quantity)
        .sum()
}

/// Bring a hold up to `wanted` (`{typeID: full quantity}`): the shortfall is
/// moved from the offered stacks by split, and whatever could not be moved
/// (no stack, no room) is added to `missing`.
fn top_up(
    reg: &mut Registry,
    owner: i64,
    station: i64,
    wish: &Wish,
    flag: i64,
    wanted: &BTreeMap<i64, i64>,
    missing: &mut BTreeMap<i64, i64>,
) {
    for (&type_id, &qty) in wanted {
        let mut remaining = qty - units_at(reg, wish.ship_id, flag, type_id);
        if remaining <= 0 {
            continue;
        }
        for id in offered(reg, owner, station, wish, type_id) {
            if remaining <= 0 {
                break;
            }
            let Some(have) = reg.store().get(id).map(|i| i.quantity) else {
                continue;
            };
            let take = remaining.min(have);
            let moved = if flag == crate::drones::FLAG_DRONE_BAY {
                crate::drones::to_bay(reg, Some(owner), wish.ship_id, id, Some(take))
            } else if flag == crate::fighters::FLAG_FIGHTER_BAY {
                crate::fighters::to_bay(reg, Some(owner), wish.ship_id, id, Some(take))
            } else {
                to_cargo(reg, Some(owner), wish.ship_id, id, Some(take), flag)
            };
            if moved.is_ok() {
                remaining -= take;
            }
        }
        if remaining > 0 {
            *missing.entry(type_id).or_insert(0) += remaining;
        }
    }
}

/// The active ship's id and the pilot, once the call is known to be about
/// them: docked, and the bound container is the active ship.
fn docked_active(reg: &Registry, req: &Request<'_>) -> Result<(i64, i64, i64), ServiceError> {
    let owner = req
        .actor
        .ok_or_else(|| custom_notify("No character is selected."))?;
    let c = reg
        .character(owner)
        .ok_or_else(|| custom_notify("No character is selected."))?;
    crate::fitting::refuse_in_space(reg, c).map_err(|r| r.to_error())?;
    let ship_id = container_of(req.bind_params).unwrap_or(0);
    if ship_id != c.ship_id {
        return Err(crate::fitting::Refusal::NotActiveShip.to_error());
    }
    let station = reg
        .docked_station(owner)
        .ok_or_else(|| crate::fitting::Refusal::NotDocked.to_error())?;
    Ok((owner, ship_id, station))
}

/// `invbroker.FitFitting`, on the active ship's inventory.
pub(super) fn fit_fitting(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let wish = decode(req.args).ok_or_else(|| {
        module_bad_args(
            req,
            "(shipID, shipTypeID, itemsToFit, stationID, fitting, cargoItemsByType, fitRigs)",
        )
    })?;
    let (owner, ship_id, station) = docked_active(reg, req)?;
    let ship_ok = reg
        .store()
        .get(ship_id)
        .is_some_and(|s| s.type_id == wish.ship_type_id);
    if wish.ship_id != ship_id || !ship_ok {
        return Err(custom_notify(
            "That fitting is for another type of ship than the one you are in.",
        ));
    }
    let mut missing: BTreeMap<i64, i64> = BTreeMap::new();
    let mark = reg.pending_len();

    // The modules, lowest flag first.
    let mut again: Vec<(i64, i64)> = Vec::new();
    for (&flag, &type_id) in &wish.modules {
        let rig = RIG_FLAGS.contains(&flag);
        if rig && !wish.fit_rigs {
            continue;
        }
        let occupant = reg
            .store()
            .contents_of(ship_id)
            .find(|i| i.flag_id == flag && !crate::item::is_loaded_charge(i))
            .map(|i| (i.item_id, i.type_id));
        if let Some((occupant_id, occupant_type)) = occupant {
            if occupant_type == type_id || rig || SUBSYSTEM_FLAGS.contains(&flag) {
                // Already there, or a rig or subsystem that stays (EvEJS does
                // the same): the line is not missing.
                continue;
            }
            // SKILLGATE: a replacement the pilot cannot use leaves the occupant
            // (and its rounds) where it is.
            if !crate::skillgate::missing(reg, owner, type_id).is_empty() {
                *missing.entry(type_id).or_insert(0) += 1;
                continue;
            }
            // [A] Another module: out to the hangar first, its rounds too.
            let _ = crate::modules::unload_ammo(
                reg,
                Some(owner),
                &[occupant_id],
                &PyValue::Int(station),
                None,
                crate::pyutil::filetime_now(),
            );
            if unfit_one(reg, Some(owner), occupant_id, None).is_err() {
                *missing.entry(type_id).or_insert(0) += 1;
                continue;
            }
        }
        if !fit_into(reg, owner, station, &wish, flag, type_id) {
            again.push((flag, type_id));
        }
    }
    // [A] One more try for what was refused, after the others have settled.
    for (flag, type_id) in again {
        if !fit_into(reg, owner, station, &wish, flag, type_id) {
            *missing.entry(type_id).or_insert(0) += 1;
        }
    }

    // Rigs the pilot did not want fitted go to the hold.
    if !wish.fit_rigs {
        top_up(
            reg,
            owner,
            station,
            &wish,
            crate::fitting::FLAG_CARGO,
            &wish.rigs_to_cargo,
            &mut missing,
        );
    }
    top_up(
        reg,
        owner,
        station,
        &wish,
        crate::fitting::FLAG_CARGO,
        &wish.cargo,
        &mut missing,
    );
    top_up(
        reg,
        owner,
        station,
        &wish,
        crate::drones::FLAG_DRONE_BAY,
        &wish.drones,
        &mut missing,
    );
    top_up(
        reg,
        owner,
        station,
        &wish,
        crate::fighters::FLAG_FIGHTER_BAY,
        &wish.fighters,
        &mut missing,
    );

    // One cache drop for the batch, after every item row (FITFIX-2).
    reg.coalesce_invalidations(mark);
    Ok(PyValue::List(
        missing
            .into_iter()
            .filter(|(_, q)| *q > 0)
            .map(|(t, q)| PyValue::Tuple(vec![PyValue::Int(t), PyValue::Int(q)]))
            .collect(),
    ))
}

/// `invbroker.StripFitting`, on the active ship's inventory: every module to
/// the hangar, rounds first.
///
/// Rigs stay (unfitting destroys them, `crate::rigs`) and so do subsystems
/// [A]. A module that cannot come off (it is running) is the first error
/// raised, after the others have gone.
pub(super) fn strip(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let (owner, ship_id, station) = docked_active(reg, req)?;
    let mut modules: Vec<(i64, i64)> = reg
        .store()
        .contents_of(ship_id)
        .filter(|i| {
            crate::item::is_ship_fitting_flag(i.flag_id)
                && !crate::item::is_loaded_charge(i)
                && !RIG_FLAGS.contains(&i.flag_id)
                && !SUBSYSTEM_FLAGS.contains(&i.flag_id)
                && i.category_id != crate::subsystems::CATEGORY_SUBSYSTEM
        })
        .map(|i| (i.flag_id, i.item_id))
        .collect();
    modules.sort_unstable();
    let mut first_error = None;
    for (flag, id) in modules {
        if crate::modules::loaded(reg.store(), ship_id, flag).is_some()
            && let Err(e) = crate::modules::unload_ammo(
                reg,
                Some(owner),
                &[id],
                &PyValue::Int(station),
                None,
                crate::pyutil::filetime_now(),
            )
        {
            first_error.get_or_insert(e);
            continue;
        }
        if let Err(e) = unfit_one(reg, Some(owner), id, None) {
            first_error.get_or_insert(e);
        }
    }
    first_error.map_or(Ok(PyValue::None), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Content;
    use crate::item::Item;
    use crate::item::find_in as item_type;
    use crate::model::{Character, SandboxConfig};

    const ALICE: i64 = 990_940_100;
    const STATION: i64 = 990_940_101;
    const HULL: i64 = 990_940_001;
    const OTHER_HULL: i64 = 990_940_002;
    const GUN: i64 = 990_940_003;
    const SHIELD: i64 = 990_940_004;
    const ROUND: i64 = 990_940_005;
    const DRONE: i64 = 990_940_006;
    const RIG: i64 = 990_940_007;
    /// An invented container type, in one of the personal container groups.
    const BOX: i64 = 990_940_010;
    /// Needs Small Hybrid Turret 3, which the starter sheet lacks (SKILLGATE).
    const LOCKED: i64 = 9_409_009;
    const CAPPED: i64 = 990_940_008;
    /// The invented group the capped module belongs to.
    const CAPPED_GROUP: i64 = 990_940_900;
    const HI0: i64 = 27;
    const HI1: i64 = 28;
    const HI2: i64 = 29;
    const MED0: i64 = 19;
    const RIG0: i64 = 92;

    fn content() -> Content {
        let hull = |id: i64| {
            format!(
                "type\t{id}\t25\t6\t1\t1\t1000.0\t500.0\t1200000000.0\t300.0\t0\t\t\
                 9=1000000.0;11=45000.0;14=3;13=2;12=3;37=100.0;48=125000.0;552=30.0;\
                 600=2.0;1281=3.0;283=50.0\tFixture Hull {id}\n"
            )
        };
        let module = |id: i64, group: i64, extra: &str| {
            format!(
                "type\t{id}\t{group}\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t12\t50=1.0{extra}\tFixture Module {id}\n"
            )
        };
        let rows = format!(
            "{}{}{}{}{}\
             type\t{RIG}\t773\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t\t\tFixture Rig\n\
             type\t{BOX}\t{box_group}\t2\t1\t1\t1.0\t0.0\t100.0\t1.0\t0\t\t\tFixture Box\n\
             type\t{SHIELD}\t56\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t13\t50=1.0\tFixture Shield\n\
             type\t{ROUND}\t83\t8\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t\t\tFixture Round\n\
             type\t{DRONE}\t100\t18\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t\t\tFixture Drone\n",
            hull(HULL),
            hull(OTHER_HULL),
            module(GUN, 55, ""),
            module(CAPPED, CAPPED_GROUP, ";1544=2.0"),
            module(LOCKED, 55, ";182=3301.0;277=3.0"),
            box_group = crate::ship_config::CONTAINER_GROUPS[0],
        );
        let text = format!("{}{rows}", crate::typedata::FIXTURE);
        let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
        let mut c = Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    /// Alice, docked, in `HULL`, with the given hangar stacks
    /// `(type, quantity, singleton)`; ids from 990_941_000 in order, the
    /// hull first (id 990_941_000).
    fn world(stacks: &[(i64, i64, bool)]) -> (Registry, Vec<i64>) {
        let alice = Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            station_id: Some(STATION),
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![alice],
            ..SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content()));
        let mut items = reg.store().items().to_vec();
        let mut ids = Vec::new();
        for (n, &(type_id, quantity, singleton)) in std::iter::once(&(HULL, 1, true))
            .chain(stacks.iter())
            .enumerate()
        {
            let id = 990_941_000 + n as i64;
            let t = item_type(reg.content(), type_id).expect("a fixture type");
            items.push(Item {
                item_id: id,
                type_id,
                owner_id: ALICE,
                location_id: STATION,
                flag_id: crate::model::FLAG_HANGAR,
                quantity,
                group_id: t.group_id,
                category_id: t.category_id,
                singleton,
            });
            ids.push(id);
        }
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 990_942_000,
            ..crate::store::Snapshot::default()
        });
        let c = reg.character_mut(ALICE).expect("Alice");
        c.ship_id = ids[0];
        c.ship_type_id = HULL;
        (reg, ids)
    }

    fn int(n: i64) -> PyValue {
        PyValue::Int(n)
    }

    /// `{typeID: set(ids)}` the way a `defaultdict(set)` arrives.
    fn pool(entries: &[(i64, &[i64])]) -> PyValue {
        let set = |ids: &[i64]| PyValue::ObjectEx {
            form: eve_marshal::ObjectForm::Call,
            header: Box::new(PyValue::Tuple(vec![
                PyValue::Global(b"__builtin__.set".to_vec()),
                PyValue::Tuple(vec![PyValue::List(ids.iter().map(|i| int(*i)).collect())]),
            ])),
            list: vec![],
            dict: vec![],
        };
        PyValue::ObjectEx {
            form: eve_marshal::ObjectForm::Call,
            header: Box::new(PyValue::Tuple(vec![
                PyValue::Global(b"collections.defaultdict".to_vec()),
                PyValue::Tuple(vec![PyValue::Global(b"__builtin__.set".to_vec())]),
            ])),
            list: vec![],
            dict: entries.iter().map(|(t, ids)| (int(*t), set(ids))).collect(),
        }
    }

    /// A plain `{key: value}` marshal dict (stored value first).
    fn dict(entries: &[(i64, i64)]) -> PyValue {
        PyValue::Dict(entries.iter().map(|(k, v)| (int(*v), int(*k))).collect())
    }

    struct Fit<'a> {
        pool: PyValue,
        modules: &'a [(i64, i64)],
        charges: &'a [(i64, i64)],
        drones: &'a [(i64, i64)],
        rigs_to_cargo: &'a [(i64, i64)],
        fit_rigs: bool,
    }

    fn args(ship: i64, ship_type: i64, f: Fit<'_>) -> Vec<PyValue> {
        vec![
            int(ship),
            int(ship_type),
            f.pool,
            int(STATION),
            crate::pyutil::keyval(vec![
                ("chargesByType", dict(f.charges)),
                ("dronesByType", dict(f.drones)),
                ("fightersByTypeID", dict(&[])),
                ("iceByType", dict(&[])),
                ("modulesByFlag", dict(f.modules)),
                ("implantsByTypeID", dict(&[])),
            ]),
            dict(f.rigs_to_cargo),
            PyValue::Bool(f.fit_rigs),
        ]
    }

    fn call(
        reg: &mut Registry,
        ship: i64,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        let bind = PyValue::Tuple(vec![int(ship), int(STATION)]);
        dispatch(
            reg,
            &Request {
                service: "invbroker::inventory",
                method,
                args,
                kwargs: &[],
                bind_params: Some(&bind),
                actor: Some(ALICE),
            },
        )
    }

    fn at(reg: &Registry, ship: i64, flag: i64) -> Vec<(i64, i64)> {
        reg.store()
            .contents_of(ship)
            .filter(|i| i.flag_id == flag)
            .map(|i| (i.type_id, i.quantity))
            .collect()
    }

    fn hangar(reg: &Registry, type_id: i64) -> i64 {
        reg.store()
            .items()
            .iter()
            .filter(|i| {
                i.location_id == STATION
                    && i.flag_id == crate::model::FLAG_HANGAR
                    && i.type_id == type_id
            })
            .map(|i| i.quantity)
            .sum()
    }

    #[test]
    fn decode_reads_a_defaultdict_of_sets_and_a_plain_dict() {
        let plain = PyValue::Dict(vec![(PyValue::List(vec![int(11), int(12)]), int(GUN))]);
        for p in [pool(&[(GUN, &[11, 12])]), plain] {
            let w = decode(&args(
                5,
                HULL,
                Fit {
                    pool: p,
                    modules: &[(HI0, GUN), (MED0, SHIELD)],
                    charges: &[(ROUND, 100)],
                    drones: &[(DRONE, 2)],
                    rigs_to_cargo: &[],
                    fit_rigs: true,
                },
            ))
            .expect("decodes");
            assert_eq!(w.pool[&GUN], BTreeSet::from([11, 12]));
            assert_eq!(w.modules, BTreeMap::from([(HI0, GUN), (MED0, SHIELD)]));
            assert_eq!(w.cargo, BTreeMap::from([(ROUND, 100)]));
            assert_eq!(w.drones, BTreeMap::from([(DRONE, 2)]));
            assert!(w.fit_rigs);
        }
        assert_eq!(decode(&[int(1)]), None);
    }

    #[test]
    fn a_saved_fitting_fits_its_modules_from_the_hangar_and_reports_nothing_missing() {
        let (mut reg, ids) = world(&[(GUN, 1, true), (SHIELD, 1, true)]);
        let a = args(
            ids[0],
            HULL,
            Fit {
                pool: pool(&[(GUN, &[ids[1]]), (SHIELD, &[ids[2]])]),
                modules: &[(HI0, GUN), (MED0, SHIELD)],
                charges: &[],
                drones: &[],
                rigs_to_cargo: &[],
                fit_rigs: true,
            },
        );
        let got = call(&mut reg, ids[0], "FitFitting", &a).expect("answers");
        assert_eq!(got, PyValue::List(vec![]));
        assert_eq!(at(&reg, ids[0], HI0), vec![(GUN, 1)]);
        assert_eq!(at(&reg, ids[0], MED0), vec![(SHIELD, 1)]);
    }

    /// SKILLGATE: `fit_into` sees the gate's refusal as a module that could
    /// not be placed, so it lands in the answer's missing list.
    #[test]
    fn a_saved_fitting_reports_the_unskilled_module_missing() {
        let (mut reg, ids) = world(&[(LOCKED, 1, true), (GUN, 1, true)]);
        let a = args(
            ids[0],
            HULL,
            Fit {
                pool: pool(&[(LOCKED, &[ids[1]]), (GUN, &[ids[2]])]),
                modules: &[(HI0, LOCKED), (HI1, GUN)],
                charges: &[],
                drones: &[],
                rigs_to_cargo: &[],
                fit_rigs: true,
            },
        );
        let got = call(&mut reg, ids[0], "FitFitting", &a).expect("answers");
        assert_eq!(
            got,
            PyValue::List(vec![PyValue::Tuple(vec![int(LOCKED), int(1)])])
        );
        assert!(at(&reg, ids[0], HI0).is_empty());
        assert_eq!(at(&reg, ids[0], HI1), vec![(GUN, 1)]);
    }

    /// SKILLGATE: a saved fit whose replacement the pilot cannot use leaves
    /// the working occupant, and its rounds, in the slot.
    #[test]
    fn an_unskilled_replacement_leaves_the_occupant_and_its_rounds() {
        let (mut reg, ids) = world(&[(GUN, 1, true), (LOCKED, 1, true), (ROUND, 50, false)]);
        let fit = |reg: &mut Registry, flag_type: (i64, i64), item: i64| {
            let a = args(
                ids[0],
                HULL,
                Fit {
                    pool: pool(&[(flag_type.1, &[item])]),
                    modules: &[flag_type],
                    charges: &[],
                    drones: &[],
                    rigs_to_cargo: &[],
                    fit_rigs: true,
                },
            );
            call(reg, ids[0], "FitFitting", &a).expect("answers")
        };
        assert_eq!(fit(&mut reg, (HI0, GUN), ids[1]), PyValue::List(vec![]));
        reg.apply_items(&[crate::item::Edit::Move {
            item_id: ids[3],
            to: crate::item::Slot::new(ALICE, ids[0], HI0),
        }])
        .expect("the rounds are loaded");
        let before = at(&reg, ids[0], HI0);
        assert!(
            before.contains(&(GUN, 1)) && before.contains(&(ROUND, 50)),
            "{before:?}"
        );
        let got = fit(&mut reg, (HI0, LOCKED), ids[2]);
        assert_eq!(
            got,
            PyValue::List(vec![PyValue::Tuple(vec![int(LOCKED), int(1)])])
        );
        assert_eq!(
            at(&reg, ids[0], HI0),
            before,
            "the occupant and its rounds stay"
        );
        assert_eq!(hangar(&reg, LOCKED), 1);
    }

    #[test]
    fn missing_units_come_back_as_type_and_quantity_pairs() {
        // One gun offered for two slots, no shield at all.
        let (mut reg, ids) = world(&[(GUN, 1, true)]);
        let a = args(
            ids[0],
            HULL,
            Fit {
                pool: pool(&[(GUN, &[ids[1]])]),
                modules: &[(HI0, GUN), (HI1, GUN), (MED0, SHIELD)],
                charges: &[],
                drones: &[],
                rigs_to_cargo: &[],
                fit_rigs: true,
            },
        );
        let got = call(&mut reg, ids[0], "FitFitting", &a).expect("answers");
        assert_eq!(
            got,
            PyValue::List(vec![
                PyValue::Tuple(vec![int(GUN), int(1)]),
                PyValue::Tuple(vec![int(SHIELD), int(1)]),
            ])
        );
        assert_eq!(at(&reg, ids[0], HI0), vec![(GUN, 1)]);
        assert!(at(&reg, ids[0], HI1).is_empty());
    }

    #[test]
    fn charges_and_drones_are_topped_up_to_the_fitting_quantity() {
        let (mut reg, ids) = world(&[(ROUND, 300, false), (DRONE, 5, false)]);
        let mut a = args(
            ids[0],
            HULL,
            Fit {
                pool: pool(&[(ROUND, &[ids[1]]), (DRONE, &[ids[2]])]),
                modules: &[],
                charges: &[(ROUND, 100)],
                drones: &[(DRONE, 3)],
                rigs_to_cargo: &[],
                fit_rigs: true,
            },
        );
        let got = call(&mut reg, ids[0], "FitFitting", &a).expect("answers");
        assert_eq!(got, PyValue::List(vec![]));
        assert_eq!(
            at(&reg, ids[0], crate::fitting::FLAG_CARGO),
            vec![(ROUND, 100)]
        );
        assert_eq!(
            at(&reg, ids[0], crate::drones::FLAG_DRONE_BAY),
            vec![(DRONE, 3)]
        );
        assert_eq!(hangar(&reg, ROUND), 200);
        // Loading again asks for the same totals: nothing more moves.
        let again = call(&mut reg, ids[0], "FitFitting", &a).expect("answers");
        assert_eq!(again, PyValue::List(vec![]));
        assert_eq!(hangar(&reg, ROUND), 200);
        // More wanted than the hangar holds: the rest is missing.
        a[4] = crate::pyutil::keyval(vec![
            ("chargesByType", dict(&[(ROUND, 400)])),
            ("dronesByType", dict(&[])),
            ("fightersByTypeID", dict(&[])),
            ("iceByType", dict(&[])),
            ("modulesByFlag", dict(&[])),
            ("implantsByTypeID", dict(&[])),
        ]);
        let short = call(&mut reg, ids[0], "FitFitting", &a).expect("answers");
        assert_eq!(
            short,
            PyValue::List(vec![PyValue::Tuple(vec![int(ROUND), int(100)])])
        );
    }

    #[test]
    fn rigs_go_to_cargo_when_fit_rigs_is_false() {
        let (mut reg, ids) = world(&[(RIG, 1, true)]);
        let a = args(
            ids[0],
            HULL,
            Fit {
                pool: pool(&[(RIG, &[ids[1]])]),
                modules: &[(RIG0, RIG)],
                charges: &[],
                drones: &[],
                rigs_to_cargo: &[(RIG, 1)],
                fit_rigs: false,
            },
        );
        let got = call(&mut reg, ids[0], "FitFitting", &a).expect("answers");
        assert_eq!(got, PyValue::List(vec![]));
        assert!(at(&reg, ids[0], RIG0).is_empty());
        assert_eq!(at(&reg, ids[0], crate::fitting::FLAG_CARGO), vec![(RIG, 1)]);
    }

    #[test]
    fn a_wrong_ship_type_is_refused_and_nothing_moves() {
        let (mut reg, ids) = world(&[(GUN, 1, true)]);
        let before = reg.store().items().to_vec();
        let a = args(
            ids[0],
            OTHER_HULL,
            Fit {
                pool: pool(&[(GUN, &[ids[1]])]),
                modules: &[(HI0, GUN)],
                charges: &[],
                drones: &[],
                rigs_to_cargo: &[],
                fit_rigs: true,
            },
        );
        assert!(call(&mut reg, ids[0], "FitFitting", &a).is_err());
        assert_eq!(reg.store().items(), &before[..]);
    }

    #[test]
    fn strip_moves_modules_and_their_charges_to_the_hangar() {
        let (mut reg, ids) = world(&[(GUN, 1, true), (SHIELD, 1, true), (ROUND, 50, false)]);
        let a = args(
            ids[0],
            HULL,
            Fit {
                pool: pool(&[(GUN, &[ids[1]]), (SHIELD, &[ids[2]])]),
                modules: &[(HI0, GUN), (MED0, SHIELD)],
                charges: &[],
                drones: &[],
                rigs_to_cargo: &[],
                fit_rigs: true,
            },
        );
        call(&mut reg, ids[0], "FitFitting", &a).expect("fits");
        // Load the rounds the way the stored rows keep them: a charge at
        // its module's flag.
        reg.apply_items(&[crate::item::Edit::Move {
            item_id: ids[3],
            to: crate::item::Slot::new(ALICE, ids[0], HI0),
        }])
        .expect("loaded");
        assert!(crate::item::is_loaded_charge(
            reg.store().get(ids[3]).expect("round")
        ));
        let got = call(&mut reg, ids[0], "StripFitting", &[]).expect("strips");
        assert_eq!(got, PyValue::None);
        assert!(at(&reg, ids[0], HI0).is_empty());
        assert!(at(&reg, ids[0], MED0).is_empty());
        assert_eq!(hangar(&reg, GUN), 1);
        assert_eq!(hangar(&reg, SHIELD), 1);
        assert_eq!(hangar(&reg, ROUND), 50);
    }

    #[test]
    fn a_third_module_of_a_max_group_two_group_is_refused_with_the_clients_key() {
        let (mut reg, ids) = world(&[(CAPPED, 1, true), (CAPPED, 1, true), (CAPPED, 1, true)]);
        for (n, flag) in [HI0, HI1].into_iter().enumerate() {
            fit_one(&mut reg, Some(ALICE), ids[0], ids[1 + n], flag).expect("under the cap");
        }
        let Err(ServiceError::Raise(e)) = fit_one(&mut reg, Some(ALICE), ids[0], ids[3], HI2)
        else {
            panic!("the third is refused");
        };
        let want = crate::fitting::Refusal::TooManyByGroup {
            type_id: CAPPED,
            ship_type_id: HULL,
            group_id: CAPPED_GROUP,
            limit: 2,
            fitted: 2,
        }
        .to_error();
        assert_eq!(format!("{:?}", ServiceError::Raise(e)), format!("{want:?}"));
        assert!(at(&reg, ids[0], HI2).is_empty());
        // Through the saved-fitting path the third line is reported missing.
        let a = args(
            ids[0],
            HULL,
            Fit {
                pool: pool(&[(CAPPED, &[ids[3]])]),
                modules: &[(HI2, CAPPED)],
                charges: &[],
                drones: &[],
                rigs_to_cargo: &[],
                fit_rigs: true,
            },
        );
        let got = call(&mut reg, ids[0], "FitFitting", &a).expect("answers");
        assert_eq!(
            got,
            PyValue::List(vec![PyValue::Tuple(vec![int(CAPPED), int(1)])])
        );
    }

    // ---- FITFIX: `ship.FitShips` and the batch's single cache drop ----

    /// `FitShips`'s keyword arguments, as the client sends them.
    fn ships_kwargs(
        ship_type: i64,
        num: i64,
        modules: &[(i64, i64)],
        charges: &[(i64, i64)],
        container: Option<i64>,
    ) -> Vec<(PyValue, PyValue)> {
        crate::pyutil::pypairs(vec![
            ("shipTypeID", int(ship_type)),
            ("numToFit", int(num)),
            ("itemLocationID", int(STATION)),
            ("fitRigs", int(1)),
            ("name", PyValue::Unicode("Fixture fit".to_owned())),
            ("itemContainerID", container.map_or(PyValue::None, int)),
            ("cargoItemsByType", dict(&[])),
            (
                "fitInfo",
                crate::pyutil::keyval(vec![
                    ("chargesByType", dict(charges)),
                    ("dronesByType", dict(&[])),
                    ("fightersByTypeID", dict(&[])),
                    ("iceByType", dict(&[])),
                    ("modulesByFlag", dict(modules)),
                    ("implantsByTypeID", dict(&[])),
                ]),
            ),
        ])
    }

    fn fit_ships_call(
        reg: &mut Registry,
        kwargs: &[(PyValue, PyValue)],
    ) -> Result<PyValue, ServiceError> {
        let bind = PyValue::Tuple(vec![int(STATION), int(15)]);
        dispatch(
            reg,
            &Request {
                service: "ship",
                method: "FitShips",
                args: &[],
                kwargs,
                bind_params: Some(&bind),
                actor: Some(ALICE),
            },
        )
    }

    /// Assembled hulls of `HULL` in the hangar, other than the pilot's own.
    fn new_hulls(reg: &Registry, own: i64) -> Vec<i64> {
        let mut v: Vec<i64> = reg
            .store()
            .items()
            .iter()
            .filter(|i| {
                i.type_id == HULL && i.singleton && i.location_id == STATION && i.item_id != own
            })
            .map(|i| i.item_id)
            .collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn fit_ships_assembles_and_fits_every_hull_and_answers_nothing_failed() {
        // Two packaged hulls, a gun and a shield each, rounds for both.
        let (mut reg, ids) = world(&[
            (HULL, 2, false),
            (GUN, 1, true),
            (GUN, 1, true),
            (SHIELD, 1, true),
            (SHIELD, 1, true),
            (ROUND, 100, false),
        ]);
        let kw = ships_kwargs(HULL, 2, &[(HI0, GUN), (MED0, SHIELD)], &[(ROUND, 40)], None);
        let got = fit_ships_call(&mut reg, &kw).expect("answers");
        assert_eq!(got, PyValue::None, "nothing failed");
        let hulls = new_hulls(&reg, ids[0]);
        assert_eq!(hulls.len(), 2, "both packaged hulls were assembled");
        for h in hulls {
            assert_eq!(at(&reg, h, HI0), vec![(GUN, 1)]);
            assert_eq!(at(&reg, h, MED0), vec![(SHIELD, 1)]);
            assert_eq!(at(&reg, h, crate::fitting::FLAG_CARGO), vec![(ROUND, 40)]);
        }
        assert_eq!(hangar(&reg, ROUND), 20);
        let c = reg.character(ALICE).expect("Alice");
        assert_eq!(c.ship_id, ids[0], "the pilot's own ship is untouched");
    }

    #[test]
    fn fit_ships_with_too_little_stock_lists_what_is_missing_and_the_hull() {
        // One gun for two hulls: the second hull is left without one.
        let (mut reg, ids) = world(&[(HULL, 2, false), (GUN, 1, true)]);
        let kw = ships_kwargs(HULL, 2, &[(HI0, GUN)], &[], None);
        let got = fit_ships_call(&mut reg, &kw).expect("answers");
        let hulls = new_hulls(&reg, ids[0]);
        assert_eq!(hulls.len(), 2);
        let PyValue::Tuple(parts) = got else {
            panic!("a failure is a (failedToLoad, failedShipID) pair, got {got:?}");
        };
        assert_eq!(
            parts[0],
            PyValue::List(vec![PyValue::Tuple(vec![int(GUN), int(1)])])
        );
        // Which hull is named is an [A]; only the client's shape is pinned.
        assert_eq!(parts.len(), 2);
        assert!(matches!(parts[1], PyValue::Int(_)));
    }

    #[test]
    fn fit_ships_takes_its_modules_from_the_chosen_container() {
        // The gun is in a container in the hangar, not in the hangar itself.
        let (mut reg, ids) = world(&[(HULL, 1, false), (BOX, 1, true), (GUN, 1, true)]);
        let container = ids[2];
        reg.apply_items(&[crate::item::Edit::Move {
            item_id: ids[3],
            to: crate::item::Slot::new(ALICE, container, 0),
        }])
        .expect("the gun goes into the container");
        let kw = ships_kwargs(HULL, 1, &[(HI0, GUN)], &[], Some(container));
        let got = fit_ships_call(&mut reg, &kw).expect("answers");
        assert_eq!(got, PyValue::None);
        let hulls = new_hulls(&reg, ids[0]);
        assert_eq!(hulls.len(), 1);
        assert_eq!(at(&reg, hulls[0], HI0), vec![(GUN, 1)]);
    }

    #[test]
    fn fit_ships_refuses_a_container_that_is_not_a_personal_container() {
        // A singleton round is not one of the client's container groups.
        let (mut reg, ids) = world(&[(HULL, 1, false), (ROUND, 1, true), (GUN, 1, true)]);
        reg.apply_items(&[crate::item::Edit::Move {
            item_id: ids[3],
            to: crate::item::Slot::new(ALICE, ids[2], 0),
        }])
        .expect("the gun goes in");
        let kw = ships_kwargs(HULL, 1, &[(HI0, GUN)], &[], Some(ids[2]));
        assert!(fit_ships_call(&mut reg, &kw).is_err());
        assert!(new_hulls(&reg, ids[0]).is_empty(), "nothing was assembled");
    }

    #[test]
    fn a_hold_that_refuses_leaves_the_containers_stacks_in_the_container() {
        let (mut reg, ids) = world(&[
            (HULL, 1, false),
            (BOX, 1, true),
            (ROUND, 100_000, false),
            (ROUND, 100_000, false),
        ]);
        for &r in &ids[3..] {
            reg.apply_items(&[crate::item::Edit::Move {
                item_id: r,
                to: crate::item::Slot::new(ALICE, ids[2], 0),
            }])
            .expect("the rounds go into the box");
        }
        // 1000 m3 a stack; the fixture hull's hold is smaller.
        let kw = ships_kwargs(HULL, 1, &[], &[(ROUND, 100_000)], Some(ids[2]));
        let got = fit_ships_call(&mut reg, &kw).expect("answers");
        assert!(matches!(got, PyValue::Tuple(_)), "the rounds are missing");
        let in_box: Vec<(i64, i64)> = reg
            .store()
            .contents_of(ids[2])
            .map(|i| (i.type_id, i.quantity))
            .collect();
        assert_eq!(in_box, vec![(ROUND, 100_000), (ROUND, 100_000)]);
        assert_eq!(hangar(&reg, ROUND), 0, "nothing was left in the hangar");
    }

    #[test]
    fn fit_rigs_false_keeps_the_rigs_out_of_the_slots_and_in_the_cargo() {
        let (mut reg, ids) = world(&[(HULL, 1, false), (RIG, 1, true)]);
        let mut kw = ships_kwargs(HULL, 1, &[(RIG0, RIG)], &[], None);
        kw.retain(|(_, k)| *k != crate::pyutil::bstr("fitRigs"));
        kw.retain(|(_, k)| *k != crate::pyutil::bstr("cargoItemsByType"));
        kw.push((int(0), crate::pyutil::bstr("fitRigs")));
        kw.push((dict(&[(RIG, 1)]), crate::pyutil::bstr("cargoItemsByType")));
        let got = fit_ships_call(&mut reg, &kw).expect("answers");
        assert_eq!(got, PyValue::None);
        let hulls = new_hulls(&reg, ids[0]);
        assert_eq!(hulls.len(), 1);
        assert!(at(&reg, hulls[0], RIG0).is_empty());
        assert_eq!(
            at(&reg, hulls[0], crate::fitting::FLAG_CARGO),
            vec![(RIG, 1)]
        );
    }

    #[test]
    fn asking_for_more_ships_than_hulls_fits_what_exists_without_an_error() {
        let (mut reg, ids) = world(&[(HULL, 2, false), (GUN, 1, true), (GUN, 1, true)]);
        let kw = ships_kwargs(HULL, 5, &[(HI0, GUN)], &[], None);
        let got = fit_ships_call(&mut reg, &kw).expect("answers");
        assert_eq!(
            got,
            PyValue::None,
            "nothing is missing for the hulls there are"
        );
        let hulls = new_hulls(&reg, ids[0]);
        assert_eq!(hulls.len(), 2);
        for h in hulls {
            assert_eq!(at(&reg, h, HI0), vec![(GUN, 1)]);
        }
    }

    #[test]
    fn fit_ships_decode_refuses_a_call_without_its_ship_type() {
        let req = Request {
            service: "ship",
            method: "FitShips",
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        assert_eq!(super::super::fit_ships::decode(&req), None);
    }

    /// The batch's cache drop is one push naming every location, sent after
    /// the last item row; a per-module drop made the client re-bind the ship
    /// inventory once per module in the middle of the item changes.
    #[test]
    fn a_multi_add_drops_the_cache_once_after_every_item_row() {
        let (mut reg, ids) = world(&[(GUN, 1, true), (GUN, 1, true), (SHIELD, 1, true)]);
        let _ = reg.take_pending();
        let a = vec![
            PyValue::List(vec![int(ids[1]), int(ids[2]), int(ids[3])]),
            int(STATION),
        ];
        let bind = PyValue::Tuple(vec![int(ids[0]), int(STATION)]);
        let kw = crate::pyutil::pypairs(vec![("flag", int(crate::fitting::FLAG_AUTOFIT))]);
        dispatch(
            &mut reg,
            &Request {
                service: "invbroker::inventory",
                method: "MultiAdd",
                args: &a,
                kwargs: &kw,
                bind_params: Some(&bind),
                actor: Some(ALICE),
            },
        )
        .expect("answers");
        let names: Vec<&'static str> = reg
            .take_pending()
            .iter()
            .map(|(_, p)| p.to_push().0)
            .collect();
        let drops: Vec<usize> = names
            .iter()
            .enumerate()
            .filter(|(_, n)| **n == "OnInvalidateCacheForLocations")
            .map(|(i, _)| i)
            .collect();
        let last_row = names
            .iter()
            .rposition(|n| *n == "OnItemChange")
            .expect("rows were pushed");
        assert_eq!(drops.len(), 1, "one drop for the batch: {names:?}");
        assert!(drops[0] > last_row, "after the last item row: {names:?}");
        assert!(
            names.iter().filter(|n| **n == "OnItemChange").count() >= 3,
            "every module still gets its row"
        );
    }
}
