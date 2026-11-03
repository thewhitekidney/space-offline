//! #57 unit tests. **Every id and number is invented** (types 990,670,xxx,
//! items 100,670,xxx, pilots 90,670,xxx, station 60,670,001); the client's
//! constants appear only by name. The capacities are round test numbers, not
//! the game's.

use super::*;
use crate::item::find_in;
use crate::model::SandboxConfig;
use crate::space_state::{Motion, Presence};

const ALICE: i64 = 90_670_001;
const BOB: i64 = 90_670_002;
const STATION: i64 = 60_670_001;
const SYSTEM: i64 = 30_670_001;

const BOX: i64 = 990_670_001;
const SECURE: i64 = 990_670_002;
const AUDIT: i64 = 990_670_003;
const STUFF: i64 = 990_670_004;
const HULL: i64 = 990_670_005;
const GROUP_STUFF: i64 = 99_670;
const GROUP_HULL: i64 = 99_671;

/// Invented sizes: a box holds 100 m3, a secure box 50, stuff is 1 m3 a
/// unit, the hull's cargo hold is 60 m3.
const BOX_M3: f64 = 100.0;
const SECURE_M3: f64 = 50.0;
const HOLD_M3: f64 = 60.0;

const SHIP: i64 = 100_670_001;
const BOXES: i64 = 100_670_010; // a packaged stack of two, hangar
const SECURES: i64 = 100_670_011; // one packaged secure container, hangar
const AUDITS: i64 = 100_670_012; // one packaged audit-log container, hangar
const A_STUFF: i64 = 100_670_013; // 40 units, hangar
const OTHER_BOX: i64 = 100_670_014; // Bob's packaged box, Bob's hangar
const PARKED: i64 = 100_670_015; // a second, assembled ship of Alice's, hangar
const STACK: i64 = 40;

fn content() -> crate::content::Content {
    let container = |id: i64, group: i64, capacity: f64, name: &str| {
        format!("type\t{id}\t{group}\t2\t1\t1\t10.0\t{capacity}\t0.0\t1.0\t0\t\t\t{name}\n")
    };
    let rows = format!(
        "{}{}{}{}\
         type\t{STUFF}\t{GROUP_STUFF}\t4\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Stuff\n\
         type\t{HULL}\t{GROUP_HULL}\t6\t1\t1\t10.0\t{HOLD_M3}\t1000000.0\t30.0\t0\t\t9=1000.0\tInvented Hull\n",
        crate::typedata::FIXTURE,
        container(BOX, 12, BOX_M3, "Invented Box"),
        container(SECURE, 340, SECURE_M3, "Invented Secure Box"),
        container(AUDIT, 448, SECURE_M3, "Invented Audit Box"),
    );
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    let mut c = crate::content::Content::fixture();
    c.insert("typedata", crate::typedata::Installed::build(table));
    c
}

#[allow(clippy::too_many_arguments)]
fn make(
    c: &crate::content::Content,
    id: i64,
    type_id: i64,
    owner: i64,
    location: i64,
    flag: i64,
    quantity: i64,
    singleton: bool,
) -> Item {
    let t = find_in(c, type_id).expect("a test type");
    Item {
        item_id: id,
        type_id,
        owner_id: owner,
        location_id: location,
        flag_id: flag,
        quantity,
        group_id: t.group_id,
        category_id: t.category_id,
        singleton,
    }
}

/// Alice docked at [`STATION`], flying [`SHIP`]; in her hangar a packaged
/// stack of two boxes, one packaged secure box, one packaged audit-log box,
/// 40 units of stuff and a second ship; Bob docked too with a box of his own.
fn world() -> Registry {
    let content = std::sync::Arc::new(content());
    let pilot = |id: i64, name: &str, ship: i64| Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        ship_type_id: HULL,
        station_id: Some(STATION),
        ..Character::default()
    };
    let mut reg = Registry::new(SandboxConfig {
        characters: vec![pilot(ALICE, "Alice", SHIP), pilot(BOB, "Bob", SHIP + 100)],
        ..SandboxConfig::default()
    })
    .with_content(content.clone());
    let h = FLAG_HANGAR;
    let items = vec![
        make(&content, SHIP, HULL, ALICE, STATION, h, 1, true),
        make(&content, SHIP + 100, HULL, BOB, STATION, h, 1, true),
        make(&content, PARKED, HULL, ALICE, STATION, h, 1, true),
        make(&content, BOXES, BOX, ALICE, STATION, h, 2, false),
        make(&content, SECURES, SECURE, ALICE, STATION, h, 1, false),
        make(&content, AUDITS, AUDIT, ALICE, STATION, h, 1, false),
        make(&content, A_STUFF, STUFF, ALICE, STATION, h, STACK, false),
        make(&content, OTHER_BOX, BOX, BOB, STATION, h, 1, false),
    ];
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_671_000,
        ..crate::store::Snapshot::default()
    });
    reg
}

