//! #172 unit tests (NW-2 the anchor path, NW-3 the mobile bubble). **Every
//! id and number is invented** (types 990,172,xxx, items 100,172,xxx, pilots
//! 90,172,xxx, systems 30,172,xxx); the delay, the radius and the hull points
//! are round test numbers. Nothing here asserts an `[A]` value of the
//! module: the delay is read back through [`delay_of`], the position and the
//! lifetime are not asserted at all.

use super::*;
use crate::bubble;
use crate::celestial::Universe;
use crate::item::Change;
use crate::model::{Character, SandboxConfig};
use crate::space_state::{Motion, Presence};

const ALICE: i64 = 90_172_001;
const BOB: i64 = 90_172_002;
const CAROL: i64 = 90_172_003;
const SYSTEM: i64 = 30_172_001;
const HIGH: i64 = 30_172_002;
const SHIP: i64 = 100_172_001;
const BOB_SHIP: i64 = 100_172_002;
const CAROL_SHIP: i64 = 100_172_003;
const CORP: i64 = 98_172_001;
const OTHER_CORP: i64 = 98_172_002;

const DISRUPTOR: i64 = 990_172_001;
const EMPIRE_DISRUPTOR: i64 = 990_172_002;
const HULL: i64 = 990_172_003;
const PLAIN: i64 = 990_172_004;

/// Invented: the delay (ms), the bubble's radius (m), the unit's hull points.
const DELAY_MS: f64 = 120_000.0;
const RADIUS: f64 = 20_000.0;
const HP: f64 = 7_000.0;

const T0: BlueTime = 133_000_000_000_000_000;

fn universe() -> Universe {
    let row = |id: i64, sec: f64| {
        format!("{id}\t19000001\t29000001\t\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture {id}")
    };
    Universe::parse(
        &[row(SYSTEM, -0.5), row(HIGH, 0.9)],
        &["19000001\t\t\t\t\tFixture region".to_owned()],
        &["29000001\t19000001\t\t\t\tFixture constellation".to_owned()],
    )
    .expect("the invented rows parse")
}

fn content() -> crate::content::Content {
    let rows = format!(
        "{}\
         type\t{DISRUPTOR}\t{GROUP_MOBILE_DISRUPTOR}\t22\t1\t1\t10.0\t50.0\t0.0\t1.0\t0\t\t\
         {ATTR_HULL_HP}={HP};{ATTR_DELAY}={DELAY_MS};{}={RADIUS}\tInvented Disruptor\n\
         type\t{EMPIRE_DISRUPTOR}\t{GROUP_MOBILE_DISRUPTOR}\t22\t1\t1\t10.0\t50.0\t0.0\t1.0\t0\t\t\
         {ATTR_HULL_HP}={HP};{ATTR_DELAY}={DELAY_MS};{}={RADIUS};{}=1.0\tInvented Empire Disruptor\n\
         type\t{PLAIN}\t99172\t4\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Plain\n\
         type\t{HULL}\t99173\t6\t1\t1\t10.0\t500.0\t1000000.0\t30.0\t0\t\t9=1000.0\tInvented Hull\n",
        crate::typedata::FIXTURE,
        bubble::ATTR_RANGE,
        bubble::ATTR_RANGE,
        bubble::ATTR_NO_EMPIRE,
    );
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    let mut c = crate::content::Content::fixture();
    c.insert("typedata", crate::typedata::Installed::build(table));
    c.insert("universe", universe());
    c
}

fn make(
    c: &crate::content::Content,
    id: i64,
    type_id: i64,
    owner: i64,
    location: i64,
    flag: i64,
) -> crate::item::Item {
    let t = crate::item::find_in(c, type_id).expect("a test type");
    crate::item::Item {
        item_id: id,
        type_id,
        owner_id: owner,
        location_id: location,
        flag_id: flag,
        quantity: 1,
        group_id: t.group_id,
        category_id: t.category_id,
        singleton: true,
    }
}

