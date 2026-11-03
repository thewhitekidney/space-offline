//! U5 (issue #65): the home services in a structure. Every id is invented.

use super::*;
use crate::clones::StationRef;
use crate::repair::Hurt;

/// The capsule's invented price: a priced hull is a repairable, insurable one.
const PRICE: f64 = 1_000_000.0;

fn priced() -> (Registry, i64) {
    let (reg, id) = world();
    let mut c = content();
    let prices = crate::econ_prices::Table::parse(&[format!(
        "{}\t{PRICE}\trecipe\t{}\t6\t25\t1\t1",
        crate::service::TYPE_CAPSULE,
        PRICE / 2.0
    )])
    .expect("parses");
    c.insert("econ_prices", prices);
    let mut reg = reg.with_content(std::sync::Arc::new(c));
    for who in [BOSS, PAL] {
        reg.character_mut(who).unwrap().balance = 10.0 * PRICE;
    }
    (reg, id)
}

fn ids(v: &[i64]) -> PyValue {
    PyValue::List(v.iter().map(|i| PyValue::Int(*i)).collect())
}

fn ship_of(reg: &Registry, who: i64) -> i64 {
    reg.character(who).unwrap().ship_id
}

fn balance(reg: &Registry, who: i64) -> f64 {
    reg.character(who).unwrap().balance
}

fn harbour_ref(reg: &Registry) -> StationRef {
    reg.station_ref(HARBOUR).expect("the harbour is a station")
}

#[test]
fn a_docked_pilot_is_offered_the_home_services() {
    let (mut reg, id) = priced();
    dock_call(&mut reg, BOSS, id).unwrap();
    let svc = call(
        &mut reg,
        BOSS,
        "structureSettings",
        "CharacterGetServices",
        &[PyValue::Int(id)],
    )
    .unwrap();
    let PyValue::Dict(d) = svc else { panic!() };
    let have: Vec<PyValue> = d.into_iter().map(|(_, k)| k).collect();
    for want in [
        sp::SERVICE_MEDICAL,
        sp::SERVICE_REPAIR,
        sp::SERVICE_INSURANCE,
        sp::SERVICE_JUMP_CLONE,
    ] {
        assert!(have.contains(&PyValue::Int(want)), "{want} in {have:?}");
    }
}

#[test]
fn the_repair_shop_of_a_structure_mends_the_whole_quote_for_isk() {
    let (mut reg, id) = priced();
    dock_call(&mut reg, BOSS, id).unwrap();
    let ship = ship_of(&reg, BOSS);
    crate::repair::put(
        &mut reg,
        ship,
        Hurt {
            armor: 10.0,
            structure: 5.0,
        },
    );
    // The window asks for the quote of the hangar ship; it has a row.
    let quote = call(
        &mut reg,
        BOSS,
        "repairSvc",
        "GetRepairQuotes",
        &[ids(&[ship])],
    )
    .unwrap();
    let PyValue::Dict(d) = quote else { panic!() };
    let PyValue::List(rows) = &d[0].0 else {
        panic!()
    };
    assert_eq!(rows.len(), 1);
    let before = balance(&reg, BOSS);
    call(
        &mut reg,
        BOSS,
        "repairSvc",
        "RepairItemsInStructure",
        &[ids(&[ship])],
    )
    .expect("repairs");
    assert!(crate::repair::hurt(&reg, ship).is_none(), "mended");
    assert!(balance(&reg, BOSS) < before, "and paid for");
}

#[test]
fn a_structure_repair_without_the_isk_is_refused_and_leaves_the_damage() {
    let (mut reg, id) = priced();
    dock_call(&mut reg, BOSS, id).unwrap();
    let ship = ship_of(&reg, BOSS);
    crate::repair::put(
        &mut reg,
        ship,
        Hurt {
            armor: 10.0,
            structure: 5.0,
        },
    );
    reg.character_mut(BOSS).unwrap().balance = 0.0;
    let e = call(
        &mut reg,
        BOSS,
        "repairSvc",
        "RepairItemsInStructure",
        &[ids(&[ship])],
    )
    .unwrap_err();
    assert!(refusal(&e).contains("NotEnoughMoney"), "{e:?}");
    assert!(crate::repair::hurt(&reg, ship).is_some());
}