fn call(
    reg: &mut Registry,
    who: i64,
    service: &str,
    method: &str,
    args: &[PyValue],
    kwargs: &[(PyValue, PyValue)],
    bind: Option<&PyValue>,
) -> Result<PyValue, ServiceError> {
    reg.dispatch(&Request {
        service,
        method,
        args,
        kwargs,
        bind_params: bind,
        actor: Some(who),
    })
}

fn kw(name: &str, v: PyValue) -> (PyValue, PyValue) {
    (v, PyValue::Bytes(name.as_bytes().to_vec()))
}

fn inv(id: i64) -> PyValue {
    PyValue::Tuple(vec![PyValue::Int(id), PyValue::Int(STATION)])
}

fn hangar_inv() -> PyValue {
    PyValue::Tuple(vec![PyValue::Int(HANGAR_INVENTORY), PyValue::None])
}

fn assemble_item(reg: &mut Registry, who: i64, id: i64) -> Result<PyValue, ServiceError> {
    call(
        reg,
        who,
        "invbroker",
        "AssembleCargoContainer",
        &[PyValue::Int(id), PyValue::None, PyValue::Real(0.0)],
        &[],
        None,
    )
}

/// Assemble one and return its id.
fn assembled(reg: &mut Registry, packaged: i64) -> i64 {
    let before = reg.store().next_id();
    assemble_item(reg, ALICE, packaged).expect("assembled");
    reg.store()
        .items()
        .iter()
        .filter(|i| i.type_id == reg.item(packaged).map_or(0, |p| p.type_id) && i.singleton)
        .map(|i| i.item_id)
        .find(|id| *id == packaged || *id >= before)
        .expect("an assembled container")
}

fn add_to(
    reg: &mut Registry,
    who: i64,
    container: i64,
    item: i64,
    qty: Option<i64>,
) -> Result<PyValue, ServiceError> {
    let mut kws = vec![kw("flag", PyValue::None)];
    if let Some(q) = qty {
        kws.push(kw("qty", PyValue::Int(q)));
    }
    call(
        reg,
        who,
        "invbroker::inventory",
        "Add",
        &[PyValue::Int(item), PyValue::Int(HANGAR_INVENTORY)],
        &kws,
        Some(&inv(container)),
    )
}

fn list(reg: &mut Registry, who: i64, container: i64) -> Result<Vec<PyValue>, ServiceError> {
    match call(
        reg,
        who,
        "invbroker::inventory",
        "List",
        &[],
        &[kw("flag", PyValue::None)],
        Some(&inv(container)),
    )? {
        PyValue::List(rows) => Ok(rows),
        other => panic!("a list, got {other:?}"),
    }
}

fn units_in(reg: &Registry, container: i64) -> i64 {
    reg.store()
        .contents_of(container)
        .filter(|i| i.type_id == STUFF)
        .map(|i| i.quantity)
        .sum()
}

fn key_of(e: &ServiceError) -> String {
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
        panic!("not a raised UserError: {e:?}");
    };
    let PyValue::Tuple(h) = &**header else {
        panic!("odd header")
    };
    let PyValue::Tuple(args) = &h[1] else {
        panic!("odd args")
    };
    let PyValue::Bytes(key) = &args[0] else {
        panic!("odd key")
    };
    String::from_utf8_lossy(key).into_owned()
}

/// The words of a `CustomNotify`.
fn notify_text(e: &ServiceError) -> String {
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
        panic!("not a raised UserError: {e:?}");
    };
    let PyValue::Tuple(h) = &**header else {
        panic!("odd header")
    };
    let PyValue::Tuple(args) = &h[1] else {
        panic!("odd args")
    };
    let PyValue::Dict(d) = &args[1] else {
        panic!("odd dict")
    };
    // A marshal dict entry is `(value, key)`.
    match d
        .iter()
        .find(|(_, k)| *k == PyValue::Bytes(b"notify".to_vec()))
    {
        Some((PyValue::Bytes(t), _)) => String::from_utf8_lossy(t).into_owned(),
        other => panic!("no notify text: {other:?}"),
    }
}

