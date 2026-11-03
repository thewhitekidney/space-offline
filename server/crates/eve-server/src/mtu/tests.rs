//! #82 unit tests. **Every id and number is invented** (types 990,820,xxx,
//! items 100,820,xxx, pilots 90,820,xxx); the client's constants appear only
//! by name (`GROUP_MTU`, `spec(..)`). The capacities and the tractor velocity
//! are round test numbers, not the game's.

use super::*;
use crate::item::find_in;
use crate::model::SandboxConfig;
use crate::space_state::{Motion, Presence};

const ALICE: i64 = 90_820_001;
const BOB: i64 = 90_820_002;
const SYSTEM: i64 = 30_820_001;
const SHIP: i64 = 100_820_001;
const BOB_SHIP: i64 = 100_820_002;

const DEPOT: i64 = 990_820_001;
const TRACTOR: i64 = 990_820_002;
const BOX: i64 = 990_820_003;
const SECURE: i64 = 990_820_004;
const STUFF: i64 = 990_820_005;
const HULL: i64 = 990_820_006;
const SHELL: i64 = 990_820_007;
const GROUP_STUFF: i64 = 99_820;
const GROUP_HULL: i64 = 99_821;
const GROUP_PLAIN_SINGLETON: i64 = 99_822;
const TRINKET: i64 = 990_820_008;

/// Invented sizes. The hull's hold is 500 m3; a depot holds 300, an MTU 200,
/// a box 100, a secure box 50; stuff is 1 m3 a unit. The invented tractor
/// moves 100 m/s.
const HOLD_M3: f64 = 500.0;
const DEPOT_M3: f64 = 300.0;
const MTU_M3: f64 = 200.0;
const SECURE_M3: f64 = 50.0;
const PULL_MS: f64 = 100.0;

fn content() -> crate::content::Content {
    let rows = format!(
        "{}\
         type\t{DEPOT}\t{GROUP_MOBILE_DEPOT}\t22\t1\t1\t10.0\t{DEPOT_M3}\t0.0\t1.0\t0\t\t\tInvented Depot\n\
         type\t{TRACTOR}\t{GROUP_MTU}\t22\t1\t1\t10.0\t{MTU_M3}\t0.0\t1.0\t0\t\t{ATTR_MAX_TRACTOR_VELOCITY}={PULL_MS}\tInvented Tractor\n\
         type\t{BOX}\t12\t2\t1\t1\t10.0\t100.0\t0.0\t1.0\t0\t\t\tInvented Box\n\
         type\t{SECURE}\t340\t2\t1\t1\t10.0\t{SECURE_M3}\t0.0\t1.0\t0\t\t\tInvented Secure Box\n\
         type\t{STUFF}\t{GROUP_STUFF}\t4\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Stuff\n\
         type\t{SHELL}\t{}\t4\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Wreck\n\
         type\t{TRINKET}\t{GROUP_PLAIN_SINGLETON}\t4\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Trinket\n\
         type\t{HULL}\t{GROUP_HULL}\t6\t1\t1\t10.0\t{HOLD_M3}\t1000000.0\t30.0\t0\t\t9=1000.0\tInvented Hull\n",
        crate::typedata::FIXTURE,
        combat::GROUP_WRECK,
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

/// Alice and Bob in space in [`SYSTEM`], Alice at the origin and Bob ten
/// metres off, each in an invented hull; nothing in either hold.
fn world() -> Registry {
    let content = std::sync::Arc::new(content());
    let pilot = |id: i64, name: &str, ship: i64, corp: i64| Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        ship_type_id: HULL,
        corporation_id: corp,
        ..Character::default()
    };
    let mut reg = Registry::new(SandboxConfig {
        characters: vec![
            pilot(ALICE, "Alice", SHIP, 98_820_001),
            pilot(BOB, "Bob", BOB_SHIP, 98_820_002),
        ],
        ..SandboxConfig::default()
    })
    .with_content(content.clone());
    let items = vec![
        make(&content, SHIP, HULL, ALICE, SYSTEM, 0, 1, true),
        make(&content, BOB_SHIP, HULL, BOB, SYSTEM, 0, 1, true),
    ];
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_821_000,
        ..crate::store::Snapshot::default()
    });
    let _ = reg.take_pending();
    for (who, ship, x) in [(ALICE, SHIP, 0.0), (BOB, BOB_SHIP, 10.0)] {
        reg.roster_mut().arrive(Presence::arriving(
            who,
            ship,
            SYSTEM,
            Motion::arrived([x, 0.0, 0.0], [0.0; 3]),
        ));
    }
    reg
}

