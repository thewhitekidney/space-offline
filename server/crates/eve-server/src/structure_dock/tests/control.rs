//! U6C (issue #115): taking and releasing the controls of a structure.
//! Every id is invented.

use super::*;
use crate::structure_dock::control as ctl;

fn take(reg: &mut Registry, who: i64, id: i64) -> Result<PyValue, ServiceError> {
    call(
        reg,
        who,
        "structureControl",
        "TakeControl",
        &[PyValue::Int(id)],
    )
}

fn release(reg: &mut Registry, who: i64) -> Result<PyValue, ServiceError> {
    call(reg, who, "structureControl", "ReleaseControl", &[])
}

fn pilot(reg: &mut Registry, who: i64, id: i64) -> PyValue {
    call(
        reg,
        who,
        "structureControl",
        "GetStructurePilot",
        &[PyValue::Int(id)],
    )
    .expect("answered")
}

/// The `shipid` pair of every change queued for `who`, in order.
fn shipid_changes(reg: &mut Registry, who: i64) -> Vec<PyValue> {
    reg.take_session_changes()
        .into_iter()
        .filter(|(w, _)| *w == who)
        .filter_map(|(_, ch)| key(&ch, "shipid").cloned())
        .collect()
}

fn pair_of(old: i64, new: i64) -> PyValue {
    PyValue::Tuple(vec![PyValue::Int(old), PyValue::Int(new)])
}

/// A stored thing in the structure at `flag`, owned by the corporation.
fn stock(reg: &mut Registry, id: i64, flag: i64) {
    reg.apply_items(&[Edit::Create {
        type_id: crate::service::TYPE_CAPSULE,
        quantity: 1,
        to: Slot::new(CORP, id, flag),
    }])
    .expect("created");
}

fn list_flag(reg: &mut Registry, who: i64, container: i64, flag: i64) -> usize {
    let bound = PyValue::Tuple(vec![PyValue::Int(container), PyValue::None]);
    let kwargs = crate::pyutil::pypairs(vec![("flag", PyValue::Int(flag))]);
    let req = Request {
        service: "invbroker::inventory",
        method: "List",
        args: &[],
        kwargs: &kwargs,
        bind_params: Some(&bound),
        actor: Some(who),
    };
    let PyValue::List(rows) = reg.dispatch(&req).expect("lists") else {
        panic!("a list")
    };
    rows.len()
}

#[test]
fn taking_control_moves_only_the_session_ship_to_the_structure() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    let _ = reg.take_session_changes();
    let hull = reg.character(BOSS).unwrap().ship_id;
    take(&mut reg, BOSS, id).unwrap();
    assert_eq!(shipid_changes(&mut reg, BOSS), vec![pair_of(hull, id)]);
    // The character's hull and dock are what they were.
    assert_eq!(reg.character(BOSS).unwrap().ship_id, hull);
    assert_eq!(docked_in(&reg, BOSS), Some(id));
    assert_eq!(pilot(&mut reg, PAL, id), PyValue::Int(BOSS));
    // Taking it again is nothing.
    take(&mut reg, BOSS, id).unwrap();
    assert!(shipid_changes(&mut reg, BOSS).is_empty());
}

#[test]
fn nobody_is_at_the_controls_of_an_empty_structure() {
    let (mut reg, id) = world();
    assert_eq!(pilot(&mut reg, BOSS, id), PyValue::None);
    dock_call(&mut reg, BOSS, id).unwrap();
    assert_eq!(pilot(&mut reg, BOSS, id), PyValue::None);
}

#[test]
fn taking_control_needs_the_dock_and_the_setting() {
    let (mut reg, id) = world();
    // Not docked: nothing.
    assert!(take(&mut reg, BOSS, id).is_err());
    assert!(reg.take_session_changes().is_empty());
    // Not a structure.
    dock_call(&mut reg, BOSS, id).unwrap();
    assert!(take(&mut reg, BOSS, id + 1).is_err());
    // A pilot whose corporation does not own it, with no group of setting 17.
    let mut s = structure::get(reg.deltas(), id).unwrap();
    s.owner_corp = FOE_CORP;
    structure::put(reg.deltas_mut(), &s);
    let _ = reg.take_session_changes();
    assert!(take(&mut reg, BOSS, id).is_err());
    assert!(reg.take_session_changes().is_empty());
    assert_eq!(pilot(&mut reg, BOSS, id), PyValue::None);
}