#[test]
fn insurance_is_sold_in_a_structure() {
    let (mut reg, id) = priced();
    dock_call(&mut reg, BOSS, id).unwrap();
    let ship = ship_of(&reg, BOSS);
    let full = crate::service::insurance::full_price(&reg, crate::service::TYPE_CAPSULE);
    assert!(full > 0.0);
    let amount = crate::service::insurance::premium(full, 0.5);
    let before = balance(&reg, BOSS);
    call(
        &mut reg,
        BOSS,
        "insuranceSvc",
        "InsureShip",
        &[
            PyValue::Int(ship),
            PyValue::Real(amount),
            PyValue::Bool(false),
        ],
    )
    .expect("insured");
    assert!((before - balance(&reg, BOSS) - amount).abs() < 1e-6);
    let got = call(
        &mut reg,
        BOSS,
        "insuranceSvc",
        "GetContractForShip",
        &[PyValue::Int(ship)],
    )
    .unwrap();
    assert_ne!(got, PyValue::None);
}

#[test]
fn a_jump_clone_is_installed_in_the_structure_and_listed_at_its_id() {
    let (mut reg, id) = priced();
    dock_call(&mut reg, BOSS, id).unwrap();
    let v = call(
        &mut reg,
        BOSS,
        "jumpCloneSvc",
        "ValidateInstallJumpClone",
        &[],
    )
    .unwrap();
    assert_eq!(v, PyValue::List(vec![]), "nothing stands in the way");
    call(
        &mut reg,
        BOSS,
        "jumpCloneSvc",
        "InstallCloneInStructure",
        &[],
    )
    .expect("installs");
    let mine = reg.clones.of(BOSS);
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].1.station.station_id, id);
    // One per place, and the answer tells the window so.
    let v = call(
        &mut reg,
        BOSS,
        "jumpCloneSvc",
        "ValidateInstallJumpClone",
        &[],
    )
    .unwrap();
    assert_ne!(v, PyValue::List(vec![]));
    let n = call(
        &mut reg,
        BOSS,
        "jumpCloneSvc",
        "GetNumClonesInPilotsStructure",
        &[],
    )
    .unwrap();
    assert_eq!(n, PyValue::Int(1));
}

#[test]
fn the_structure_can_be_made_home_and_a_revive_wakes_there_with_a_structureid() {
    let (mut reg, id) = priced();
    dock_call(&mut reg, BOSS, id).unwrap();
    let cand = call(
        &mut reg,
        BOSS,
        "home_station",
        "get_home_station_candidates",
        &[],
    )
    .unwrap();
    let PyValue::List(c) = cand else { panic!() };
    assert_eq!(c.len(), 1);
    call(
        &mut reg,
        BOSS,
        "home_station",
        "set_home_station",
        &[PyValue::Int(id), PyValue::Bool(false)],
    )
    .expect("sets");
    assert_eq!(crate::clones::home_of(&reg, BOSS).unwrap().0.station_id, id);
    assert!(names(&mut reg).contains(&"OnHomeStationChanged"));
    // Out in space in a pod, killed: the revive is a dock in the structure.
    let pod = ship_of(&reg, BOSS);
    reg.roster_mut().arrive(Presence::arriving(
        BOSS,
        pod,
        SYSTEM,
        Motion::arrived([0.0; 3], [0.0; 3]),
    ));
    let _ = reg.take_session_changes();
    let r = crate::clones::revive_in_clone(&mut reg, BOSS).expect("revived");
    assert_eq!(r.station.station_id, id);
    assert_eq!(reg.docked_station(BOSS), Some(id));
    let cap = reg.store().get(r.capsule_id).expect("a capsule");
    assert_eq!(cap.location_id, id);
    // The structure's session key, not the station's pair.
    assert_eq!(
        key(&r.change, "structureid"),
        Some(&PyValue::Tuple(vec![PyValue::None, PyValue::Int(id)]))
    );
    assert!(key(&r.change, "stationid").is_none(), "{:?}", r.change);
    assert!(key(&r.change, "solarsystemid").is_none(), "{:?}", r.change);
    assert!(key(&r.change, "shipid").is_some());
}

#[test]
fn a_revive_from_a_structure_to_an_npc_station_swaps_the_keys() {
    let (mut reg, id) = priced();
    dock_call(&mut reg, BOSS, id).unwrap();
    // Home is the harbour: a clone jump and a self-destruct both go there.
    let h = harbour_ref(&reg);
    reg.clones.homes.insert(BOSS, h);
    let _ = reg.take_session_changes();
    call(&mut reg, BOSS, "home_station", "self_destruct_clone", &[]).expect("goes");
    let changes = reg.take_session_changes();
    let (_, ch) = &changes[0];
    assert_eq!(
        key(ch, "structureid"),
        Some(&PyValue::Tuple(vec![PyValue::Int(id), PyValue::None]))
    );
    assert_eq!(
        key(ch, "stationid"),
        Some(&PyValue::Tuple(vec![PyValue::None, PyValue::Int(HARBOUR)]))
    );
    assert_eq!(
        key(ch, "solarsystemid"),
        Some(&PyValue::Tuple(vec![PyValue::Int(SYSTEM), PyValue::None]))
    );
    assert_eq!(reg.docked_station(BOSS), Some(HARBOUR));
    assert_eq!(docked_count(&reg, id), 0);
}

