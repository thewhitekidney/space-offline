//! A pilot assembles a strategic cruiser with four subsystems over the wire,
//! sees its slots, and cannot undock with fewer (lane `t3-d`, T3-D). Invented
//! ids and rows (`crate::subsystems::tests::content`); the shapes are the
//! client's: `ship.AssembleShip(shipID, name=..., subSystems=[ids])`
//! (`invItemFunctions.py:201`, the list is `assembleModularShip.Submit`'s
//! `subSystems.values()`) and the fused `ship.Undock`.

use super::*;
use crate::subsystems::tests::{ACTOR, MODULE, SUBS, id_of, world, world_with};
use crate::subsystems::{FIRST_FLAG, GROUP_STRATEGIC_CRUISER, VISIBLE};

fn station_access(station: i64) -> PyValue {
    PyValue::Tuple(vec![PyValue::Int(station), PyValue::Int(15)])
}

fn kw(name: &str, v: PyValue) -> (PyValue, PyValue) {
    (v, PyValue::Bytes(name.as_bytes().to_vec()))
}

fn call(
    reg: &mut Registry,
    station: i64,
    service: &str,
    method: &str,
    args: &[PyValue],
    kwargs: &[(PyValue, PyValue)],
    bind: PyValue,
) -> Result<PyValue, ServiceError> {
    let _ = station;
    dispatch(
        reg,
        &Request {
            service,
            method,
            args,
            kwargs,
            bind_params: Some(&bind),
            actor: Some(ACTOR),
        },
    )
}

/// Packaged: `Repackage`, as the client's repackaging does.
fn packaged_world() -> (Registry, i64, i64) {
    let (mut reg, station, hull) = world(1);
    reg.apply_items(&[crate::item::Edit::Repackage { item_id: hull }])
        .expect("repackaged");
    (reg, station, hull)
}

fn assemble(
    reg: &mut Registry,
    station: i64,
    hull: i64,
    subs: &[i64],
) -> Result<PyValue, ServiceError> {
    call(
        reg,
        station,
        "ship",
        "AssembleShip",
        &[PyValue::Int(hull)],
        &[
            kw("name", PyValue::None),
            kw(
                "subSystems",
                PyValue::List(subs.iter().map(|i| PyValue::Int(*i)).collect()),
            ),
        ],
        station_access(station),
    )
}

fn undock(reg: &mut Registry, station: i64) -> Result<PyValue, ServiceError> {
    call(
        reg,
        station,
        "ship",
        "Undock",
        &[],
        &[],
        station_access(station),
    )
}

fn ids_of_subs(reg: &Registry) -> Vec<i64> {
    SUBS.iter().map(|t| id_of(reg, *t)).collect()
}

#[test]
fn assemble_with_subsystems_fits_all_four() {
    let (mut reg, station, hull) = packaged_world();
    assert_eq!(reg.store().get(hull).map(|i| i.singleton), Some(false));
    // The dialog's `values()` order is not slot order: reverse it.
    let mut subs = ids_of_subs(&reg);
    subs.reverse();
    let got = assemble(&mut reg, station, hull, &subs).expect("assembled");
    assert_eq!(got, PyValue::Int(hull), "the answer is the ship's id");
    assert_eq!(reg.store().get(hull).map(|i| i.singleton), Some(true));
    for (n, t) in SUBS.iter().enumerate() {
        let s = reg.store().get(id_of(&reg, *t)).expect("the subsystem");
        assert_eq!(
            (s.location_id, s.flag_id, s.singleton),
            (hull, FIRST_FLAG + n as i64, true),
            "subsystem {n} in its own flag"
        );
    }
    // The ship's slots are the subsystems' sums, on the sheet.
    let sheet = crate::dogma::sheet(&reg, hull).expect("a sheet");
    assert_eq!(sheet.ship(crate::dogma::attr::HI_SLOTS), 6.0);
    assert_eq!(sheet.ship(crate::dogma::attr::TURRET_SLOTS), 3.0);
    // The HUD's prime lists the four, passive (no isOnline), with attributes.
    let c = reg.character(ACTOR).cloned().expect("Alice");
    let rows = crate::modules::ship_info_entries(&reg, &c);
    assert_eq!(rows.len() as i64, VISIBLE, "{rows:?}");
}