fn set_pw(
    reg: &mut Registry,
    who: i64,
    container: i64,
    which: i64,
    old: Option<&str>,
    new: &str,
) -> Result<PyValue, ServiceError> {
    call(
        reg,
        who,
        "invbroker::inventory",
        "SetPassword",
        &[
            PyValue::Int(which),
            old.map_or(PyValue::None, |o| PyValue::Unicode(o.to_owned())),
            PyValue::Unicode(new.to_owned()),
        ],
        &[],
        Some(&inv(container)),
    )
}

fn enter_pw(reg: &mut Registry, who: i64, container: i64, pw: &str) -> PyValue {
    call(
        reg,
        who,
        "invbroker::inventory",
        "EnterPassword",
        &[
            PyValue::Int(PASSWORD_GENERAL),
            PyValue::Unicode(pw.to_owned()),
        ],
        &[],
        Some(&inv(container)),
    )
    .expect("answered")
}

fn has_pw(reg: &mut Registry, container: i64, which: i64) -> PyValue {
    call(
        reg,
        ALICE,
        "invbroker::inventory",
        "HasExistingPasswordSet",
        &[PyValue::Int(which)],
        &[],
        Some(&inv(container)),
    )
    .expect("answered")
}

/// A pilot's session forgets what they entered when the server restarts.
fn restart_sessions(reg: &mut Registry) {
    reg.ext_mut::<Entered>().0.clear();
}

// ---- assembling ----

/// One off a stack of two: a new singleton id in place, the stack left with
/// one, the name the pilot gave kept, the client told of both.
#[test]
fn assembling_one_of_two_splits_it_off_and_names_it() {
    let mut reg = world();
    let _ = reg.take_pending();
    let named = call(
        &mut reg,
        ALICE,
        "invbroker",
        "AssembleCargoContainer",
        &[
            PyValue::Int(BOXES),
            PyValue::Unicode("Ore box".to_owned()),
            PyValue::Real(0.0),
        ],
        &[],
        None,
    )
    .expect("assembled");
    assert_eq!(named, PyValue::None, "the client drops the answer");
    assert_eq!(reg.item(BOXES).unwrap().quantity, 1);
    assert!(!reg.item(BOXES).unwrap().singleton);
    let new = reg
        .store()
        .items()
        .iter()
        .find(|i| i.type_id == BOX && i.singleton)
        .expect("the assembled one")
        .clone();
    assert_ne!(new.item_id, BOXES);
    assert_eq!(
        (new.owner_id, new.location_id, new.flag_id, new.quantity),
        (ALICE, STATION, FLAG_HANGAR, 1)
    );
    assert_eq!(
        crate::item_names::name_of(&reg, new.item_id).as_deref(),
        Some("Ore box")
    );
    assert!(!reg.take_pending().is_empty(), "OnItemChange went out");
}

/// A stack of one is assembled in place and keeps its id.
#[test]
fn assembling_a_single_container_keeps_its_id() {
    let mut reg = world();
    assemble_item(&mut reg, ALICE, SECURES).expect("assembled");
    let i = reg.item(SECURES).unwrap();
    assert!(i.singleton);
    assert_eq!((i.location_id, i.flag_id), (STATION, FLAG_HANGAR));
    // Again: left alone.
    assemble_item(&mut reg, ALICE, SECURES).expect("a no-op");
}

/// What the client's menu does not offer is refused in words: not yours, not a
/// container, in space. A container in a ship's hold is assembled there.
#[test]
fn assembling_refuses_what_is_not_offered() {
    let mut reg = world();
    assert!(assemble_item(&mut reg, ALICE, OTHER_BOX).is_err(), "Bob's");
    assert!(assemble_item(&mut reg, ALICE, A_STUFF).is_err(), "stuff");
    assert!(assemble_item(&mut reg, ALICE, 1).is_err(), "no such item");
    assert!(!reg.item(OTHER_BOX).unwrap().singleton);

    // Into the hold, and assembled there.
    reg.apply_items(&[Edit::Move {
        item_id: AUDITS,
        to: Slot::new(ALICE, SHIP, FLAG_CARGO),
    }])
    .unwrap();
    assemble_item(&mut reg, ALICE, AUDITS).expect("assembled in the hold");
    assert!(reg.item(AUDITS).unwrap().singleton);

    // In space: CanOnlyDoInStations.
    let _ = reg.roster_mut().leave(ALICE);
    reg.roster_mut().arrive(Presence::arriving(
        ALICE,
        SHIP,
        SYSTEM,
        Motion::arrived([0.0; 3], [0.0; 3]),
    ));
    let e = assemble_item(&mut reg, ALICE, SECURES).unwrap_err();
    assert_eq!(key_of(&e), "CanOnlyDoInStations");
}

