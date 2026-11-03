//! Inventory quick pack (lane gap-inventory, GAP-INVPACK, from EVEJS-GAP
//! svcB §3.7): `MultiMerge` and `GetDamageForCrystals`.
//!
//! **`MultiMerge(ops, sourceContainerID)`** is called on the *destination*
//! container when a stack is dropped on a stack of the same type
//! (`invCache.py:1063-1088`, `item.py:1016-1040`, `invControllers.py:317-325`)
//! [V]. `ops` is `[(sourceID, destID, qty)]`; loaded charges are unloaded
//! client-side first. On a one-op merge the client catches
//! `NotEnoughCargoSpace` and its siblings and offers the move again at a
//! quantity that fits (`invControllers.py:327-365`), so a refusal for room
//! is the plan's own `NotEnoughCargoSpace`, not a note. The answer is
//! `None`.
//!
//! Each op is checked as the plain move of `qty` units from the source to
//! the destination stack's address would be -- the same planners `Add` uses
//! ([`crate::fitting::plan_to_hold`], [`crate::fitting::plan_unfit`], the
//! drone and fighter bays') -- and then applied as `Consume` (or `Destroy`
//! for the whole stack) plus `Grow` in one plan, so the units are conserved
//! and the destination keeps its id. Ops are applied in order; the first
//! refusal is the answer, and earlier ops stay done (**[A]**, like
//! `MultiAdd`).
//!
//! **`GetDamageForCrystals(ids)`** primes the crystal tooltip
//! (`inventorysvc.py:372-379`): `{itemID: damage}`. **[A]** `0.0` for every
//! id the pilot owns: there is no crystal wear yet (SP#33). Falsifier: a
//! worn crystal showing as new.

use super::*;

/// `invbroker.GetDamageForCrystals(itemIDs)`.
pub(super) fn crystal_damage(reg: &Registry, req: &Request<'_>) -> PyValue {
    let mut ids = std::collections::BTreeSet::new();
    if let Some(v) = req.args.first() {
        super::fit_apply::int_set(v, &mut ids);
    }
    PyValue::Dict(
        ids.into_iter()
            .filter(|id| reg.item(*id).is_some_and(|i| Some(i.owner_id) == req.actor))
            // `(value, key)`.
            .map(|id| (PyValue::Real(0.0), PyValue::Int(id)))
            .collect(),
    )
}