#[test]
fn a_fit_into_the_assembled_hull_takes_a_module_in_a_new_slot() {
    let (mut reg, station, hull) = packaged_world();
    let subs = ids_of_subs(&reg);
    assemble(&mut reg, station, hull, &subs).expect("assembled");
    let module = id_of(&reg, MODULE);
    call(
        &mut reg,
        station,
        "invbroker::inventory",
        "Add",
        &[PyValue::Int(module), PyValue::Int(station)],
        &[
            kw("qty", PyValue::Int(1)),
            kw(
                "flag",
                PyValue::Int(crate::fitting::Rack::High.first_flag()),
            ),
        ],
        PyValue::Tuple(vec![PyValue::Int(hull), PyValue::Int(station)]),
    )
    .expect("a high slot the subsystems made");
    assert!(
        reg.store()
            .contents_of(hull)
            .any(|i| i.type_id == MODULE && i.flag_id == crate::fitting::Rack::High.first_flag())
    );
}

#[test]
fn assemble_with_fewer_than_four_is_refused() {
    let (mut reg, station, hull) = packaged_world();
    let subs = ids_of_subs(&reg);
    let before = reg.store().items().to_vec();
    for n in [0, 3] {
        let got = assemble(&mut reg, station, hull, &subs[..n]);
        assert!(matches!(got, Err(ServiceError::Raise(_))), "{n}: {got:?}");
        assert_eq!(reg.store().items().to_vec(), before, "nothing moved at {n}");
    }
    // A subsystem for another hull in the four is refused whole.
    let mut mixed = subs.clone();
    mixed[0] = id_of(&reg, crate::subsystems::tests::OTHER_SUB);
    let got = assemble(&mut reg, station, hull, &mixed);
    assert!(matches!(got, Err(ServiceError::Raise(_))), "{got:?}");
    assert_eq!(reg.store().items().to_vec(), before);
    // And with the call carrying no `subSystems` at all.
    let none = call(
        &mut reg,
        station,
        "ship",
        "AssembleShip",
        &[PyValue::Int(hull)],
        &[],
        station_access(station),
    );
    assert!(matches!(none, Err(ServiceError::Raise(_))), "{none:?}");
    assert_eq!(reg.store().items().to_vec(), before);
    assert_eq!(
        reg.store().get(hull).map(|i| i.group_id),
        Some(GROUP_STRATEGIC_CRUISER)
    );
}

#[test]
fn undock_with_three_is_refused() {
    let (mut reg, station, hull) = packaged_world();
    let subs = ids_of_subs(&reg);
    assemble(&mut reg, station, hull, &subs).expect("assembled");
    assert_eq!(undock(&mut reg, station), Ok(PyValue::None), "with four");
    // Take one out, docked: the unfit is the hangar `Add`.
    let off = id_of(&reg, SUBS[1]);
    call(
        &mut reg,
        station,
        "invbroker::inventory",
        "Add",
        &[PyValue::Int(off), PyValue::Int(hull)],
        &[
            kw("qty", PyValue::None),
            kw("flag", PyValue::Int(crate::model::FLAG_HANGAR)),
        ],
        PyValue::Tuple(vec![PyValue::Int(CONTAINER_HANGAR), PyValue::None]),
    )
    .expect("a subsystem out of its slot, docked");
    assert_eq!(crate::subsystems::fitted_count(reg.store(), hull), 3);
    let got = undock(&mut reg, station);
    assert!(matches!(got, Err(ServiceError::Raise(_))), "{got:?}");
    let e = format!("{:?}", got.unwrap_err());
    let key = b"TooFewSubSystemsToUndock"
        .iter()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    assert!(e.contains(&key), "the client's own key: {e}");
}

#[test]
fn a_named_strategic_assemble_names_the_hull() {
    let (mut reg, station, hull) = packaged_world();
    let subs = ids_of_subs(&reg);
    let got = call(
        &mut reg,
        station,
        "ship",
        "AssembleShip",
        &[PyValue::Int(hull)],
        &[
            kw("name", PyValue::Unicode("Tengu one".to_owned())),
            kw(
                "subSystems",
                PyValue::List(subs.iter().map(|i| PyValue::Int(*i)).collect()),
            ),
        ],
        station_access(station),
    )
    .expect("assembled");
    let PyValue::Int(id) = got else {
        panic!("an id")
    };
    assert_eq!(
        crate::item_names::name_of(&reg, id).as_deref(),
        Some("Tengu one")
    );
}

/// SKILLGATE: one subsystem the pilot cannot use refuses the whole assemble
/// before anything is applied.
#[test]
fn assemble_with_an_unskilled_subsystem_applies_nothing() {
    let (mut reg, station, hull) = world_with(1, true);
    reg.apply_items(&[crate::item::Edit::Repackage { item_id: hull }])
        .expect("repackaged");
    let before = reg.store().items().to_vec();
    let subs = ids_of_subs(&reg);
    let got = assemble(&mut reg, station, hull, &subs);
    assert!(matches!(got, Err(ServiceError::Raise(_))), "{got:?}");
    assert_eq!(
        reg.store().items(),
        before.as_slice(),
        "nothing was applied"
    );
}