#[test]
fn a_clone_jump_from_the_harbour_into_the_structure_uses_the_structure_keys() {
    let (mut reg, id) = priced();
    // BOSS is docked at the harbour, in a pod, with a jump clone in the
    // structure.
    reg.roster_mut().leave(BOSS);
    let s = structure::get(reg.deltas(), id).unwrap();
    let there = station_ref(&reg, &s);
    let clone = reg.clones.install(BOSS, there, String::new());
    let _ = reg.take_session_changes();
    call(
        &mut reg,
        BOSS,
        "jumpCloneSvc",
        "CloneJump",
        &[
            PyValue::Int(id),
            PyValue::Int(clone),
            PyValue::Real(0.0),
            PyValue::Bool(false),
        ],
    )
    .expect("jumps");
    assert_eq!(reg.docked_station(BOSS), Some(id));
    let changes = reg.take_session_changes();
    let (_, ch) = &changes[0];
    assert_eq!(
        key(ch, "stationid"),
        Some(&PyValue::Tuple(vec![PyValue::Int(HARBOUR), PyValue::None]))
    );
    assert_eq!(
        key(ch, "structureid"),
        Some(&PyValue::Tuple(vec![PyValue::None, PyValue::Int(id)]))
    );
    assert_eq!(
        key(ch, "solarsystemid"),
        Some(&PyValue::Tuple(vec![PyValue::None, PyValue::Int(SYSTEM)]))
    );
    assert_eq!(docked_count(&reg, id), 1);
}

#[test]
fn a_revive_goes_elsewhere_when_the_home_structure_stops_taking_the_pilot() {
    let (mut reg, id) = priced();
    dock_call(&mut reg, PAL, id).unwrap();
    call(
        &mut reg,
        PAL,
        "home_station",
        "set_home_station",
        &[PyValue::Int(id), PyValue::Bool(false)],
    )
    .unwrap();
    // PAL leaves the owner's corporation: the dock right goes with it.
    reg.corporations_mut().enrol_for_test(PAL, FOE_CORP, 0);
    reg.character_mut(PAL).unwrap().corporation_id = FOE_CORP;
    let pod = ship_of(&reg, PAL);
    reg.roster_mut().arrive(Presence::arriving(
        PAL,
        pod,
        SYSTEM,
        Motion::arrived([0.0; 3], [0.0; 3]),
    ));
    let r = crate::clones::revive_in_clone(&mut reg, PAL).expect("revived");
    assert_eq!(r.station.station_id, HARBOUR);
    assert_eq!(reg.docked_station(PAL), Some(HARBOUR));
}

#[test]
fn a_structure_leaving_space_takes_its_homes_and_clones_with_it() {
    let (mut reg, id) = priced();
    dock_call(&mut reg, BOSS, id).unwrap();
    call(
        &mut reg,
        BOSS,
        "home_station",
        "set_home_station",
        &[PyValue::Int(id), PyValue::Bool(false)],
    )
    .unwrap();
    call(
        &mut reg,
        BOSS,
        "jumpCloneSvc",
        "InstallCloneInStructure",
        &[],
    )
    .unwrap();
    let _ = names(&mut reg);
    let s = structure::get(reg.deltas(), id).unwrap();
    evacuate(&mut reg, &s);
    assert!(reg.clones.home(BOSS).is_none());
    assert!(reg.clones.of(BOSS).is_empty());
    let sent = names(&mut reg);
    assert!(
        sent.contains(&"OnHomeStationMovedFromStructure"),
        "{sent:?}"
    );
    assert!(sent.contains(&"OnJumpCloneCacheInvalidated"), "{sent:?}");
    // The home reads as where they are based now.
    assert_eq!(
        crate::clones::home_of(&reg, BOSS).unwrap().0.station_id,
        HARBOUR
    );
}

#[test]
fn a_contract_can_name_a_structure_as_a_place() {
    let (reg, id) = priced();
    let p = place_ref(&reg, id).expect("a structure is a place");
    assert_eq!(p.solar_system_id, SYSTEM);
    assert!(reg.station_ref(id).is_none(), "though it is no station");
    assert!(place_ref(&reg, HARBOUR).is_some());
}