#[test]
fn releasing_gives_the_hull_back_and_a_pilot_without_control_is_left_alone() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    let hull = reg.character(BOSS).unwrap().ship_id;
    release(&mut reg, BOSS).unwrap();
    assert!(shipid_changes(&mut reg, BOSS).is_empty());
    take(&mut reg, BOSS, id).unwrap();
    let _ = reg.take_session_changes();
    release(&mut reg, BOSS).unwrap();
    assert_eq!(shipid_changes(&mut reg, BOSS), vec![pair_of(id, hull)]);
    assert_eq!(pilot(&mut reg, BOSS, id), PyValue::None);
    // Eject names the hull he has, or refuses.
    take(&mut reg, BOSS, id).unwrap();
    let _ = reg.take_session_changes();
    assert!(
        call(
            &mut reg,
            BOSS,
            "structureControl",
            "EjectFromStructure",
            &[PyValue::Int(424_242)],
        )
        .is_err()
    );
    assert!(shipid_changes(&mut reg, BOSS).is_empty());
    assert_eq!(pilot(&mut reg, BOSS, id), PyValue::Int(BOSS));
    call(
        &mut reg,
        BOSS,
        "structureControl",
        "EjectFromStructure",
        &[PyValue::Int(hull)],
    )
    .unwrap();
    assert_eq!(shipid_changes(&mut reg, BOSS), vec![pair_of(id, hull)]);
}

#[test]
fn a_second_pilot_takes_the_controls_and_the_first_gets_his_hull_back() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    dock_call(&mut reg, PAL, id).unwrap();
    take(&mut reg, BOSS, id).unwrap();
    let _ = reg.take_session_changes();
    take(&mut reg, PAL, id).unwrap();
    let boss_hull = reg.character(BOSS).unwrap().ship_id;
    let pal_hull = reg.character(PAL).unwrap().ship_id;
    let changes = reg.take_session_changes();
    let of = |who: i64| -> Vec<PyValue> {
        changes
            .iter()
            .filter(|(w, _)| *w == who)
            .filter_map(|(_, ch)| key(ch, "shipid").cloned())
            .collect()
    };
    assert_eq!(of(BOSS), vec![pair_of(id, boss_hull)]);
    assert_eq!(of(PAL), vec![pair_of(pal_hull, id)]);
    assert_eq!(pilot(&mut reg, BOSS, id), PyValue::Int(PAL));
    assert_eq!(ctl::controlling(&reg, BOSS), None);
    assert_eq!(ctl::controlling(&reg, PAL), Some(id));
}

#[test]
fn a_pilot_at_the_controls_cannot_undock_or_board_until_he_lets_go() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    take(&mut reg, BOSS, id).unwrap();
    let undock = |reg: &mut Registry| {
        call(
            reg,
            BOSS,
            "structureDocking",
            "Undock",
            &[PyValue::Int(id), PyValue::None],
        )
    };
    assert!(undock(&mut reg).is_err());
    assert_eq!(docked_in(&reg, BOSS), Some(id));
    let hull = reg.character(BOSS).unwrap().ship_id;
    assert!(
        call(
            &mut reg,
            BOSS,
            "ship",
            "Board",
            &[PyValue::Int(hull), PyValue::Int(hull)],
        )
        .is_err()
    );
    assert_eq!(ctl::controlling(&reg, BOSS), Some(id));
    release(&mut reg, BOSS).unwrap();
    undock(&mut reg).expect("undocks once he has let go");
}

#[test]
fn control_ends_when_the_structure_is_unanchored_with_the_ship_first() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    take(&mut reg, BOSS, id).unwrap();
    let hull = reg.character(BOSS).unwrap().ship_id;
    let _ = reg.take_session_changes();
    let s = structure::get(reg.deltas(), id).unwrap();
    evacuate(&mut reg, &s);
    let changes = reg.take_session_changes();
    let mine: Vec<&Vec<(PyValue, PyValue)>> = changes
        .iter()
        .filter(|(w, _)| *w == BOSS)
        .map(|(_, ch)| ch)
        .collect();
    assert_eq!(mine.len(), 1, "{mine:?}");
    assert_eq!(key(mine[0], "shipid"), Some(&pair_of(id, hull)));
    assert!(key(mine[0], "structureid").is_some());
    assert_eq!(ctl::controlling(&reg, BOSS), None);
}

#[test]
fn a_gm_move_out_of_the_structure_ends_the_control() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    take(&mut reg, BOSS, id).unwrap();
    let _ = reg.take_session_changes();
    left_by_teleport(&mut reg, BOSS, id);
    assert_eq!(ctl::controlling(&reg, BOSS), None);
    assert_eq!(shipid_changes(&mut reg, BOSS).len(), 1);
}