/// The three of one op, `(sourceID, destID, qty)`.
fn ops_of(req: &Request<'_>) -> Option<Vec<(i64, i64, i64)>> {
    let (PyValue::List(ops) | PyValue::Tuple(ops)) = req.args.first()? else {
        return None;
    };
    ops.iter()
        .map(|op| match op {
            PyValue::Tuple(p) | PyValue::List(p) => match p.as_slice() {
                [PyValue::Int(s), PyValue::Int(d), PyValue::Int(q)] => Some((*s, *d, *q)),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// `invCacheContainer.MultiMerge`, on the bound container.
pub(super) fn multi_merge(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(ops) = ops_of(req) else {
        return Err(module_bad_args(
            req,
            "([(sourceID, destID, qty)], sourceID)",
        ));
    };
    let owner = req
        .actor
        .ok_or_else(|| custom_notify("No character is selected."))?;
    let bound = container_of(req.bind_params);
    for (source_id, dest_id, qty) in ops {
        merge_one(reg, owner, bound, source_id, dest_id, qty)?;
    }
    Ok(PyValue::None)
}

fn merge_one(
    reg: &mut Registry,
    owner: i64,
    bound: Option<i64>,
    source_id: i64,
    dest_id: i64,
    qty: i64,
) -> Result<(), ServiceError> {
    let not_here = || custom_notify("Those stacks cannot be merged here.");
    let (Some(source), Some(dest)) = (reg.item(source_id).cloned(), reg.item(dest_id).cloned())
    else {
        return Err(not_here());
    };
    if source_id == dest_id
        || source.owner_id != owner
        || dest.owner_id != owner
        || source.singleton
        || dest.singleton
        || source.type_id != dest.type_id
        || crate::item::is_loaded_charge(&source)
        || qty < 1
        || qty > source.quantity
    {
        return Err(not_here());
    }
    let c = reg.character(owner).cloned();
    let station = reg.docked_station(owner);
    // The destination must be in the bound container: the hangar, or the
    // ship the inventory is bound on.
    let in_hangar = bound == Some(CONTAINER_HANGAR)
        && Some(dest.location_id) == station
        && dest.flag_id == crate::model::FLAG_HANGAR;
    let in_ship = bound == Some(dest.location_id) && bound != Some(CONTAINER_HANGAR);
    // #57: a stack in an assembled container, which the bound inventory is.
    let in_container = in_ship
        && reg
            .item(dest.location_id)
            .is_some_and(crate::containers::is_container);
    let from_container = reg
        .item(source.location_id)
        .is_some_and(crate::containers::is_container);
    if !in_hangar && !in_ship {
        return Err(not_here());
    }
    // The move of `qty` units to the destination's address must be one `Add`
    // would accept; its edits are not used, only its verdict.
    if source.slot() != dest.slot() {
        let _plan = if in_container || (in_hangar && from_container) {
            let (Some(c), Some(station)) = (c.as_ref(), station) else {
                return Err(not_here());
            };
            let to = if in_container {
                crate::containers::Dest::Container(dest.location_id)
            } else {
                crate::containers::Dest::Hangar
            };
            crate::containers::plan(reg, c, station, to, source_id, Some(qty))?
        } else if in_hangar {
            crate::fitting::plan_unfit(reg, c.as_ref(), source_id, Some(qty))
                .map_err(|r| r.to_error())?
        } else if dest.flag_id == crate::drones::FLAG_DRONE_BAY {
            crate::drones::plan_to_bay(reg, Some(owner), dest.location_id, source_id, Some(qty))?
        } else if dest.flag_id == crate::fighters::FLAG_FIGHTER_BAY {
            crate::fighters::plan_to_bay(reg, Some(owner), dest.location_id, source_id, Some(qty))
                .map_err(|r| r.to_error())?
        } else if crate::fitting::is_plain_hold(dest.flag_id) {
            crate::fitting::plan_to_hold(
                reg,
                c.as_ref(),
                dest.location_id,
                source_id,
                Some(qty),
                dest.flag_id,
            )
            .map_err(|r| r.to_error())?
        } else {
            return Err(not_here());
        };
    }
    let take = if qty == source.quantity {
        crate::item::Edit::Destroy { item_id: source_id }
    } else {
        crate::item::Edit::Consume {
            item_id: source_id,
            quantity: qty,
        }
    };
    reg.apply_items(&[
        take,
        crate::item::Edit::Grow {
            item_id: dest_id,
            quantity: qty,
        },
    ])
    .map(|_| ())
    .map_err(plan_refused)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Content;
    use crate::item::Item;
    use crate::model::{Character, SandboxConfig};

    const ALICE: i64 = 990_950_100;
    const STATION: i64 = 990_950_101;
    const HULL: i64 = 990_950_001;
    const ORE: i64 = 990_950_002;
    const GAS: i64 = 990_950_003;
    const JUNK: i64 = 990_950_004;
    /// The gas group of `crate::item::HOLDS` (flag 135).
    const GAS_GROUP: i64 = 711;
    const GAS_HOLD: i64 = 135;

    fn content() -> Content {
        let rows = format!(
            "type\t{HULL}\t25\t6\t1\t1\t1000.0\t500.0\t1200000000.0\t300.0\t0\t\t\
             9=1000000.0;11=45000.0;14=3;13=2;12=3;37=100.0;48=125000.0;552=30.0;\
             600=2.0;1281=3.0;1556=50.0;1557=10.0\tFixture Hull\n\
             type\t{ORE}\t1884\t25\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t\tFixture Ore\n\
             type\t{GAS}\t{GAS_GROUP}\t2\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tFixture Gas\n\
             type\t{JUNK}\t99\t4\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tFixture Junk\n"
        );
        let text = format!("{}{rows}", crate::typedata::FIXTURE);
        let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
        let mut c = Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    /// Alice docked in `HULL`, with `(type, qty, flag)` stacks: in the
    /// hangar for flag 4, else inside the ship. Ids from 990_951_001; the
    /// hull is 990_951_000.
    fn world(stacks: &[(i64, i64, i64)]) -> (Registry, Vec<i64>) {
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
        let mut ids = vec![990_951_000];
        let hull = crate::item::find_in(reg.content(), HULL).expect("hull");
        items.push(Item {
            item_id: ids[0],
            type_id: HULL,
            owner_id: ALICE,
            location_id: STATION,
            flag_id: crate::model::FLAG_HANGAR,
            quantity: 1,
            group_id: hull.group_id,
            category_id: hull.category_id,
            singleton: true,
        });
        for (n, &(type_id, quantity, flag)) in stacks.iter().enumerate() {
            let id = 990_951_001 + n as i64;
            let t = crate::item::find_in(reg.content(), type_id).expect("a fixture type");
            items.push(Item {
                item_id: id,
                type_id,
                owner_id: ALICE,
                location_id: if flag == crate::model::FLAG_HANGAR {
                    STATION
                } else {
                    ids[0]
                },
                flag_id: flag,
                quantity,
                group_id: t.group_id,
                category_id: t.category_id,
                singleton: false,
            });
            ids.push(id);
        }
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 990_952_000,
            ..crate::store::Snapshot::default()
        });
        let c = reg.character_mut(ALICE).expect("Alice");
        c.ship_id = ids[0];
        c.ship_type_id = HULL;
        (reg, ids)
    }

    fn kw(name: &str, v: PyValue) -> (PyValue, PyValue) {
        (v, PyValue::Bytes(name.as_bytes().to_vec()))
    }

    fn call(
        reg: &mut Registry,
        bind: PyValue,
        method: &str,
        args: &[PyValue],
        kwargs: &[(PyValue, PyValue)],
    ) -> Result<PyValue, ServiceError> {
        dispatch(
            reg,
            &Request {
                service: "invbroker::inventory",
                method,
                args,
                kwargs,
                bind_params: Some(&bind),
                actor: Some(ALICE),
            },
        )
    }

    fn ship_bind(ids: &[i64]) -> PyValue {
        PyValue::Tuple(vec![PyValue::Int(ids[0]), PyValue::Int(STATION)])
    }

    fn merge(
        reg: &mut Registry,
        bind: PyValue,
        ops: &[(i64, i64, i64)],
    ) -> Result<PyValue, ServiceError> {
        let ops = PyValue::List(
            ops.iter()
                .map(|&(s, d, q)| {
                    PyValue::Tuple(vec![PyValue::Int(s), PyValue::Int(d), PyValue::Int(q)])
                })
                .collect(),
        );
        call(reg, bind, "MultiMerge", &[ops, PyValue::Int(STATION)], &[])
    }

    fn qty(reg: &Registry, id: i64) -> Option<i64> {
        reg.item(id).map(|i| i.quantity)
    }

    fn total(reg: &Registry, type_id: i64) -> i64 {
        reg.store()
            .items()
            .iter()
            .filter(|i| i.type_id == type_id)
            .map(|i| i.quantity)
            .sum()
    }

    /// The message key of a raised `UserError`.
    fn key_of(e: &ServiceError) -> String {
        if let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e
            && let PyValue::Tuple(parts) = header.as_ref()
            && let Some(PyValue::Tuple(args)) = parts.get(1)
            && let Some(PyValue::Bytes(b)) = args.first()
        {
            return String::from_utf8_lossy(b).into_owned();
        }
        format!("{e}")
    }

    const CARGO: i64 = crate::fitting::FLAG_CARGO;
    const HANGAR: i64 = crate::model::FLAG_HANGAR;

    #[test]
    fn merge_moves_units_between_two_stacks_and_conserves_them() {
        // 100 ore in the hangar, 40 in the hold: drop 30 of the hangar's on
        // the hold's stack.
        let (mut reg, ids) = world(&[(ORE, 100, HANGAR), (ORE, 40, CARGO)]);
        merge(&mut reg, ship_bind(&ids), &[(ids[1], ids[2], 30)]).expect("merges");
        assert_eq!(qty(&reg, ids[1]), Some(70));
        assert_eq!(qty(&reg, ids[2]), Some(70));
        assert_eq!(total(&reg, ORE), 140);
        // Within one hold, and back out to the hangar's stack.
        merge(
            &mut reg,
            PyValue::Tuple(vec![PyValue::Int(CONTAINER_HANGAR)]),
            &[(ids[2], ids[1], 20)],
        )
        .expect("merges out");
        assert_eq!(qty(&reg, ids[1]), Some(90));
        assert_eq!(qty(&reg, ids[2]), Some(50));
        assert_eq!(total(&reg, ORE), 140);
    }

    #[test]
    fn a_whole_merge_destroys_the_source() {
        let (mut reg, ids) = world(&[(ORE, 25, HANGAR), (ORE, 40, CARGO)]);
        merge(&mut reg, ship_bind(&ids), &[(ids[1], ids[2], 25)]).expect("merges");
        assert_eq!(qty(&reg, ids[1]), None, "the source stack is gone");
        assert_eq!(qty(&reg, ids[2]), Some(65));
        assert_eq!(total(&reg, ORE), 65);
    }

    #[test]
    fn merging_into_a_full_hold_is_refused_with_not_enough_cargo_space() {
        // The fixture hull's hold is 500 m3 and ore is 0.1 m3 a unit, so
        // 5000 units fill it; 4990 are in, room for ten more.
        let (mut reg, ids) = world(&[(ORE, 6000, HANGAR), (ORE, 4990, CARGO)]);
        let before = reg.store().items().to_vec();
        let Err(e) = merge(&mut reg, ship_bind(&ids), &[(ids[1], ids[2], 11)]) else {
            panic!("refused");
        };
        assert_eq!(key_of(&e), "NotEnoughCargoSpace");
        assert_eq!(reg.store().items(), &before[..], "a refusal moves nothing");
        // What fits goes: the client's re-offer at the quantity that fits.
        merge(&mut reg, ship_bind(&ids), &[(ids[1], ids[2], 10)]).expect("fits");
        assert_eq!(qty(&reg, ids[2]), Some(5000));
    }

    #[test]
    fn merge_refuses_other_types_singletons_and_bad_quantities() {
        let (mut reg, ids) = world(&[(ORE, 10, HANGAR), (JUNK, 10, CARGO), (ORE, 10, CARGO)]);
        let before = reg.store().items().to_vec();
        for (s, d, q) in [
            (ids[1], ids[2], 5),  // different types
            (ids[1], ids[3], 0),  // nothing
            (ids[1], ids[3], 11), // more than there is
            (ids[1], ids[1], 5),  // itself
        ] {
            assert!(merge(&mut reg, ship_bind(&ids), &[(s, d, q)]).is_err());
        }
        // A destination that is not in the bound container.
        assert!(
            merge(
                &mut reg,
                PyValue::Tuple(vec![PyValue::Int(CONTAINER_HANGAR)]),
                &[(ids[3], ids[1], 5)],
            )
            .is_ok(),
            "the hangar's own stack is in the hangar"
        );
        assert!(merge(&mut reg, ship_bind(&ids), &[(ids[3], ids[1], 5)]).is_err());
        assert_ne!(reg.store().items(), &before[..]);
    }

    #[test]
    fn crystal_damage_is_a_dict_keyed_by_item() {
        let (reg, ids) = world(&[(ORE, 10, HANGAR)]);
        let set = PyValue::ObjectEx {
            form: eve_marshal::ObjectForm::Call,
            header: Box::new(PyValue::Tuple(vec![
                PyValue::Global(b"__builtin__.set".to_vec()),
                PyValue::Tuple(vec![PyValue::List(vec![
                    PyValue::Int(ids[1]),
                    PyValue::Int(777),
                ])]),
            ])),
            list: vec![],
            dict: vec![],
        };
        let args = [set];
        let got = crystal_damage(
            &reg,
            &Request {
                service: "invbroker",
                method: "GetDamageForCrystals",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        );
        assert_eq!(
            got,
            PyValue::Dict(vec![(PyValue::Real(0.0), PyValue::Int(ids[1]))]),
            "mine, unworn; the unknown id is left out"
        );
    }

    // ---- GAP-HOLDS ----------------------------------------------------

    fn add_to_hold(
        reg: &mut Registry,
        ids: &[i64],
        item: i64,
        qty: i64,
        flag: i64,
    ) -> Result<PyValue, ServiceError> {
        call(
            reg,
            ship_bind(ids),
            "Add",
            &[PyValue::Int(item), PyValue::Int(STATION)],
            &[kw("qty", PyValue::Int(qty)), kw("flag", PyValue::Int(flag))],
        )
    }

    #[test]
    fn gas_goes_into_a_gas_hold_and_counts_against_its_capacity() {
        // The hull's gas hold is 10 m3 (attribute 1557); gas is 1 m3 a unit.
        let (mut reg, ids) = world(&[(GAS, 30, HANGAR)]);
        add_to_hold(&mut reg, &ids, ids[1], 10, GAS_HOLD).expect("fills the hold");
        let held: i64 = reg
            .store()
            .contents_of(ids[0])
            .filter(|i| i.flag_id == GAS_HOLD)
            .map(|i| i.quantity)
            .sum();
        assert_eq!(held, 10);
        let Err(e) = add_to_hold(&mut reg, &ids, ids[1], 1, GAS_HOLD) else {
            panic!("full");
        };
        assert_eq!(key_of(&e), "NotEnoughCargoSpace");
    }

    #[test]
    fn a_type_the_hold_does_not_take_is_refused_with_wrong_hold() {
        let (mut reg, ids) = world(&[(JUNK, 3, HANGAR)]);
        let Err(e) = add_to_hold(&mut reg, &ids, ids[1], 3, GAS_HOLD) else {
            panic!("refused");
        };
        let want = crate::fitting::Refusal::WrongHold { type_id: JUNK }.to_error();
        assert_eq!(format!("{e:?}"), format!("{want:?}"));
    }

    #[test]
    fn ore_dragged_from_cargo_into_the_mining_hold_moves() {
        // The hull's general mining hold (attribute 1556) is 50 m3; ore is
        // 0.1 m3 a unit.
        let (mut reg, ids) = world(&[(ORE, 100, CARGO)]);
        let flag = crate::mining_hold::FLAG_GENERAL_MINING_HOLD;
        add_to_hold(&mut reg, &ids, ids[1], 100, flag).expect("cargo to the mining hold");
        let held: i64 = reg
            .store()
            .contents_of(ids[0])
            .filter(|i| i.flag_id == flag)
            .map(|i| i.quantity)
            .sum();
        assert_eq!(held, 100);
        assert_eq!(total(&reg, ORE), 100);
    }

    #[test]
    fn a_hull_without_the_hold_has_no_room() {
        // The fixture hull has no mineral hold (attribute 1558).
        let (mut reg, ids) = world(&[(ORE, 5, HANGAR)]);
        let Err(e) = add_to_hold(&mut reg, &ids, ids[1], 5, 136) else {
            panic!("refused");
        };
        // Ore is no mineral, so the type check answers first.
        assert_eq!(key_of(&e), "CannotPlaceTypeInFuelBay");
        assert!(
            reg.store().contents_of(ids[0]).all(|i| i.flag_id != 136),
            "nothing moved"
        );
    }
}