fn spawn(reg: &mut Registry, type_id: i64, n: i64, to: Slot) -> i64 {
    let changes = reg
        .apply_items(&[Edit::Create {
            type_id,
            quantity: n,
            to,
        }])
        .expect("a known type");
    let _ = reg.take_pending();
    changes
        .iter()
        .find_map(|c| match c {
            Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .expect("created")
}

fn cargo(reg: &mut Registry, owner: i64, ship: i64, type_id: i64, n: i64) -> i64 {
    spawn(reg, type_id, n, Slot::new(owner, ship, FLAG_CARGO))
}

/// An assembled container in the hold: a stack of one, assembled.
fn container_in_hold(reg: &mut Registry, owner: i64, ship: i64, type_id: i64) -> i64 {
    let id = cargo(reg, owner, ship, type_id, 1);
    reg.apply_items(&[Edit::Assemble {
        item_id: id,
        to: Slot::new(owner, ship, FLAG_CARGO),
    }])
    .expect("assembled");
    let _ = reg.take_pending();
    id
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
    PyValue::Tuple(vec![PyValue::Int(id), PyValue::Int(SYSTEM)])
}

fn ints(v: &[i64]) -> PyValue {
    PyValue::List(v.iter().map(|n| PyValue::Int(*n)).collect())
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

/// The words of a `CustomNotify` (a marshal dict entry is `(value, key)`).
fn notify_text(e: &ServiceError) -> String {
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
        return format!("{e:?}");
    };
    let PyValue::Tuple(h) = &**header else {
        return format!("{e:?}");
    };
    let Some(PyValue::Tuple(args)) = h.get(1) else {
        return format!("{e:?}");
    };
    match args.get(1) {
        Some(PyValue::Dict(d)) => d
            .iter()
            .find_map(|(v, k)| match (v, k) {
                (PyValue::Bytes(t), PyValue::Bytes(k)) if k == b"notify" => {
                    Some(String::from_utf8_lossy(t).into_owned())
                }
                _ => None,
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// `ship.Drop([(item, 1)], charid, False)`.
fn drop_item(reg: &mut Registry, who: i64, item: i64) -> PyValue {
    call(
        reg,
        who,
        "ship",
        "Drop",
        &[
            PyValue::List(vec![PyValue::Tuple(vec![
                PyValue::Int(item),
                PyValue::Int(1),
            ])]),
            PyValue::Int(who),
            PyValue::Bool(false),
        ],
        &[],
        None,
    )
    .expect("an answer")
}

/// The id a launch answered for `item`, or the refusal text.
fn launched_id(answer: &PyValue, item: i64) -> Result<i64, String> {
    let PyValue::Dict(d) = answer else {
        panic!("a dict: {answer:?}")
    };
    let (value, key) = d.first().expect("one entry");
    assert_eq!(key, &PyValue::Int(item), "keyed by the item asked for");
    let PyValue::List(v) = value else {
        panic!("a list")
    };
    match &v[0] {
        PyValue::Int(id) => Ok(*id),
        PyValue::Tuple(t) => match &t[1] {
            PyValue::Dict(d) => Err(d
                .iter()
                .find_map(|(v, _)| match v {
                    PyValue::Bytes(t) => Some(String::from_utf8_lossy(t).into_owned()),
                    _ => None,
                })
                .unwrap_or_default()),
            other => Err(format!("{other:?}")),
        },
        other => Err(format!("{other:?}")),
    }
}

/// Launch one depot or MTU from a stack of `n` in Alice's hold and return its
/// id; activated at once.
fn deployed(reg: &mut Registry, type_id: i64) -> i64 {
    let stack = cargo(reg, ALICE, SHIP, type_id, 2);
    let id = launched_id(&drop_item(reg, ALICE, stack), stack).expect("launched");
    activate(reg, id);
    id
}

/// Make a unit active now: the row's activation moved into the past.
fn activate(reg: &mut Registry, id: i64) {
    let mut row = row_of(reg, id).expect("a row");
    row.active_at = 0;
    remember(reg, id, row);
}

fn list_of(reg: &mut Registry, who: i64, unit: i64) -> Result<PyValue, ServiceError> {
    call(
        reg,
        who,
        "invbroker::inventory",
        "List",
        &[],
        &[kw("flag", PyValue::None)],
        Some(&inv(unit)),
    )
}

// ---------------------------------------------------------------------------
// Launching a container whole
// ---------------------------------------------------------------------------

/// A container in the hold goes out as itself: same id, in space, in the
/// second list, with its contents and its password row; cargo goes into a can
/// beside it, in the first.
#[test]
fn a_jettisoned_container_launches_whole() {
    let mut reg = world();
    let secure = container_in_hold(&mut reg, ALICE, SHIP, SECURE);
    let inside = spawn(
        &mut reg,
        STUFF,
        7,
        Slot::new(ALICE, secure, FLAG_IN_CONTAINER),
    );
    reg.deltas_mut().put(
        crate::containers::KIND,
        vec![secure.to_string(), "1".to_owned()],
        vec!["pass-word".to_owned()],
    );
    let loose = cargo(&mut reg, ALICE, SHIP, STUFF, 5);
    let answer = call(
        &mut reg,
        ALICE,
        "ship",
        "Jettison",
        &[ints(&[secure, loose])],
        &[],
        None,
    )
    .expect("jettisoned");
    let PyValue::Tuple(lists) = answer else {
        panic!("a tuple")
    };
    assert_eq!(lists[0], ints(&[loose]), "the cargo went into a can");
    assert_eq!(
        lists[1],
        ints(&[secure]),
        "the container launched as itself"
    );
    let c = reg.item(secure).expect("kept");
    assert_eq!(
        (c.owner_id, c.location_id, c.flag_id, c.singleton),
        (ALICE, SYSTEM, combat::FLAG_IN_SPACE, true)
    );
    let wr = reg.combat().wreck(secure).copied().expect("on the grid");
    assert_eq!((wr.owner_id, wr.type_id), (ALICE, SECURE));
    assert!(wr.expires_at > crate::pyutil::filetime_now());
    let held = reg.item(inside).expect("contents kept");
    assert_eq!(held.location_id, secure);
    assert_eq!(
        crate::containers::password_of(&reg, secure, 1).as_deref(),
        Some("pass-word"),
        "the password stays with it"
    );
    assert!(row_of(&reg, secure).is_some(), "and a restart finds it");
}

/// A singleton that is not a container stays where it is.
#[test]
fn another_singleton_stays_in_the_hold() {
    let mut reg = world();
    let loose = cargo(&mut reg, ALICE, SHIP, STUFF, 5);
    let trinket = cargo(&mut reg, ALICE, SHIP, TRINKET, 1);
    reg.apply_items(&[Edit::Assemble {
        item_id: trinket,
        to: Slot::new(ALICE, SHIP, FLAG_CARGO),
    }])
    .expect("assembled");
    let (to_can, launched) =
        crate::jettison::jettison_items(&mut reg, Some(ALICE), &[trinket, loose]).expect("some");
    assert_eq!((to_can, launched.is_empty()), (vec![loose], true));
    assert_eq!(reg.item(trinket).unwrap().location_id, SHIP);
}

/// Launched containers do not pile up on one spot, and only the pilot's own
/// container, in the active ship, goes.
#[test]
fn only_the_pilots_own_containers_launch_and_apart() {
    let mut reg = world();
    let a = container_in_hold(&mut reg, ALICE, SHIP, BOX);
    let b = container_in_hold(&mut reg, ALICE, SHIP, BOX);
    let bobs = container_in_hold(&mut reg, BOB, BOB_SHIP, BOX);
    let (_, launched) =
        crate::jettison::jettison_items(&mut reg, Some(ALICE), &[a, b, bobs]).expect("two");
    assert_eq!(launched.len(), 2);
    assert_eq!(reg.item(bobs).unwrap().location_id, BOB_SHIP);
    let pa = reg.combat().wreck(a).unwrap().position;
    let pb = reg.combat().wreck(b).unwrap().position;
    assert!(crate::space_state::distance(pa, pb) > 1.0, "{pa:?} {pb:?}");
    let e = crate::jettison::jettison_items(&mut reg, Some(ALICE), &[bobs]).unwrap_err();
    assert!(user_error_key(&e).contains("NothingToJettison"), "{e:?}");
}

// ---------------------------------------------------------------------------
// Scooping it back
// ---------------------------------------------------------------------------

/// A launched secure container asks for its password with `ShpScoopSecureCC`;
/// the right one scoops it, contents and all, and it is off the grid.
#[test]
fn a_scoop_of_a_secure_container_asks_for_the_password() {
    let mut reg = world();
    let secure = container_in_hold(&mut reg, ALICE, SHIP, SECURE);
    let inside = spawn(
        &mut reg,
        STUFF,
        3,
        Slot::new(ALICE, secure, FLAG_IN_CONTAINER),
    );
    reg.deltas_mut().put(
        crate::containers::KIND,
        vec![secure.to_string(), "1".to_owned()],
        vec!["pass-word".to_owned()],
    );
    crate::jettison::jettison_items(&mut reg, Some(ALICE), &[secure]).expect("launched");
    let scoop = |reg: &mut Registry, who: i64, args: &[PyValue]| {
        call(reg, who, "ship", "Scoop", args, &[], None)
    };
    let e = scoop(&mut reg, BOB, &[PyValue::Int(secure)]).unwrap_err();
    assert!(user_error_key(&e).contains("ShpScoopSecureCC"), "{e:?}");
    let e = scoop(
        &mut reg,
        BOB,
        &[PyValue::Int(secure), PyValue::Unicode("not-it".to_owned())],
    )
    .unwrap_err();
    assert!(user_error_key(&e).contains("ShpScoopSecureCC"), "{e:?}");
    assert!(reg.combat().wreck(secure).is_some(), "still out there");
    scoop(
        &mut reg,
        BOB,
        &[
            PyValue::Int(secure),
            PyValue::Unicode("pass-word".to_owned()),
        ],
    )
    .expect("scooped with the word");
    let c = reg.item(secure).unwrap();
    assert_eq!(
        (c.owner_id, c.location_id, c.flag_id),
        (BOB, BOB_SHIP, FLAG_CARGO)
    );
    assert_eq!(reg.item(inside).unwrap().location_id, secure);
    assert!(reg.combat().wreck(secure).is_none());
    assert!(row_of(&reg, secure).is_none(), "no longer parked");
}

// ---------------------------------------------------------------------------
// A container in space: opening, filling, emptying
// ---------------------------------------------------------------------------

/// Beside a launched container, its password gates the listing and the add;
/// with it the pilot's cargo goes in, within the container's capacity.
#[test]
fn a_floating_container_takes_cargo_behind_its_password() {
    let mut reg = world();
    let secure = container_in_hold(&mut reg, ALICE, SHIP, SECURE);
    reg.deltas_mut().put(
        crate::containers::KIND,
        vec![secure.to_string(), "1".to_owned()],
        vec!["pass-word".to_owned()],
    );
    crate::jettison::jettison_items(&mut reg, Some(ALICE), &[secure]).expect("launched");
    let stuff = cargo(&mut reg, BOB, BOB_SHIP, STUFF, 80);
    let add = |reg: &mut Registry, qty: i64| {
        call(
            reg,
            BOB,
            "invbroker::inventory",
            "Add",
            &[PyValue::Int(stuff), PyValue::Int(BOB_SHIP)],
            &[kw("qty", PyValue::Int(qty)), kw("flag", PyValue::None)],
            Some(&inv(secure)),
        )
    };
    let e = add(&mut reg, 10).unwrap_err();
    assert!(
        user_error_key(&e).contains("SCGeneralPasswordRequired"),
        "{e:?}"
    );
    let e = list_of(&mut reg, BOB, secure).unwrap_err();
    assert!(
        user_error_key(&e).contains("SCGeneralPasswordRequired"),
        "{e:?}"
    );
    call(
        &mut reg,
        BOB,
        "invbroker::inventory",
        "EnterPassword",
        &[PyValue::Int(1), PyValue::Unicode("pass-word".to_owned())],
        &[],
        Some(&inv(secure)),
    )
    .expect("entered");
    add(&mut reg, 30).expect("30 m3 into 50");
    assert_eq!(crate::containers::used_volume(&reg, secure), 30.0);
    let e = add(&mut reg, 40).unwrap_err();
    assert!(user_error_key(&e).contains("NotEnoughCargoSpace"), "{e:?}");
    assert!(list_of(&mut reg, BOB, secure).is_ok());
}

// ---------------------------------------------------------------------------
// Launching a deployable
// ---------------------------------------------------------------------------

/// A depot comes off a stack of two as a singleton with a new id, a record
/// that lasts its decay, a row, and a position the spec's distance from the
/// ship; the answer is `{stack: [new id]}`.
#[test]
fn a_depot_launches_from_the_hold() {
    let mut reg = world();
    let stack = cargo(&mut reg, ALICE, SHIP, DEPOT, 2);
    let sp = spec(reg.content(), GROUP_MOBILE_DEPOT).expect("a spec");
    let before = crate::pyutil::filetime_now();
    let id = launched_id(&drop_item(&mut reg, ALICE, stack), stack).expect("launched");
    assert_ne!(id, stack, "one unit came off the stack");
    assert_eq!(reg.item(stack).unwrap().quantity, 1);
    let unit = reg.item(id).unwrap();
    assert_eq!(
        (
            unit.owner_id,
            unit.location_id,
            unit.flag_id,
            unit.singleton
        ),
        (ALICE, SYSTEM, combat::FLAG_IN_SPACE, true)
    );
    let wr = reg.combat().wreck(id).copied().expect("a ball");
    assert!(
        (wr.position[0] - sp.deploy_at).abs() < 1e-6,
        "{:?}",
        wr.position
    );
    assert!(wr.expires_at >= before + sp.decay);
    let row = row_of(&reg, id).expect("a row");
    assert_eq!(row.system, SYSTEM);
    assert!(
        row.active_at >= before + sp.activate,
        "shut while it activates"
    );
    assert!(is_unit(reg.content(), unit));
}

/// Not in space, not in the hold, not near its own kind, and not a deployable
/// at all: refused or passed on.
#[test]
fn a_launch_is_refused_where_the_checks_say() {
    let mut reg = world();
    let first = cargo(&mut reg, ALICE, SHIP, DEPOT, 1);
    let second = cargo(&mut reg, ALICE, SHIP, DEPOT, 1);
    launched_id(&drop_item(&mut reg, ALICE, first), first).expect("the first");
    let why = launched_id(&drop_item(&mut reg, ALICE, second), second).unwrap_err();
    assert!(why.contains("another of its kind"), "{why}");
    assert_eq!(reg.item(second).unwrap().location_id, SHIP);
    // Bob cannot launch Alice's.
    let why = launched_id(&drop_item(&mut reg, BOB, second), second).unwrap_err();
    assert!(why.contains("hold"), "{why}");
    // Docked or between: nothing.
    reg.roster_mut().leave(ALICE);
    let why = launched_id(&drop_item(&mut reg, ALICE, second), second).unwrap_err();
    assert!(why.contains("in space"), "{why}");
    // Something that is no deployable is the next handler's.
    let stuff = cargo(&mut reg, BOB, BOB_SHIP, STUFF, 1);
    let req = Request {
        service: "ship",
        method: "Drop",
        args: &[PyValue::List(vec![PyValue::Tuple(vec![
            PyValue::Int(stuff),
            PyValue::Int(1),
        ])])],
        kwargs: &[],
        bind_params: None,
        actor: Some(BOB),
    };
    assert!(drop_call(&mut reg, &req).is_none());
}

// ---------------------------------------------------------------------------
// The depot's cargo
// ---------------------------------------------------------------------------

/// Shut while it activates, the owner's alone afterwards.
#[test]
fn a_depot_is_shut_until_active_and_then_the_owners() {
    let mut reg = world();
    let stack = cargo(&mut reg, ALICE, SHIP, DEPOT, 1);
    let id = launched_id(&drop_item(&mut reg, ALICE, stack), stack).expect("launched");
    let e = list_of(&mut reg, ALICE, id).unwrap_err();
    assert!(notify_text(&e).contains("activating"), "{e:?}");
    activate(&mut reg, id);
    assert!(list_of(&mut reg, ALICE, id).is_ok());
    let e = list_of(&mut reg, BOB, id).unwrap_err();
    assert!(notify_text(&e).contains("not yours"), "{e:?}");
}

/// The owner fills a depot from the hold, up to its capacity, and takes it
/// back; a stranger can do neither. An MTU takes nothing by hand.
#[test]
fn a_depot_takes_and_gives_the_owners_cargo() {
    let mut reg = world();
    let depot = deployed(&mut reg, DEPOT);
    let stuff = cargo(&mut reg, ALICE, SHIP, STUFF, 400);
    let add = |reg: &mut Registry, who: i64, id: i64, qty: i64| {
        call(
            reg,
            who,
            "invbroker::inventory",
            "Add",
            &[PyValue::Int(id), PyValue::Int(SHIP)],
            &[kw("qty", PyValue::Int(qty)), kw("flag", PyValue::None)],
            Some(&inv(depot)),
        )
    };
    add(&mut reg, ALICE, stuff, 250).expect("250 of 300 m3");
    assert_eq!(crate::containers::used_volume(&reg, depot), 250.0);
    assert_eq!(reg.item(stuff).unwrap().quantity, 150);
    let e = add(&mut reg, ALICE, stuff, 100).unwrap_err();
    assert!(user_error_key(&e).contains("NotEnoughCargoSpace"), "{e:?}");
    let theirs = cargo(&mut reg, BOB, BOB_SHIP, STUFF, 5);
    assert!(add(&mut reg, BOB, theirs, 5).is_err());
    // A GetCapacity answers anyone's window.
    call(
        &mut reg,
        ALICE,
        "invbroker::inventory",
        "GetCapacity",
        &[],
        &[],
        Some(&inv(depot)),
    )
    .expect("a capacity");
    // The way out: the owner's drag to the hold is the loot path, a stranger's
    // is refused at the gate.
    let inside = reg
        .store()
        .contents_of(depot)
        .next()
        .map(|i| i.item_id)
        .unwrap();
    let take = |reg: &mut Registry, who: i64, ship: i64| {
        call(
            reg,
            who,
            "invbroker::inventory",
            "Add",
            &[PyValue::Int(inside), PyValue::Int(depot)],
            &[
                kw("qty", PyValue::Int(50)),
                kw("flag", PyValue::Int(FLAG_CARGO)),
            ],
            Some(&inv(ship)),
        )
    };
    assert!(take(&mut reg, BOB, BOB_SHIP).is_err());
    take(&mut reg, ALICE, SHIP).expect("the owner takes");
    assert_eq!(crate::containers::used_volume(&reg, depot), 200.0);
}

#[test]
fn an_mtu_cannot_be_filled_by_hand() {
    let mut reg = world();
    let mtu = deployed(&mut reg, TRACTOR);
    let stuff = cargo(&mut reg, ALICE, SHIP, STUFF, 5);
    let e = call(
        &mut reg,
        ALICE,
        "invbroker::inventory",
        "Add",
        &[PyValue::Int(stuff), PyValue::Int(SHIP)],
        &[kw("flag", PyValue::None)],
        Some(&inv(mtu)),
    )
    .unwrap_err();
    assert!(notify_text(&e).contains("by hand"), "{e:?}");
    assert_eq!(reg.item(stuff).unwrap().location_id, SHIP);
}

// ---------------------------------------------------------------------------
// The MTU
// ---------------------------------------------------------------------------

/// A wreck of `owner` at `x` metres along the axis with `n` units of stuff in
/// it.
fn wreck_at(reg: &mut Registry, owner: i64, corp: i64, x: f64, n: i64) -> (i64, i64) {
    let id = spawn(
        reg,
        SHELL,
        1,
        Slot::new(owner, SYSTEM, combat::FLAG_IN_SPACE),
    );
    let stuff = spawn(reg, STUFF, n, Slot::new(owner, id, combat::FLAG_IN_WRECK));
    combat::add_wreck(
        reg,
        Wreck {
            wreck_id: id,
            type_id: SHELL,
            solar_system_id: SYSTEM,
            position: [x, 0.0, 0.0],
            ship_id: 0,
            owner_id: owner,
            corp_id: corp,
            fleet_id: None,
            abandoned: false,
            expires_at: crate::pyutil::filetime_now() + combat::WRECK_LIFETIME,
        },
    );
    (id, stuff)
}

fn mtu_x(reg: &Registry, id: i64) -> f64 {
    reg.combat().wreck(id).unwrap().position[0]
}

/// In reach, an active MTU empties the owner's wreck into its cargo, as the
/// owner's; out of reach it pulls one step; a wreck its owner has no rights
/// to is left alone; and it works once a cycle.
#[test]
fn an_mtu_loots_what_is_near_and_pulls_what_is_far() {
    let mut reg = world();
    let mtu = deployed(&mut reg, TRACTOR);
    let sp = spec(reg.content(), GROUP_MTU).unwrap();
    let t = sp.tractor.unwrap();
    let at = mtu_x(&reg, mtu);
    let (near, near_stuff) = wreck_at(&mut reg, ALICE, 98_820_001, at + 1_000.0, 30);
    let (far, far_stuff) = wreck_at(&mut reg, ALICE, 98_820_001, at + 20_000.0, 10);
    let (foreign, foreign_stuff) = wreck_at(&mut reg, BOB, 98_820_002, at + 500.0, 10);
    let now = crate::pyutil::filetime_now();
    tick(&mut reg, now);
    let looted = reg.item(near_stuff).unwrap();
    assert_eq!((looted.location_id, looted.owner_id), (mtu, ALICE));
    assert_eq!(
        reg.item(foreign_stuff).unwrap().location_id,
        foreign,
        "no rights"
    );
    assert_eq!(
        reg.item(far_stuff).unwrap().location_id,
        far,
        "out of reach"
    );
    assert!(reg.store().contents_of(near).next().is_none());
    let moved = at + 20_000.0 - reg.combat().wreck(far).unwrap().position[0];
    let want = PULL_MS * (t.cycle as f64 / SEC as f64);
    assert!((moved - want).abs() < 1e-6, "pulled {moved}, wanted {want}");
    // The same instant again: not a new cycle.
    tick(&mut reg, now);
    assert_eq!(
        at + 20_000.0 - reg.combat().wreck(far).unwrap().position[0],
        moved
    );
    // The next cycle pulls it further.
    tick(&mut reg, now + t.cycle);
    assert!(at + 20_000.0 - reg.combat().wreck(far).unwrap().position[0] > moved + 1.0);
}

/// An MTU that has not finished activating does nothing; one with a full
/// cargo takes what fits.
#[test]
fn an_mtu_waits_for_activation_and_fills_up() {
    let mut reg = world();
    let stack = cargo(&mut reg, ALICE, SHIP, TRACTOR, 1);
    let mtu = launched_id(&drop_item(&mut reg, ALICE, stack), stack).expect("launched");
    let at = mtu_x(&reg, mtu);
    let (wreck, stuff) = wreck_at(&mut reg, ALICE, 98_820_001, at + 100.0, 250);
    let now = crate::pyutil::filetime_now();
    tick(&mut reg, now);
    assert_eq!(
        reg.item(stuff).unwrap().location_id,
        wreck,
        "still activating"
    );
    activate(&mut reg, mtu);
    tick(&mut reg, now + 1);
    let taken: i64 = reg
        .store()
        .contents_of(mtu)
        .filter(|i| i.type_id == STUFF)
        .map(|i| i.quantity)
        .sum();
    assert_eq!(taken as f64, MTU_M3, "the 200 m3 it holds");
    let left: i64 = reg.store().contents_of(wreck).map(|i| i.quantity).sum();
    assert_eq!(left, 250 - taken);
}

// ---------------------------------------------------------------------------
// Scooping a deployable
// ---------------------------------------------------------------------------

/// Its owner scoops an empty depot back into the hold; a stranger, or a
/// loaded one, is refused.
#[test]
fn the_owner_scoops_an_empty_depot() {
    let mut reg = world();
    let depot = deployed(&mut reg, DEPOT);
    let scoop = |reg: &mut Registry, who: i64| {
        call(reg, who, "ship", "Scoop", &[PyValue::Int(depot)], &[], None)
    };
    assert!(scoop(&mut reg, BOB).is_err());
    let filler = spawn(
        &mut reg,
        STUFF,
        3,
        Slot::new(ALICE, depot, FLAG_IN_CONTAINER),
    );
    let e = scoop(&mut reg, ALICE).unwrap_err();
    assert!(notify_text(&e).contains("Empty it"), "{e:?}");
    reg.apply_items(&[Edit::Destroy { item_id: filler }])
        .unwrap();
    scoop(&mut reg, ALICE).expect("scooped");
    let c = reg.item(depot).unwrap();
    assert_eq!((c.location_id, c.flag_id), (SHIP, FLAG_CARGO));
    assert!(reg.combat().wreck(depot).is_none());
    assert!(row_of(&reg, depot).is_none());
}

// ---------------------------------------------------------------------------
// Time and restarts
// ---------------------------------------------------------------------------

/// A restart: the records are gone, the rows and items are not. The combat
/// sweep brings the units back and destroys only what has no row.
#[test]
fn a_restart_brings_parked_units_back() {
    let mut reg = world();
    let depot = deployed(&mut reg, DEPOT);
    let secure = container_in_hold(&mut reg, ALICE, SHIP, BOX);
    crate::jettison::jettison_items(&mut reg, Some(ALICE), &[secure]).expect("launched");
    let inside = spawn(
        &mut reg,
        STUFF,
        2,
        Slot::new(ALICE, secure, FLAG_IN_CONTAINER),
    );
    let pos = reg.combat().wreck(secure).unwrap().position;
    // A can with no row is still swept.
    let orphan = spawn(
        &mut reg,
        crate::jettison::JETCAN_TYPE,
        1,
        Slot::new(ALICE, SYSTEM, combat::FLAG_IN_SPACE),
    );
    combat::release_wreck(&mut reg, depot);
    combat::release_wreck(&mut reg, secure);
    assert!(reg.combat().wreck(depot).is_none());
    combat::tick(&mut reg, crate::pyutil::filetime_now());
    assert!(reg.item(orphan).is_none(), "no row, swept");
    assert!(reg.item(depot).is_some() && reg.item(inside).is_some());
    assert_eq!(reg.combat().wreck(secure).unwrap().position, pos);
    assert!(reg.combat().wreck(depot).is_some());
}

/// A row whose item is gone is dropped, and a unit that decays takes its
/// cargo and its row with it.
#[test]
fn a_decayed_unit_leaves_nothing_behind() {
    let mut reg = world();
    let depot = deployed(&mut reg, DEPOT);
    let inside = spawn(
        &mut reg,
        STUFF,
        2,
        Slot::new(ALICE, depot, FLAG_IN_CONTAINER),
    );
    let ghost = 100_829_999;
    remember(
        &mut reg,
        ghost,
        Row {
            system: SYSTEM,
            at: [0.0; 3],
            active_at: 0,
            decay_at: 0,
        },
    );
    restore(&mut reg);
    assert!(row_of(&reg, ghost).is_none(), "an item that is gone");
    let end = reg.combat().wreck(depot).unwrap().expires_at;
    combat::tick(&mut reg, end + 1);
    assert!(reg.item(depot).is_none() && reg.item(inside).is_none());
    assert!(row_of(&reg, depot).is_none(), "the row goes with it");
}

/// The group figures come from the spec and no further: a depot has no
/// tractor, an MTU does, and a group with no spec is nothing.
#[test]
fn only_two_groups_are_deployables_here() {
    let reg = world();
    let c = reg.content();
    assert!(spec(c, GROUP_MOBILE_DEPOT).unwrap().tractor.is_none());
    assert!(spec(c, GROUP_MTU).unwrap().tractor.is_some());
    assert!(!spec(c, GROUP_MTU).unwrap().allow_user_add);
    assert!(spec(c, 12).is_none());
    assert!(spec(c, GROUP_PLAIN_SINGLETON).is_none());
}

/// The figures are the table's: the fixture's invented rows, in blue ticks
/// and metres, and a bundle without the file serves no deployable.
#[test]
fn the_spec_is_read_from_the_table() {
    let reg = world();
    let sp = spec(reg.content(), GROUP_MTU).unwrap();
    assert_eq!(sp.activate, 6 * SEC);
    assert_eq!(sp.decay, 500 * SEC);
    assert_eq!(sp.deploy_at, 900.0);
    let t = sp.tractor.unwrap();
    assert_eq!(
        (t.max_range, t.min_range, t.cycle),
        (100_000.0, 1_000.0, 4 * SEC)
    );
    assert!(spec(&crate::content::Content::default(), GROUP_MTU).is_none());
}

/// A line that is short, not a number or a group twice names itself.
#[test]
fn bad_rows_are_refused() {
    assert!(Specs::parse(&["1\t2\t3"]).is_err());
    assert!(Specs::parse(&["1\tx\t3\t4\t1\t6\t7\t\t\t"]).is_err());
    assert!(Specs::parse(&["1\t1\t3\t4\t1\t6\t7\t8\t\t2"]).is_err());
    let twice = ["1\t1\t3\t4\t1\t6\t7\t\t\t", "1\t1\t3\t4\t1\t6\t7\t\t\t"];
    assert!(Specs::parse(&twice).unwrap_err().contains("twice"));
    assert!(Specs::parse::<&str>(&[]).unwrap().0.is_empty());
}

/// NW-4: a row with an `inhibit_range` is the inhibitor's, not a deployable's
/// (`spec` stays `None` for its group); the ten-column rows still parse, and
/// the placement columns are read when present (invented values throughout).
#[test]
fn an_inhibitor_row_is_not_a_spec() {
    let specs = Specs::parse(&[
        "7\t5\t800\t0\t0\t0\t33000\t\t\t\t30000\t12000\t0.2",
        "8\t1\t3\t4\t1\t6\t7\t\t\t",
        "9\t5\t800\t0\t0\t0\t0\t\t\t\t30000",
    ])
    .expect("every shape parses");
    assert_eq!(specs.0.len(), 1, "only the second is a deployable");
    assert_eq!(specs.1.len(), 2);
    let i = specs.1[0];
    assert_eq!(
        (i.range, i.spacing, i.from_gate, i.max_security),
        (30_000.0, 33_000.0, 12_000.0, Some(0.2))
    );
    let old = specs.1[1];
    assert_eq!((old.from_gate, old.max_security), (0.0, None));
    assert!(
        Specs::parse(&["7\t5\t800\t0\t0\t0\t0\t\t\t\t0"]).is_err(),
        "a zero range is no row"
    );
}