#[test]
fn a_logoff_at_the_controls_vacates_the_seat_and_the_relog_can_undock() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    take(&mut reg, BOSS, id).unwrap();
    assert_eq!(ctl::controlling(&reg, BOSS), Some(id));
    reg.session_closed(1);
    assert_eq!(pilot(&mut reg, PAL, id), PyValue::None);
    // The relog: a pilot in his hull, docked, who may undock.
    reg.session_opened(crate::session_table::SessionEntry {
        client_id: 1,
        session_id: 1,
        user_id: 1,
        character_id: Some(BOSS),
    });
    assert_eq!(ctl::controlling(&reg, BOSS), None);
    let info = all_info_of(&mut reg, BOSS);
    let PyValue::Dict(ships) = key(&info, "shipInfo").expect("present") else {
        panic!("shipInfo is a dict")
    };
    assert!(ships.iter().all(|(_, k)| *k != PyValue::Int(id)));
    call(
        &mut reg,
        BOSS,
        "structureDocking",
        "Undock",
        &[PyValue::Int(id), PyValue::None],
    )
    .expect("undocks");
}

#[test]
fn taking_the_controls_drops_a_seat_left_elsewhere_and_tells_the_old_pilot() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    // A row left behind under another structure id.
    reg.ext_mut::<Docked>().controls.insert(id + 5, BOSS);
    take(&mut reg, BOSS, id).unwrap();
    assert_eq!(reg.ext::<Docked>().controls.len(), 1);
    assert_eq!(ctl::controlling(&reg, BOSS), Some(id));
}

#[test]
fn bays_are_the_owning_corporations_and_need_the_setting_still() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    stock(&mut reg, id, ctl::FLAG_FUEL);
    reg.apply_items(&[Edit::Create {
        type_id: crate::service::TYPE_CAPSULE,
        quantity: 1,
        to: Slot::new(BOSS, id, ctl::FLAG_FUEL),
    }])
    .expect("created");
    take(&mut reg, BOSS, id).unwrap();
    assert_eq!(list_flag(&mut reg, BOSS, id, ctl::FLAG_FUEL), 1);
    // The corporation changes hands: the seat goes with the setting.
    let mut s = structure::get(reg.deltas(), id).unwrap();
    s.owner_corp = FOE_CORP;
    structure::put(reg.deltas_mut(), &s);
    let _ = reg.take_session_changes();
    let _ = list_flag(&mut reg, BOSS, id, ctl::FLAG_FUEL);
    assert_eq!(ctl::controlling(&reg, BOSS), None);
    assert_eq!(shipid_changes(&mut reg, BOSS).len(), 1);
}

#[test]
fn get_all_info_at_the_controls_primes_the_structure_as_the_ship() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    take(&mut reg, BOSS, id).unwrap();
    let info = all_info_of(&mut reg, BOSS);
    for field in ["structureInfo", "shipInfo"] {
        let PyValue::Dict(primed) = key(&info, field).expect("present") else {
            panic!("{field} is a dict")
        };
        assert_eq!(primed.len(), 1, "{field}");
        assert_eq!(primed[0].1, PyValue::Int(id), "{field}");
    }
    // Let go, and it is the hull again.
    release(&mut reg, BOSS).unwrap();
    let hull = reg.character(BOSS).unwrap().ship_id;
    let info = all_info_of(&mut reg, BOSS);
    let PyValue::Dict(ships) = key(&info, "shipInfo").expect("present") else {
        panic!("shipInfo is a dict")
    };
    assert!(ships.iter().any(|(_, k)| *k == PyValue::Int(hull)));
    assert!(ships.iter().all(|(_, k)| *k != PyValue::Int(id)));
}

#[test]
fn the_controller_lists_the_bays_and_the_slots_as_themselves() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    dock_call(&mut reg, PAL, id).unwrap();
    stock(&mut reg, id, ctl::FLAG_FUEL);
    stock(&mut reg, id, ctl::FLAG_FUEL);
    stock(&mut reg, id, ctl::FLAG_DEED);
    stock(&mut reg, id, 164);
    // Before control the structure's id reads as the hangar, bays or not.
    let before = list_flag(&mut reg, BOSS, id, ctl::FLAG_FUEL);
    take(&mut reg, BOSS, id).unwrap();
    assert_eq!(list_flag(&mut reg, BOSS, id, ctl::FLAG_FUEL), 2);
    assert_eq!(list_flag(&mut reg, BOSS, id, ctl::FLAG_DEED), 1);
    assert_eq!(list_flag(&mut reg, BOSS, id, 164), 1);
    assert_eq!(list_flag(&mut reg, BOSS, id, 165), 0);
    // The hangar is still the hangar, with no flag or with its own.
    assert_eq!(list_flag(&mut reg, BOSS, id, crate::model::FLAG_HANGAR), 1);
    // A pilot who does not hold the controls gets the old answer.
    assert_eq!(list_flag(&mut reg, PAL, id, ctl::FLAG_FUEL), before);
    assert_ne!(before, 2);
}