// ---- moving in and out ----

/// Stuff goes in with `Add(item, source, qty=, flag=None)`: a part of the
/// stack, then the rest; it is listed by the container's own inventory, its
/// volume is counted, and it is gone from the hangar.
#[test]
fn items_go_into_a_container_and_are_listed() {
    let mut reg = world();
    let bx = assembled(&mut reg, BOXES);
    add_to(&mut reg, ALICE, bx, A_STUFF, Some(15)).expect("a part in");
    assert_eq!(units_in(&reg, bx), 15);
    assert_eq!(reg.item(A_STUFF).unwrap().quantity, STACK - 15);
    let inside = reg
        .store()
        .contents_of(bx)
        .find(|i| i.type_id == STUFF)
        .unwrap()
        .clone();
    assert_eq!(
        (inside.owner_id, inside.flag_id),
        (ALICE, FLAG_IN_CONTAINER),
        "owned by the pilot, flag none"
    );
    assert_eq!(list(&mut reg, ALICE, bx).unwrap().len(), 1);
    add_to(&mut reg, ALICE, bx, A_STUFF, None).expect("the rest in");
    assert_eq!(units_in(&reg, bx), STACK);
    assert_eq!(
        reg.item(A_STUFF).unwrap().location_id,
        bx,
        "the whole stack moved, keeping its id"
    );
    // The hangar listing no longer shows what is inside.
    let hangar = call(
        &mut reg,
        ALICE,
        "invbroker::inventory",
        "List",
        &[],
        &[],
        Some(&hangar_inv()),
    )
    .unwrap();
    let PyValue::List(rows) = hangar else {
        panic!("a list")
    };
    assert!(
        !rows
            .iter()
            .any(|r| format!("{r:?}").contains("Invented Stuff"))
    );
    // `GetCapacity` is the type's, with what is in it.
    let cap = call(
        &mut reg,
        ALICE,
        "invbroker::inventory",
        "GetCapacity",
        &[PyValue::None],
        &[],
        Some(&inv(bx)),
    )
    .unwrap();
    let PyValue::Instance { state, .. } = &cap else {
        panic!("a KeyVal instance, got {cap:?}")
    };
    assert_eq!(
        crate::pyutil::dict_get(state, b"capacity"),
        Some(&PyValue::Real(BOX_M3))
    );
    assert_eq!(
        crate::pyutil::dict_get(state, b"used"),
        Some(&PyValue::Real(STACK as f64))
    );
}