/// Alice and Carol (one corporation) and Bob (another) in the null system,
/// Alice at the origin, Bob 5 km away, Carol 10 m away. Nothing in any hold.
fn world() -> Registry {
    let content = std::sync::Arc::new(content());
    let pilot = |id: i64, name: &str, ship: i64, corp: i64| Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        ship_type_id: HULL,
        solar_system_id: Some(SYSTEM),
        corporation_id: corp,
        ..Character::default()
    };
    let mut reg = Registry::new(SandboxConfig {
        characters: vec![
            pilot(ALICE, "Alice", SHIP, CORP),
            pilot(BOB, "Bob", BOB_SHIP, OTHER_CORP),
            pilot(CAROL, "Carol", CAROL_SHIP, CORP),
        ],
        ..SandboxConfig::default()
    })
    .with_content(content.clone());
    let items = vec![
        make(&content, SHIP, HULL, ALICE, SYSTEM, 0),
        make(&content, BOB_SHIP, HULL, BOB, SYSTEM, 0),
        make(&content, CAROL_SHIP, HULL, CAROL, SYSTEM, 0),
    ];
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_173_000,
        ..crate::store::Snapshot::default()
    });
    for (who, ship, x) in [
        (ALICE, SHIP, 0.0),
        (BOB, BOB_SHIP, 5_000.0),
        (CAROL, CAROL_SHIP, 10.0),
    ] {
        reg.roster_mut().arrive(Presence::arriving(
            who,
            ship,
            SYSTEM,
            Motion::arrived([x, 0.0, 0.0], [0.0; 3]),
        ));
    }
    let _ = reg.take_pending();
    reg
}

fn call(
    reg: &mut Registry,
    who: i64,
    service: &str,
    method: &str,
    args: &[PyValue],
) -> Result<PyValue, ServiceError> {
    reg.dispatch(&Request {
        service,
        method,
        args,
        kwargs: &[],
        bind_params: None,
        actor: Some(who),
    })
}