/// A move that does not fit is `NotEnoughCargoSpace`, the key the client
/// catches to offer the quantity that does; nothing moves.
#[test]
fn a_full_container_refuses_with_the_clients_key() {
    let mut reg = world();
    let sec = assembled(&mut reg, SECURES);
    // 50 m3 of room, 40 units of 1 m3: fits. Add 20 more from a new stack.
    add_to(&mut reg, ALICE, sec, A_STUFF, None).expect("40 in");
    let more = reg
        .apply_items(&[Edit::Create {
            type_id: STUFF,
            quantity: 20,
            to: Slot::new(ALICE, STATION, FLAG_HANGAR),
        }])
        .unwrap();
    let more_id = more
        .iter()
        .find_map(|c| match c {
            Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .unwrap();
    let e = add_to(&mut reg, ALICE, sec, more_id, None).unwrap_err();
    assert_eq!(key_of(&e), "NotEnoughCargoSpace");
    assert_eq!(reg.item(more_id).unwrap().quantity, 20, "nothing moved");
    add_to(&mut reg, ALICE, sec, more_id, Some(10)).expect("the 10 that fit");
    assert_eq!(units_in(&reg, sec), 50);
}

/// Out to the hangar, and out to a ship's cargo hold (with the hold's room
/// checked); a part of a stack stays.
#[test]
fn items_come_out_to_the_hangar_and_to_a_hold() {
    let mut reg = world();
    let bx = assembled(&mut reg, BOXES);
    add_to(&mut reg, ALICE, bx, A_STUFF, None).expect("in");
    let stuff = reg
        .store()
        .contents_of(bx)
        .find(|i| i.type_id == STUFF)
        .unwrap()
        .item_id;
    // 10 to the hangar: `Add(item, container, qty=10, flag=hangar)` on the hangar.
    call(
        &mut reg,
        ALICE,
        "invbroker::inventory",
        "Add",
        &[PyValue::Int(stuff), PyValue::Int(bx)],
        &[
            kw("qty", PyValue::Int(10)),
            kw("flag", PyValue::Int(FLAG_HANGAR)),
        ],
        Some(&hangar_inv()),
    )
    .expect("out to the hangar");
    let out = reg
        .store()
        .at(Slot::new(ALICE, STATION, FLAG_HANGAR))
        .find(|i| i.type_id == STUFF)
        .unwrap()
        .clone();
    assert_eq!(out.quantity, 10);
    assert_eq!(units_in(&reg, bx), STACK - 10);
    // The rest to the active ship's hold: 60 m3 of room, 30 units.
    call(
        &mut reg,
        ALICE,
        "invbroker::inventory",
        "MultiAdd",
        &[PyValue::List(vec![PyValue::Int(stuff)]), PyValue::Int(bx)],
        &[kw("flag", PyValue::Int(FLAG_CARGO))],
        Some(&inv(SHIP)),
    )
    .expect("out to the hold");
    assert_eq!(units_in(&reg, bx), 0);
    assert_eq!(reg.item(stuff).unwrap().location_id, SHIP);
    assert_eq!(reg.item(stuff).unwrap().flag_id, FLAG_CARGO);
    // More than the hold takes is refused with the hold's own key.
    reg.apply_items(&[Edit::Create {
        type_id: STUFF,
        quantity: 40,
        to: Slot::new(ALICE, bx, FLAG_IN_CONTAINER),
    }])
    .unwrap();
    let more = reg
        .store()
        .contents_of(bx)
        .find(|i| i.type_id == STUFF)
        .unwrap()
        .item_id;
    let e = call(
        &mut reg,
        ALICE,
        "invbroker::inventory",
        "Add",
        &[PyValue::Int(more), PyValue::Int(bx)],
        &[kw("flag", PyValue::Int(FLAG_CARGO))],
        Some(&inv(SHIP)),
    )
    .unwrap_err();
    assert_eq!(key_of(&e), "NotEnoughCargoSpace");
    assert_eq!(reg.item(more).unwrap().location_id, bx);
}

/// A container in a ship's hold takes items from that hangar; another pilot's
/// container, a container in space, and an assembled container or ship put
/// inside are refused.
#[test]
fn reach_and_nesting_are_refused() {
    let mut reg = world();
    let bx = assembled(&mut reg, BOXES);
    // Bob's container is not Alice's to fill, and Alice's is not Bob's.
    let theirs = {
        assemble_item(&mut reg, BOB, OTHER_BOX).expect("Bob's");
        OTHER_BOX
    };
    assert!(add_to(&mut reg, ALICE, theirs, A_STUFF, None).is_err());
    assert!(add_to(&mut reg, BOB, bx, A_STUFF, None).is_err());
    assert_eq!(reg.item(A_STUFF).unwrap().location_id, STATION);
    // A second assembled box does not go into the first; nor a ship.
    let second = reg.item(BOXES).map(|i| i.item_id).unwrap();
    let again = assembled(&mut reg, second);
    let e = add_to(&mut reg, ALICE, bx, again, None).unwrap_err();
    assert!(
        notify_text(&e).contains("inside another container"),
        "{e:?}"
    );
    let e = add_to(&mut reg, ALICE, bx, PARKED, None).unwrap_err();
    assert_eq!(key_of(&e), "AssembledShipsDontFitInCargo");
    let e = add_to(&mut reg, ALICE, bx, bx, None).unwrap_err();
    assert_eq!(key_of(&e), "CannotPlaceItemInsideItself");
    // In space nothing is reachable.
    let _ = reg.roster_mut().leave(ALICE);
    reg.roster_mut().arrive(Presence::arriving(
        ALICE,
        SHIP,
        SYSTEM,
        Motion::arrived([0.0; 3], [0.0; 3]),
    ));
    assert!(add_to(&mut reg, ALICE, bx, A_STUFF, None).is_err());
}

/// A container sitting in the active ship's hold is usable from the hangar
/// of its station.
#[test]
fn a_container_in_a_ship_hold_is_usable() {
    let mut reg = world();
    reg.apply_items(&[Edit::Move {
        item_id: SECURES,
        to: Slot::new(ALICE, SHIP, FLAG_CARGO),
    }])
    .unwrap();
    let sec = assembled(&mut reg, SECURES);
    add_to(&mut reg, ALICE, sec, A_STUFF, Some(5)).expect("in");
    assert_eq!(units_in(&reg, sec), 5);
}

/// "Stack All" inside a container folds two stacks of one type into one.
#[test]
fn stack_all_folds_a_containers_stacks() {
    let mut reg = world();
    let bx = assembled(&mut reg, BOXES);
    add_to(&mut reg, ALICE, bx, A_STUFF, Some(10)).unwrap();
    add_to(&mut reg, ALICE, bx, A_STUFF, Some(10)).unwrap();
    assert_eq!(
        reg.store()
            .contents_of(bx)
            .filter(|i| i.type_id == STUFF)
            .count(),
        2
    );
    call(
        &mut reg,
        ALICE,
        "invbroker::inventory",
        "StackAll",
        &[PyValue::None],
        &[],
        Some(&inv(bx)),
    )
    .expect("stacked");
    assert_eq!(
        reg.store()
            .contents_of(bx)
            .filter(|i| i.type_id == STUFF)
            .count(),
        1
    );
    assert_eq!(units_in(&reg, bx), 20);
}

/// Dropping a stack on a stack of its type inside a container merges them
/// (`MultiMerge`), and the capacity check still applies.
#[test]
fn a_stack_dropped_on_a_stack_in_a_container_merges() {
    let mut reg = world();
    let bx = assembled(&mut reg, BOXES);
    add_to(&mut reg, ALICE, bx, A_STUFF, Some(10)).unwrap();
    let inside = reg
        .store()
        .contents_of(bx)
        .find(|i| i.type_id == STUFF)
        .unwrap()
        .item_id;
    let ops = PyValue::List(vec![PyValue::Tuple(vec![
        PyValue::Int(A_STUFF),
        PyValue::Int(inside),
        PyValue::Int(5),
    ])]);
    call(
        &mut reg,
        ALICE,
        "invbroker::inventory",
        "MultiMerge",
        &[ops, PyValue::Int(HANGAR_INVENTORY)],
        &[],
        Some(&inv(bx)),
    )
    .expect("merged");
    assert_eq!(units_in(&reg, bx), 15);
    assert_eq!(reg.item(A_STUFF).unwrap().quantity, STACK - 15);
}

/// Repackaging an emptied container is the existing `repackagingSvc`'s:
/// contents come out to the hangar. A container with stuff in it is
/// repackaged and the stuff follows into the hangar.
#[test]
fn repackaging_a_container_brings_its_contents_to_the_hangar() {
    let mut reg = world();
    let bx = assembled(&mut reg, BOXES);
    add_to(&mut reg, ALICE, bx, A_STUFF, None).unwrap();
    let batch = PyValue::Dict(vec![(
        PyValue::List(vec![PyValue::Tuple(vec![
            PyValue::Int(bx),
            PyValue::Int(STATION),
        ])]),
        PyValue::Int(STATION),
    )]);
    call(
        &mut reg,
        ALICE,
        "repackagingSvc",
        "RepackageItems",
        &[batch],
        &[],
        None,
    )
    .expect("repackaged");
    assert!(!reg.item(bx).unwrap().singleton);
    assert_eq!(
        reg.item(
            reg.store()
                .at(Slot::new(ALICE, STATION, FLAG_HANGAR))
                .find(|i| i.type_id == STUFF)
                .map(|i| i.item_id)
                .expect("the stuff is in the hangar")
        )
        .unwrap()
        .quantity,
        STACK
    );
}

// ---- passwords ----

/// A plain cargo container takes no password; a secure one does, and a
/// config password is the audit-log container's alone.
#[test]
fn only_a_secure_container_takes_a_password() {
    let mut reg = world();
    let bx = assembled(&mut reg, BOXES);
    let sec = assembled(&mut reg, SECURES);
    assert!(set_pw(&mut reg, ALICE, bx, PASSWORD_GENERAL, None, "hunter").is_err());
    assert_eq!(has_pw(&mut reg, bx, PASSWORD_GENERAL), PyValue::Bool(false));
    assert!(set_pw(&mut reg, ALICE, sec, PASSWORD_CONFIG, None, "hunter").is_err());
    set_pw(&mut reg, ALICE, sec, PASSWORD_GENERAL, None, "hunter").expect("set");
    assert_eq!(has_pw(&mut reg, sec, PASSWORD_GENERAL), PyValue::Bool(true));
    assert_eq!(has_pw(&mut reg, sec, PASSWORD_CONFIG), PyValue::Bool(false));
    let audit = assembled(&mut reg, AUDITS);
    set_pw(&mut reg, ALICE, audit, PASSWORD_CONFIG, None, "cfgpass").expect("config set");
    assert_eq!(
        has_pw(&mut reg, audit, PASSWORD_CONFIG),
        PyValue::Bool(true)
    );
    assert_eq!(
        has_pw(&mut reg, audit, PASSWORD_GENERAL),
        PyValue::Bool(false)
    );
}

/// The rules of `SetPassword`: not another pilot's, three to sixteen
/// characters, the old password when there is one, not the same.
#[test]
fn set_password_checks_who_how_long_and_the_old_one() {
    let mut reg = world();
    let sec = assembled(&mut reg, SECURES);
    let e = set_pw(&mut reg, BOB, sec, PASSWORD_GENERAL, None, "hunter").unwrap_err();
    assert_eq!(key_of(&e), "PermissionDenied");
    assert!(set_pw(&mut reg, ALICE, sec, PASSWORD_GENERAL, None, "ab").is_err());
    let e = set_pw(
        &mut reg,
        ALICE,
        sec,
        PASSWORD_GENERAL,
        None,
        &"x".repeat(MAX_PASSWORD + 1),
    )
    .unwrap_err();
    assert_eq!(key_of(&e), "CantSetPasswordTooLong");
    assert!(set_pw(&mut reg, ALICE, sec, 7, None, "hunter").is_err());
    set_pw(&mut reg, ALICE, sec, PASSWORD_GENERAL, None, "hunter").unwrap();
    let e = set_pw(&mut reg, ALICE, sec, PASSWORD_GENERAL, None, "swordfish").unwrap_err();
    assert_eq!(key_of(&e), "SCGeneralPasswordRequired", "no old password");
    let e = set_pw(
        &mut reg,
        ALICE,
        sec,
        PASSWORD_GENERAL,
        Some("nope"),
        "swordfish",
    )
    .unwrap_err();
    assert_eq!(
        key_of(&e),
        "SCGeneralPasswordRequired",
        "wrong old password"
    );
    let e = set_pw(
        &mut reg,
        ALICE,
        sec,
        PASSWORD_GENERAL,
        Some("hunter"),
        "hunter",
    )
    .unwrap_err();
    assert_eq!(key_of(&e), "CantChangePasswordToSame");
    set_pw(
        &mut reg,
        ALICE,
        sec,
        PASSWORD_GENERAL,
        Some("hunter"),
        "swordfish",
    )
    .expect("changed");
    assert_eq!(
        password_of(&reg, sec, PASSWORD_GENERAL).as_deref(),
        Some("swordfish")
    );
}

/// The whole client protocol: a secure container asks for its password to
/// list, to add to and to take from; the setter has entered it; after a
/// "restart" nobody has; a wrong password asks again; the right one opens it;
/// changing it closes it to anyone who entered the old one.
#[test]
fn a_secure_container_asks_for_its_password() {
    let mut reg = world();
    let sec = assembled(&mut reg, SECURES);
    set_pw(&mut reg, ALICE, sec, PASSWORD_GENERAL, None, "hunter").unwrap();
    // The setter is in.
    add_to(&mut reg, ALICE, sec, A_STUFF, Some(10)).expect("the setter has entered it");
    assert_eq!(list(&mut reg, ALICE, sec).unwrap().len(), 1);

    restart_sessions(&mut reg);
    for e in [
        list(&mut reg, ALICE, sec).unwrap_err(),
        add_to(&mut reg, ALICE, sec, A_STUFF, Some(1)).unwrap_err(),
        call(
            &mut reg,
            ALICE,
            "invbroker::inventory",
            "StackAll",
            &[PyValue::None],
            &[],
            Some(&inv(sec)),
        )
        .unwrap_err(),
    ] {
        assert_eq!(key_of(&e), "SCGeneralPasswordRequired");
    }
    // Taking out of it, from the hangar's inventory: the cache key, which the
    // client turns into the same dialog.
    let inside = reg
        .store()
        .contents_of(sec)
        .find(|i| i.type_id == STUFF)
        .unwrap()
        .item_id;
    let take = |reg: &mut Registry| {
        call(
            reg,
            ALICE,
            "invbroker::inventory",
            "Add",
            &[PyValue::Int(inside), PyValue::Int(sec)],
            &[kw("flag", PyValue::Int(FLAG_HANGAR))],
            Some(&hangar_inv()),
        )
    };
    assert_eq!(
        key_of(&take(&mut reg).unwrap_err()),
        "PasswordProtectedCacheInvalid"
    );
    assert_eq!(reg.item(inside).unwrap().location_id, sec, "nothing moved");

    // A wrong word: answered false, still closed.
    assert_eq!(
        enter_pw(&mut reg, ALICE, sec, "guess"),
        PyValue::Bool(false)
    );
    assert!(list(&mut reg, ALICE, sec).is_err());
    // The right one.
    assert_eq!(
        enter_pw(&mut reg, ALICE, sec, "hunter"),
        PyValue::Bool(true)
    );
    assert_eq!(list(&mut reg, ALICE, sec).unwrap().len(), 1);
    take(&mut reg).expect("out");
    assert_eq!(reg.item(inside).unwrap().location_id, STATION);

    // Changing the password closes it to what was entered before.
    set_pw(
        &mut reg,
        ALICE,
        sec,
        PASSWORD_GENERAL,
        Some("hunter"),
        "swordfish",
    )
    .unwrap();
    restart_sessions(&mut reg);
    enter_pw(&mut reg, ALICE, sec, "hunter");
    assert!(
        list(&mut reg, ALICE, sec).is_err(),
        "the old word is no key"
    );
    enter_pw(&mut reg, ALICE, sec, "swordfish");
    assert!(list(&mut reg, ALICE, sec).is_ok());
}

/// `RetrievePassword` is the owner's.
#[test]
fn the_owner_can_retrieve_the_password() {
    let mut reg = world();
    let sec = assembled(&mut reg, SECURES);
    let retrieve = |reg: &mut Registry, who: i64| {
        call(
            reg,
            who,
            "invbroker::inventory",
            "RetrievePassword",
            &[PyValue::Int(PASSWORD_GENERAL)],
            &[],
            Some(&inv(sec)),
        )
    };
    assert_eq!(retrieve(&mut reg, ALICE).unwrap(), PyValue::None);
    set_pw(&mut reg, ALICE, sec, PASSWORD_GENERAL, None, "hunter").unwrap();
    assert_eq!(
        retrieve(&mut reg, ALICE).unwrap(),
        PyValue::Unicode("hunter".to_owned())
    );
    assert_eq!(
        key_of(&retrieve(&mut reg, BOB).unwrap_err()),
        "PermissionDenied"
    );
}

/// A password survives a restart through the delta rows; what pilots had
/// entered does not, and a freshly assembled container has none.
#[test]
fn passwords_persist_but_entries_do_not() {
    let mut reg = world();
    let sec = assembled(&mut reg, SECURES);
    set_pw(&mut reg, ALICE, sec, PASSWORD_GENERAL, None, "hunter").unwrap();
    let snapshot = reg.to_snapshot();
    let rows: Vec<(Vec<String>, Vec<String>)> = reg
        .deltas()
        .of(KIND)
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    assert_eq!(rows.len(), 1);
    let mut again = world();
    again.apply_snapshot(snapshot);
    for (k, v) in rows {
        again.deltas_mut().put(KIND, k, v);
    }
    assert_eq!(
        password_of(&again, sec, PASSWORD_GENERAL).as_deref(),
        Some("hunter")
    );
    assert!(
        list(&mut again, ALICE, sec).is_err(),
        "nobody has entered it yet"
    );

    // A stale row on an id that is assembled anew is forgotten.
    let mut fresh = world();
    fresh
        .deltas_mut()
        .put(KIND, key(SECURES, PASSWORD_GENERAL), vec!["old".to_owned()]);
    assemble_item(&mut fresh, ALICE, SECURES).unwrap();
    assert_eq!(password_of(&fresh, SECURES, PASSWORD_GENERAL), None);
}

/// A gone or repackaged container's rows are dropped at the next write.
#[test]
fn stale_password_rows_are_pruned() {
    let mut reg = world();
    let sec = assembled(&mut reg, SECURES);
    reg.deltas_mut().put(
        KIND,
        key(100_670_999, PASSWORD_GENERAL),
        vec!["x".to_owned()],
    );
    set_pw(&mut reg, ALICE, sec, PASSWORD_GENERAL, None, "hunter").unwrap();
    assert_eq!(reg.deltas().of(KIND).len(), 1, "the dead id's row went");
}