fn in_hold(reg: &mut Registry, who: i64, ship: i64, type_id: i64) -> i64 {
    let changes = reg
        .apply_items(&[Edit::Create {
            type_id,
            quantity: 1,
            to: Slot::new(who, ship, crate::fitting::FLAG_CARGO),
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

/// `ship.Drop`, as the client's "Launch for Self" sends it.
fn drop_item(reg: &mut Registry, who: i64, item: i64) -> Result<i64, String> {
    let answer = call(
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
    )
    .expect("an answer");
    let PyValue::Dict(d) = answer else {
        panic!("a dict")
    };
    let (value, _key) = d.first().expect("one entry");
    let PyValue::List(v) = value else {
        panic!("a list")
    };
    match &v[0] {
        PyValue::Int(id) => Ok(*id),
        _ => Err("refused".to_owned()),
    }
}

/// `dogmaLM.Activate(itemID, effectID)`: two arguments, the second an int.
fn effect(
    reg: &mut Registry,
    who: i64,
    unit: i64,
    effect_id: i64,
) -> Result<PyValue, ServiceError> {
    call(
        reg,
        who,
        "dogmaIM",
        "Activate",
        &[PyValue::Int(unit), PyValue::Int(effect_id)],
    )
}

fn launched(reg: &mut Registry) -> i64 {
    let stack = in_hold(reg, ALICE, SHIP, DISRUPTOR);
    drop_item(reg, ALICE, stack).expect("launched")
}

/// A unit anchored now: dropped, and the clock run past its delay.
fn anchored_unit(reg: &mut Registry) -> i64 {
    let id = launched(reg);
    effect(reg, ALICE, id, EFFECT_ANCHOR_DROP).expect("anchors");
    finish(reg, id);
    id
}

/// Run the unit's change to its end by moving the row's clock into the past.
fn finish(reg: &mut Registry, id: i64) {
    let mut row = row_of(reg, id).expect("a row");
    row.state = match row.state {
        State::Anchoring(_) => State::Anchoring(1),
        State::Unanchoring(_) => State::Unanchoring(1),
        s => s,
    };
    write(reg, id, row);
    tick(reg, T0);
}

fn key_of(e: &ServiceError) -> String {
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

fn move_to(reg: &mut Registry, who: i64, ship: i64, x: f64) {
    let _ = reg.roster_mut().leave(who);
    reg.roster_mut().arrive(Presence::arriving(
        who,
        ship,
        SYSTEM,
        Motion::arrived([x, 0.0, 0.0], [0.0; 3]),
    ));
}

// ---------------------------------------------------------------------------
// NW-2: the anchor path
// ---------------------------------------------------------------------------

#[test]
fn a_launched_unit_is_free() {
    let mut reg = world();
    let id = launched(&mut reg);
    assert_eq!(state_of(&reg, id), Some(State::Free));
    assert!(is_unit(&reg, id));
    // The ball the park gets is a free one, with a physics block.
    let wr = *reg.combat().wreck(id).expect("a record");
    let (ball, _) = combat::wreck_parts(&reg, &wr);
    assert!(ball.physics.is_some(), "free until anchored");
}

#[test]
fn an_int_effect_id_reaches_anchor() {
    let mut reg = world();
    let id = launched(&mut reg);
    // The old shape, a bytes name, is not this arm's: it is a module call.
    let by_name = call(
        &mut reg,
        ALICE,
        "dogmaIM",
        "Activate",
        &[PyValue::Int(id), PyValue::Bytes(b"anchorDrop".to_vec())],
    );
    assert!(by_name.is_err(), "a name is a module's: {by_name:?}");
    assert_eq!(state_of(&reg, id), Some(State::Free));
    effect(&mut reg, ALICE, id, EFFECT_ANCHOR_DROP).expect("the int id anchors");
    assert!(matches!(state_of(&reg, id), Some(State::Anchoring(_))));
}

#[test]
fn anchoring_takes_the_delay() {
    let mut reg = world();
    let id = launched(&mut reg);
    let before = crate::pyutil::filetime_now();
    effect(&mut reg, ALICE, id, EFFECT_ANCHOR_DROP).expect("anchors");
    let delay = delay_of(reg.content(), DISRUPTOR).expect("a delay");
    let Some(State::Anchoring(until)) = state_of(&reg, id) else {
        panic!("anchoring");
    };
    assert!(until >= before + delay && until <= crate::pyutil::filetime_now() + delay);
    // Not yet: a tick short of the end changes nothing.
    tick(&mut reg, until - 1);
    assert!(matches!(state_of(&reg, id), Some(State::Anchoring(_))));
    // Anchoring again, or lifting, is refused while it changes.
    assert!(effect(&mut reg, ALICE, id, EFFECT_ANCHOR_DROP).is_err());
    assert!(effect(&mut reg, ALICE, id, EFFECT_ANCHOR_LIFT).is_err());
    // At the end it is anchored, and the ball is not free any more.
    tick(&mut reg, until);
    assert_eq!(state_of(&reg, id), Some(State::Anchored));
    let wr = *reg.combat().wreck(id).expect("a record");
    let (ball, _) = combat::wreck_parts(&reg, &wr);
    assert!(ball.physics.is_none(), "anchored is not free");
    // And a lift runs the same way back.
    effect(&mut reg, ALICE, id, EFFECT_ANCHOR_LIFT).expect("lifts");
    let Some(State::Unanchoring(until)) = state_of(&reg, id) else {
        panic!("unanchoring");
    };
    tick(&mut reg, until);
    assert_eq!(state_of(&reg, id), Some(State::Free));
}

#[test]
fn a_ball_is_free_through_anchoring_and_not_after() {
    let mut reg = world();
    let id = launched(&mut reg);
    let free = |reg: &Registry| {
        let wr = *reg.combat().wreck(id).expect("a record");
        combat::wreck_parts(reg, &wr).0.physics.is_some()
    };
    assert!(free(&reg));
    effect(&mut reg, ALICE, id, EFFECT_ANCHOR_DROP).expect("anchors");
    // A viewer who gets the ball now (arrival, a restart) sees a free one.
    assert!(free(&reg), "anchoring is still a free ball");
    finish(&mut reg, id);
    assert!(!free(&reg), "anchored is not");
}

#[test]
fn the_removal_skips_a_viewer_who_holds_an_order_on_the_ball() {
    let mut reg = world();
    let id = launched(&mut reg);
    let all = withdraw_to(&reg, id);
    assert!(all.contains(&ALICE) && all.contains(&BOB), "{all:?}");
    reg.locks_mut().add(SHIP, id);
    let some = withdraw_to(&reg, id);
    assert!(!some.contains(&ALICE), "a lock keeps it: {some:?}");
    assert!(some.contains(&BOB));
}

#[test]
fn only_an_unanchored_unit_scoops() {
    let mut reg = world();
    let id = anchored_unit(&mut reg);
    let refused =
        call(&mut reg, ALICE, "ship", "Scoop", &[PyValue::Int(id)]).expect_err("anchored");
    assert_eq!(key_of(&refused), "CanOnlyScoopIfUnanchored");
    assert!(is_unit(&reg, id), "still out there");
    effect(&mut reg, ALICE, id, EFFECT_ANCHOR_LIFT).expect("lifts");
    // Not while it is lifting either.
    assert!(call(&mut reg, ALICE, "ship", "Scoop", &[PyValue::Int(id)]).is_err());
    finish(&mut reg, id);
    call(&mut reg, ALICE, "ship", "Scoop", &[PyValue::Int(id)]).expect("free, so it scoops");
    assert!(!is_unit(&reg, id));
    assert!(row_of(&reg, id).is_none(), "the row goes with it");
    let item = reg.item(id).expect("still an item");
    assert_eq!(
        (item.location_id, item.flag_id),
        (SHIP, crate::fitting::FLAG_CARGO)
    );
}

#[test]
fn a_stranger_cannot_anchor_it() {
    let mut reg = world();
    let id = launched(&mut reg);
    move_to(&mut reg, BOB, BOB_SHIP, 100.0);
    let e = effect(&mut reg, BOB, id, EFFECT_ANCHOR_DROP).expect_err("not his");
    assert!(key_of(&e).contains("CustomNotify"), "{e:?}");
    assert_eq!(state_of(&reg, id), Some(State::Free));
    assert!(call(&mut reg, BOB, "ship", "Scoop", &[PyValue::Int(id)]).is_err());
    // Nor from across the system.
    let far = launched(&mut reg);
    move_to(&mut reg, ALICE, SHIP, 100_000_000.0);
    assert!(
        effect(&mut reg, ALICE, far, EFFECT_ANCHOR_DROP).is_err(),
        "out of reach"
    );
}

#[test]
fn a_thing_that_is_not_a_unit_is_not_anchored() {
    let mut reg = world();
    let plain = in_hold(&mut reg, ALICE, SHIP, PLAIN);
    assert!(effect(&mut reg, ALICE, plain, EFFECT_ANCHOR_DROP).is_err());
    // And a Drop of it is not this module's.
    let list = [PyValue::List(vec![PyValue::Tuple(vec![
        PyValue::Int(plain),
        PyValue::Int(1),
    ])])];
    let req = Request {
        service: "ship",
        method: "Drop",
        args: &list,
        kwargs: &[],
        bind_params: None,
        actor: Some(ALICE),
    };
    assert!(drop_call(&mut reg, &req).is_none());
}

#[test]
fn anchored_state_survives_a_restart() {
    let mut reg = world();
    let id = anchored_unit(&mut reg);
    let at = reg.combat().wreck(id).expect("a record").position;
    // A restart: the records are gone, the rows and the item are not.
    combat::release_wreck(&mut reg, id);
    assert!(!is_unit(&reg, id));
    combat::tick(&mut reg, crate::pyutil::filetime_now());
    assert!(is_unit(&reg, id), "the combat sweep brought it back");
    assert_eq!(state_of(&reg, id), Some(State::Anchored));
    assert_eq!(reg.combat().wreck(id).expect("a record").position, at);
}

#[test]
fn a_row_with_no_unit_is_dropped() {
    let mut reg = world();
    let id = launched(&mut reg);
    combat::remove_wreck(&mut reg, id);
    tick(&mut reg, T0);
    assert!(row_of(&reg, id).is_none());
}

// ---------------------------------------------------------------------------
// NW-3: the mobile bubble
// ---------------------------------------------------------------------------

#[test]
fn a_mobile_bubble_holds_only_once_anchored() {
    let mut reg = world();
    let id = launched(&mut reg);
    // Bob at 5 km is inside the radius all along; only the state differs.
    assert!(
        bubble::inside(&reg, BOB).is_none(),
        "a free unit holds nobody"
    );
    effect(&mut reg, ALICE, id, EFFECT_ANCHOR_DROP).expect("anchors");
    assert!(bubble::inside(&reg, BOB).is_none(), "nor while it anchors");
    finish(&mut reg, id);
    let refusal = bubble::inside(&reg, BOB).expect("held");
    assert_eq!(refusal.user_error, bubble::WARP_DISRUPTED);
    assert!(
        refusal
            .args
            .iter()
            .any(|(v, k)| *k == crate::pyutil::bstr("range") && *v == PyValue::Real(RADIUS)),
        "the range is the type's own: {:?}",
        refusal.args
    );
    // Outside the sphere is free to warp.
    move_to(&mut reg, BOB, BOB_SHIP, RADIUS * 3.0);
    assert!(bubble::inside(&reg, BOB).is_none());
    // And lifting it lets go.
    move_to(&mut reg, BOB, BOB_SHIP, 5_000.0);
    assert!(bubble::inside(&reg, BOB).is_some());
    effect(&mut reg, ALICE, id, EFFECT_ANCHOR_LIFT).expect("lifts");
    assert!(bubble::inside(&reg, BOB).is_none(), "a lift releases it");
}

#[test]
fn it_drags_like_a_probe() {
    let mut reg = world();
    let id = anchored_unit(&mut reg);
    let centre = reg.combat().wreck(id).expect("a record").position;
    let zones = bubble::zones(&reg, SYSTEM);
    assert!(zones.iter().any(|z| matches!(
        z.source,
        bubble::Source::Mobile { ball_id, .. } if ball_id == id
    )));
    // A warp that runs through the sphere to a point past it lands at its edge.
    let from = [centre[0] - RADIUS * 4.0, centre[1], centre[2]];
    let to = [centre[0] + RADIUS * 2.0, centre[1], centre[2]];
    let landed = bubble::drag_in(&zones, BOB_SHIP, from, to).expect("dragged");
    let gap = crate::space_state::distance(landed, centre);
    assert!((gap - RADIUS).abs() < 1.0, "at the edge: {gap}");
}

#[test]
fn it_dies_and_lets_go() {
    let mut reg = world();
    let id = anchored_unit(&mut reg);
    assert!(is_lockable(&reg, id, SYSTEM));
    assert!(!is_lockable(&reg, id, HIGH), "only in its own system");
    let whole = hull_left(&reg, id).expect("a unit");
    assert_eq!(hit(&mut reg, id, 1.0, T0), Some(false));
    assert!(hull_left(&reg, id).expect("alive") < whole, "it loses hull");
    assert!(bubble::inside(&reg, BOB).is_some(), "hurt is still up");
    assert_eq!(hit(&mut reg, id, whole * 2.0, T0), Some(true), "dead");
    assert!(!is_unit(&reg, id));
    assert!(row_of(&reg, id).is_none());
    assert!(reg.item(id).is_none(), "the item is destroyed");
    assert!(
        bubble::inside(&reg, BOB).is_none(),
        "and the bubble with it"
    );
    assert_eq!(hit(&mut reg, id, 1.0, T0), None, "nothing left to hit");
}

#[test]
fn damage_survives_a_restart() {
    let mut reg = world();
    let id = anchored_unit(&mut reg);
    hit(&mut reg, id, 1_000.0, T0);
    let hurt = hull_left(&reg, id).expect("alive");
    assert!(hurt < HP, "it lost hull");
    combat::release_wreck(&mut reg, id);
    combat::tick(&mut reg, crate::pyutil::filetime_now());
    assert_eq!(hull_left(&reg, id), Some(hurt));
}

#[test]
fn a_salvager_cannot_lock_it() {
    let mut reg = world();
    let id = anchored_unit(&mut reg);
    assert!(!crate::salvage::is_lockable(&reg, id, SYSTEM));
    assert!(is_lockable(&reg, id, SYSTEM));
}

#[test]
fn no_launch_in_empire() {
    let mut reg = world();
    // The type that forbids empire space is refused in a high-security
    // system, and launched in null.
    let stack = in_hold(&mut reg, ALICE, SHIP, EMPIRE_DISRUPTOR);
    assert!(drop_item(&mut reg, ALICE, stack).is_ok(), "null is fine");
    let _ = reg.roster_mut().leave(ALICE);
    reg.roster_mut().arrive(Presence::arriving(
        ALICE,
        SHIP,
        HIGH,
        Motion::arrived([0.0; 3], [0.0; 3]),
    ));
    reg.character_mut(ALICE).expect("a pilot").solar_system_id = Some(HIGH);
    let stack = in_hold(&mut reg, ALICE, SHIP, EMPIRE_DISRUPTOR);
    let refused = drop_item(&mut reg, ALICE, stack);
    assert!(refused.is_err(), "high security refuses it: {refused:?}");
    assert_eq!(
        reg.item(stack).map(|i| i.location_id),
        Some(SHIP),
        "it stays in the hold"
    );
    // A type without the flag launches there.
    let stack = in_hold(&mut reg, ALICE, SHIP, DISRUPTOR);
    assert!(drop_item(&mut reg, ALICE, stack).is_ok());
}

#[test]
fn a_kill_releases_the_locks_and_the_guns_on_it() {
    let mut reg = world();
    let id = anchored_unit(&mut reg);
    reg.locks_mut().add(SHIP, id);
    reg.locks_mut().add(BOB_SHIP, id);
    let whole = hull_left(&reg, id).expect("a unit");
    assert_eq!(hit(&mut reg, id, whole * 2.0, T0), Some(true));
    assert!(reg.locks().targeters_of(id).is_empty(), "no lock is left");
}

#[test]
fn a_scoop_releases_the_locks_on_it() {
    let mut reg = world();
    let id = launched(&mut reg);
    reg.locks_mut().add(BOB_SHIP, id);
    call(&mut reg, ALICE, "ship", "Scoop", &[PyValue::Int(id)]).expect("scoops");
    assert!(reg.locks().targeters_of(id).is_empty());
}

#[test]
fn the_menu_reach_works_beyond_a_scoops() {
    let mut reg = world();
    let id = launched(&mut reg);
    // The client offers anchor under 5 km [V]; a scoop stays near.
    move_to(&mut reg, ALICE, SHIP, 4_000.0);
    assert!(
        call(&mut reg, ALICE, "ship", "Scoop", &[PyValue::Int(id)]).is_err(),
        "too far to scoop"
    );
    effect(&mut reg, ALICE, id, EFFECT_ANCHOR_DROP).expect("anchors from 4 km");
}

#[test]
fn a_system_holds_a_bounded_number() {
    let mut reg = world();
    for _ in 0..MAX_PER_SYSTEM {
        launched(&mut reg);
    }
    let stack = in_hold(&mut reg, ALICE, SHIP, DISRUPTOR);
    assert!(drop_item(&mut reg, ALICE, stack).is_err(), "one too many");
}
