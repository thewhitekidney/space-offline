//! The grid's tests (`docs/lanes/grid-research.md` §7). Ids, types and
//! positions are invented (`schema.md` §1.1); no expected value below is one
//! of the module's `[A]` constants -- distances are placed *relative* to
//! [`GRID_ENTER`] and [`GRID_LEAVE`], so the tests hold whatever they become.
//! The fixture is `pub(crate)` for the per-module tests in `asteroid.rs`,
//! `combat.rs` and `jump.rs`.

use super::*;
use crate::destiny::{Park, bootstrap_for, on_ballpark_bind};
use crate::notify::Push;
use crate::space_state::Motion;
use eve_marshal::PyValue;

pub(crate) const SYSTEM: i64 = 30_000_001;
pub(crate) const OTHER_SYSTEM: i64 = 30_000_002;
/// The `Character` default's station, catalogued here at [`STATION_AT`].
pub(crate) const STATION: i64 = 60_012_526;
pub(crate) const PLANET: i64 = 40_000_200;
pub(crate) const BELT_A: i64 = 40_000_201;
pub(crate) const BELT_B: i64 = 40_000_202;
pub(crate) const MOON: i64 = 40_000_203;
pub(crate) const GATE: i64 = 50_000_201;
pub(crate) const FAR_GATE: i64 = 50_000_202;
pub(crate) const OFFICE_TYPE: i64 = 990_000_401;

pub(crate) const ALICE: i64 = 90_000_101;
pub(crate) const BOB: i64 = 90_000_102;
pub(crate) const CAROL: i64 = 90_000_103;
pub(crate) const ALICE_SHIP: i64 = 100_000_101;
pub(crate) const BOB_SHIP: i64 = 100_000_102;
pub(crate) const CAROL_SHIP: i64 = 100_000_103;

pub(crate) const AU: f64 = 149_597_870_700.0;
pub(crate) const STATION_AT: [f64; 3] = [1.0e12, 0.0, 0.0];
/// The two belts are 1 AU from the station and 1 AU from each other: the
/// Tanoo shape of `docs/port-tracker.md` §74.2.
pub(crate) const BELT_A_AT: [f64; 3] = [STATION_AT[0], AU, 0.0];
pub(crate) const BELT_B_AT: [f64; 3] = [STATION_AT[0], 2.0 * AU, 0.0];
pub(crate) const ROCKS_PER_BELT: i64 = 3;

pub(crate) fn plus(a: [f64; 3], d: [f64; 3]) -> [f64; 3] {
    [a[0] + d[0], a[1] + d[1], a[2] + d[2]]
}

/// The rocks of one belt: [`ROCKS_PER_BELT`] of them, a few kilometres
/// apart.
pub(crate) fn belt_rocks(belt: i64, at: [f64; 3], first_id: i64) -> Vec<crate::asteroid::Rock> {
    (0..ROCKS_PER_BELT)
        .map(|k| crate::asteroid::Rock {
            id: first_id + k,
            type_id: 1230,
            belt_id: belt,
            solar_system_id: SYSTEM,
            quantity: 1_000,
            radius: 500.0,
            position: plus(at, [5_000.0 * k as f64, 2_000.0, 0.0]),
            law: crate::asteroid::Law {
                growth: 0.5,
                unit_size: 90.0,
                max_radius: 16_255.0,
            },
        })
        .collect()
}

pub(crate) fn rocks_a() -> Vec<i64> {
    (0..ROCKS_PER_BELT)
        .map(|k| crate::asteroid::MIN_ROCK_ID + k)
        .collect()
}

pub(crate) fn rocks_b() -> Vec<i64> {
    (0..ROCKS_PER_BELT)
        .map(|k| crate::asteroid::MIN_ROCK_ID + 100 + k)
        .collect()
}

/// One system with each global kind the catalogue serves, two belts with
/// rocks, a customs office type, and three pilots based at the station,
/// all docked.
pub(crate) fn world() -> Registry {
    let pilot = |id: i64, name: &str, ship: i64| pilot_at_station(id, name, ship);
    world_of(vec![
        pilot(ALICE, "Alice", ALICE_SHIP),
        pilot(BOB, "Bob", BOB_SHIP),
        pilot(CAROL, "Carol", CAROL_SHIP),
    ])
}

fn pilot_at_station(id: i64, name: &str, ship: i64) -> crate::model::Character {
    crate::model::Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        station_id: Some(STATION),
        solar_system_id: Some(SYSTEM),
        ..crate::model::Character::default()
    }
}

/// [`world`] with the given pilots, all docked at the station.
pub(crate) fn world_of(pilots: Vec<crate::model::Character>) -> Registry {
    let catalogue = crate::celestial::Catalogue::parse(&format!(
        "#kind\tid\ttype_id\tradius\tx\ty\tz\tsystem\tdest\tdest_system\n\
         #system\t{SYSTEM}\t20000001\t10000001\n\
         #system\t{OTHER_SYSTEM}\t20000001\t10000001\n\
         station\t{STATION}\t2502\t0.0\t{sx}\t0.0\t0.0\t{SYSTEM}\t0\t0\n\
         planet\t{PLANET}\t11\t6000000.0\t{sx}\t0.0\t{AU}\t{SYSTEM}\t0\t0\n\
         moon\t{MOON}\t14\t1000000.0\t{sx}\t0.0\t{moon_z}\t{SYSTEM}\t0\t0\n\
         belt\t{BELT_A}\t15\t0.0\t{sx}\t{AU}\t0.0\t{SYSTEM}\t0\t0\n\
         belt\t{BELT_B}\t15\t0.0\t{sx}\t{two_au}\t0.0\t{SYSTEM}\t0\t0\n\
         gate\t{GATE}\t16\t2500.0\t{gx}\t0.0\t0.0\t{SYSTEM}\t{FAR_GATE}\t{OTHER_SYSTEM}\n\
         gate\t{FAR_GATE}\t16\t2500.0\t0.0\t0.0\t0.0\t{OTHER_SYSTEM}\t{GATE}\t{SYSTEM}\n",
        sx = STATION_AT[0],
        moon_z = AU + 5.0e8,
        two_au = 2.0 * AU,
        gx = STATION_AT[0] - AU,
    ));
    let mut rocks = belt_rocks(BELT_A, BELT_A_AT, crate::asteroid::MIN_ROCK_ID);
    rocks.extend(belt_rocks(
        BELT_B,
        BELT_B_AT,
        crate::asteroid::MIN_ROCK_ID + 100,
    ));
    let office = crate::planet::Statics::new().with_customs_type(crate::planet::CustomsType {
        type_id: OFFICE_TYPE,
        capacity: 35_000.0,
        radius: 8_000.0,
        tax_rate: 0.1,
    });
    Registry::new(crate::model::SandboxConfig {
        characters: pilots,
        celestials: catalogue,
        rocks: crate::asteroid::Field::from_rocks(rocks),
        ..crate::model::SandboxConfig::default()
    })
    .with_content(std::sync::Arc::new(crate::planet::content_with(office)))
}

/// One push as it would leave: `(audience, event, args)`.
pub(crate) type Sent = (Vec<i64>, &'static str, Vec<PyValue>);

fn sent(p: &dyn Push) -> Sent {
    let (n, a) = p.to_push();
    (p.audience().unwrap_or_default(), n, a)
}

/// Both queues, through the filter, as they would leave.
pub(crate) fn drain(reg: &mut Registry) -> Vec<Sent> {
    let mut out: Vec<Sent> = reg
        .take_pending()
        .iter()
        .map(|(_, p)| sent(p.as_ref()))
        .collect();
    out.extend(
        reg.take_due(Instant::now() + Duration::from_secs(600))
            .iter()
            .map(|p| sent(p.as_ref())),
    );
    out
}

/// Undock `who` (the ballpark bind) and drain: their park is modelled from
/// here on.
pub(crate) fn undock(reg: &mut Registry, who: i64) -> Vec<Sent> {
    on_ballpark_bind(reg, Some(who));
    drain(reg)
}

/// Put `who`'s ship at `at`, at rest, as of now: a warp that has landed.
pub(crate) fn put(reg: &mut Registry, who: i64, at: [f64; 3]) {
    let p = reg.roster_mut().get_mut(who).expect("in space");
    p.motion = Motion::arrived(at, [0.0; 3]);
    p.since = crate::space_state::now_seconds();
}

/// One reconcile of [`SYSTEM`], drained.
pub(crate) fn settle(reg: &mut Registry) -> Vec<Sent> {
    reconcile_in(reg, SYSTEM);
    drain(reg)
}

pub(crate) fn held(reg: &Registry, who: i64) -> BTreeSet<i64> {
    reg.ext::<Parks>().of(who).cloned().unwrap_or_default()
}

/// `(funcName, args)` of every entry of a `DoDestinyUpdate`'s batch.
pub(crate) fn entries(args: &[PyValue]) -> Vec<(String, Vec<PyValue>)> {
    let Some(PyValue::List(es)) = args.first() else {
        panic!("a batch is a list: {args:?}")
    };
    es.iter()
        .map(|e| {
            let PyValue::Tuple(e) = e else {
                panic!("(stamp, call)")
            };
            let PyValue::Tuple(call) = &e[1] else {
                panic!("(name, args)")
            };
            let (PyValue::Bytes(name), PyValue::Tuple(a)) = (&call[0], &call[1]) else {
                panic!("(str, tuple)")
            };
            (String::from_utf8_lossy(name).into_owned(), a.clone())
        })
        .collect()
}

/// Every Destiny entry `who` was sent, in order.
pub(crate) fn sent_to(out: &[Sent], who: i64) -> Vec<(String, Vec<PyValue>)> {
    out.iter()
        .filter(|(to, n, _)| *n == "DoDestinyUpdate" && to.contains(&who))
        .flat_map(|(_, _, a)| entries(a))
        .collect()
}

/// The ball ids an `AddBalls2` or `SetState` entry creates, read back out of
/// its blob.
pub(crate) fn created(name: &str, args: &[PyValue]) -> Vec<i64> {
    let blob = match (name, args.first()) {
        ("AddBalls2", Some(PyValue::Tuple(pair))) => &pair[0],
        ("SetState", Some(PyValue::Instance { state, .. })) => {
            let PyValue::Dict(kv) = &**state else {
                panic!("a KeyVal's state is a dict")
            };
            &kv.iter()
                .find(|(_, k)| *k == crate::pyutil::bstr("state"))
                .expect("a state blob")
                .0
        }
        _ => panic!("{name} creates no balls: {args:?}"),
    };
    let PyValue::Bytes(bytes) = blob else {
        panic!("a blob is bytes")
    };
    Park::decode(bytes)
        .expect("our own blob decodes")
        .balls
        .iter()
        .map(|b| b.ball_id)
        .collect()
}

/// The ids a `RemoveBalls` entry takes out.
pub(crate) fn removed(args: &[PyValue]) -> Vec<i64> {
    let [PyValue::List(ids)] = args else {
        panic!("RemoveBalls has one positional argument, a list: {args:?}")
    };
    ids.iter()
        .map(|v| match v {
            PyValue::Int(i) => *i,
            other => panic!("an id: {other:?}"),
        })
        .collect()
}

/// Every id `who` was told to add, in `out`.
pub(crate) fn added_to(out: &[Sent], who: i64) -> BTreeSet<i64> {
    sent_to(out, who)
        .iter()
        .filter(|(n, _)| n == "AddBalls2")
        .flat_map(|(n, a)| created(n, a))
        .collect()
}

/// Every id `who` was told to remove, in `out`.
pub(crate) fn removed_from(out: &[Sent], who: i64) -> BTreeSet<i64> {
    sent_to(out, who)
        .iter()
        .filter(|(n, _)| n == "RemoveBalls")
        .flat_map(|(_, a)| removed(a))
        .collect()
}

fn ids(state: &crate::destiny::Bootstrap) -> BTreeSet<i64> {
    state.park.balls.iter().map(|b| b.ball_id).collect()
}

fn dock(reg: &mut Registry, who: i64, ship: i64) {
    // The pilot has flown back to the station: the server refuses a dock
    // from across the system (`dock_policy::refuse_out_of_range`).
    if let Some(at) = reg.ball_position(STATION)
        && let Some(p) = reg.roster_mut().get_mut(who)
    {
        p.motion.position = at;
    }
    reg.dispatch(&crate::service::Request {
        service: "beyonce",
        method: "CmdDock",
        args: &[PyValue::Int(STATION), PyValue::Int(ship)],
        kwargs: &[],
        bind_params: None,
        actor: Some(who),
    })
    .expect("docks");
    // `dispatch::session_change_for` runs this after the answer.
    let _ = reg.dock_change(Some(who));
}

/// **The defect, as reported** (`docs/port-tracker.md` §74.2): undocked at
/// a station with two belts 1 AU away and 1 AU apart, the state holds both
/// belts' *balls* -- warpable from the overview -- and no rock of either.
#[test]
fn an_undock_state_holds_no_rock_from_another_belt() {
    let mut reg = world();
    let state = bootstrap_for(&mut reg, ALICE).expect("a state");
    let balls = ids(&state);
    assert!(balls.contains(&BELT_A) && balls.contains(&BELT_B));
    for rock in rocks_a().iter().chain(rocks_b().iter()) {
        assert!(!balls.contains(rock), "rock {rock} is 1 AU away");
    }
    assert!(balls.contains(&ALICE_SHIP), "the ego");
    assert!(state.every_slim_has_a_ball());
}

/// **Global is global**: wherever in the system a state is built -- at the
/// station, at a belt, in deep space -- it holds the planet, moon, both
/// belts, the gate, the station and the customs office. (The star is not
/// served at all yet, `docs/port-tracker.md` §74.1.)
#[test]
fn every_global_kind_is_in_every_park() {
    let mut reg = world();
    let office = crate::planet::customs_offices_in(
        reg.celestials(),
        crate::planet::statics_in(reg.content()),
        SYSTEM,
    );
    assert_eq!(office.len(), 1, "setup: one office over the one planet");
    let office = office[0].item_id;
    undock(&mut reg, ALICE);
    for at in [STATION_AT, BELT_A_AT, plus(BELT_B_AT, [0.0, 3.0 * AU, AU])] {
        put(&mut reg, ALICE, at);
        let state = bootstrap_for(&mut reg, ALICE).expect("a state");
        let balls = ids(&state);
        for global in [PLANET, MOON, BELT_A, BELT_B, GATE, STATION, office] {
            assert!(balls.contains(&global), "{global} missing at {at:?}");
        }
        assert!(state.every_slim_has_a_ball());
    }
}

/// A pilot at a belt's centre holds every rock of that belt, and none of the
/// other's.
#[test]
fn the_rocks_of_the_belt_you_sit_in_are_in_the_park() {
    let mut reg = world();
    undock(&mut reg, ALICE);
    put(&mut reg, ALICE, BELT_A_AT);
    let balls = ids(&bootstrap_for(&mut reg, ALICE).expect("a state"));
    for rock in rocks_a() {
        assert!(balls.contains(&rock), "rock {rock} of the belt we sit in");
    }
    for rock in rocks_b() {
        assert!(!balls.contains(&rock), "rock {rock} of the other belt");
    }
}

/// **A ship on another grid is not in the park, and d-scan still finds it**:
/// `cone_scan` reads the roster, never the park (research §0.4).
#[test]
fn an_off_grid_ship_is_absent_from_the_park_but_on_dscan() {
    let mut reg = world();
    undock(&mut reg, BOB);
    put(&mut reg, BOB, BELT_A_AT);
    undock(&mut reg, ALICE);
    assert!(
        !held(&reg, ALICE).contains(&BOB_SHIP),
        "Bob is 1 AU away, so not in Alice's state"
    );
    let _ = settle(&mut reg);
    assert!(!held(&reg, BOB).contains(&ALICE_SHIP), "nor she in his");
    let at = reg.roster().get(ALICE).expect("in space").motion.position;
    let to_bob = crate::space_state::unit([
        BELT_A_AT[0] - at[0],
        BELT_A_AT[1] - at[1],
        BELT_A_AT[2] - at[2],
    ])
    .expect("a direction");
    let seen = crate::scan::cone_scan(
        &mut reg,
        Some(ALICE),
        &[
            PyValue::Real(0.5),
            PyValue::Real(2.0 * AU),
            PyValue::Real(to_bob[0]),
            PyValue::Real(to_bob[1]),
            PyValue::Real(to_bob[2]),
        ],
    )
    .expect("a scan");
    assert!(
        format!("{seen:?}").contains(&format!("Int({BOB_SHIP})")),
        "d-scan still reports him: {seen:?}"
    );
}

/// **A warp moves a pilot from one grid to the other**: once they have left,
/// the old grid's rocks and pilots are removed from their park and they from
/// the old grid's parks; once they have landed, the new grid is added both
/// ways.
#[test]
fn a_warp_moves_the_viewer_from_one_grid_to_the_other() {
    let mut reg = world();
    for who in [ALICE, BOB, CAROL] {
        undock(&mut reg, who);
    }
    put(&mut reg, ALICE, BELT_A_AT);
    put(&mut reg, BOB, plus(BELT_A_AT, [10_000.0, 0.0, 0.0]));
    put(&mut reg, CAROL, plus(BELT_B_AT, [10_000.0, 0.0, 0.0]));
    let _ = settle(&mut reg);
    assert!(held(&reg, ALICE).contains(&BOB_SHIP));
    assert!(!held(&reg, ALICE).contains(&CAROL_SHIP));

    // Mid-warp, past the old grid's edge and nowhere near the new one.
    put(&mut reg, ALICE, plus(BELT_A_AT, [0.0, AU / 2.0, 0.0]));
    let out = settle(&mut reg);
    let gone = removed_from(&out, ALICE);
    for id in rocks_a().iter().chain([BOB_SHIP].iter()) {
        assert!(gone.contains(id), "{id} left with the old grid");
    }
    assert!(removed_from(&out, BOB).contains(&ALICE_SHIP));
    assert!(added_to(&out, ALICE).is_empty(), "nothing out here");

    // Landed at the other belt.
    put(&mut reg, ALICE, BELT_B_AT);
    let out = settle(&mut reg);
    let new = added_to(&out, ALICE);
    for id in rocks_b().iter().chain([CAROL_SHIP].iter()) {
        assert!(new.contains(id), "{id} arrives with the new grid");
    }
    assert!(added_to(&out, CAROL).contains(&ALICE_SHIP));
    assert!(sent_to(&out, BOB).is_empty(), "Bob's grid did not change");
    let mine = held(&reg, ALICE);
    assert!(rocks_b().iter().all(|r| mine.contains(r)));
    assert!(rocks_a().iter().all(|r| !mine.contains(r)));
}

/// **Leaving a grid is never an explosion**: every departure is a
/// `RemoveBalls` with exactly one positional argument
/// (`docs/lanes/addballs.md` §3.2), and no departure batch holds a
/// `TerminalPlayDestructionEffect` or a terminal `RemoveBall`.
#[test]
fn a_grid_departure_is_never_a_destruction() {
    let mut reg = world();
    for who in [ALICE, BOB] {
        undock(&mut reg, who);
    }
    put(&mut reg, ALICE, BELT_A_AT);
    put(&mut reg, BOB, BELT_A_AT);
    let _ = settle(&mut reg);
    put(&mut reg, ALICE, BELT_B_AT);
    let out = settle(&mut reg);
    let mut departures = 0;
    for who in [ALICE, BOB] {
        for (name, args) in sent_to(&out, who) {
            assert_ne!(name, "TerminalPlayDestructionEffect");
            assert_ne!(name, "RemoveBall", "the singular is the terminal one");
            if name == "RemoveBalls" {
                assert_eq!(args.len(), 1, "one positional: {args:?}");
                departures += 1;
            }
        }
    }
    assert!(departures >= 2, "both sides saw the other leave");
}

/// **Hysteresis**: a ship between [`GRID_ENTER`] and [`GRID_LEAVE`] is kept
/// if held and not added if not; past [`GRID_LEAVE`] it goes.
#[test]
fn hysteresis_holds_a_ball_between_enter_and_leave() {
    let mut reg = world();
    for who in [ALICE, BOB] {
        undock(&mut reg, who);
    }
    let here = plus(BELT_A_AT, [0.0, 0.0, AU / 3.0]);
    let between = (GRID_ENTER + GRID_LEAVE) / 2.0;
    put(&mut reg, ALICE, here);
    put(&mut reg, BOB, plus(here, [GRID_ENTER / 2.0, 0.0, 0.0]));
    let _ = settle(&mut reg);
    assert!(held(&reg, ALICE).contains(&BOB_SHIP), "setup: on grid");

    put(&mut reg, BOB, plus(here, [between, 0.0, 0.0]));
    let out = settle(&mut reg);
    assert!(
        sent_to(&out, ALICE).is_empty(),
        "held between the radii: kept"
    );
    assert!(held(&reg, ALICE).contains(&BOB_SHIP));

    put(&mut reg, BOB, plus(here, [GRID_LEAVE * 1.01, 0.0, 0.0]));
    let out = settle(&mut reg);
    assert!(
        removed_from(&out, ALICE).contains(&BOB_SHIP),
        "past leave: gone"
    );

    put(&mut reg, BOB, plus(here, [between, 0.0, 0.0]));
    let out = settle(&mut reg);
    assert!(
        sent_to(&out, ALICE).is_empty(),
        "not held between: not added"
    );
    assert!(!held(&reg, ALICE).contains(&BOB_SHIP));

    put(&mut reg, BOB, plus(here, [GRID_ENTER * 0.99, 0.0, 0.0]));
    let out = settle(&mut reg);
    assert!(
        added_to(&out, ALICE).contains(&BOB_SHIP),
        "inside enter: added"
    );
}

/// The reconcile never sends a pilot's own ship to that pilot, wherever the
/// ship goes.
#[test]
fn the_ego_is_never_removed_from_its_own_park() {
    let mut reg = world();
    undock(&mut reg, ALICE);
    for at in [BELT_A_AT, BELT_B_AT, plus(STATION_AT, [0.0, 0.0, 9.0 * AU])] {
        put(&mut reg, ALICE, at);
        let out = settle(&mut reg);
        assert!(!removed_from(&out, ALICE).contains(&ALICE_SHIP));
        assert!(!added_to(&out, ALICE).contains(&ALICE_SHIP));
        assert!(held(&reg, ALICE).contains(&ALICE_SHIP));
    }
}

/// A second reconcile with nothing moved sends nothing.
#[test]
fn reconcile_twice_sends_nothing_the_second_time() {
    let mut reg = world();
    for who in [ALICE, BOB] {
        undock(&mut reg, who);
    }
    put(&mut reg, ALICE, BELT_A_AT);
    put(&mut reg, BOB, BELT_A_AT);
    let first = settle(&mut reg);
    assert!(!first.is_empty(), "setup: the first one moved both");
    assert!(settle(&mut reg).is_empty());
}

/// **The clock runs it**: [`tick`] reconciles on one tick in
/// [`RECONCILE_EVERY`] and on no other.
#[test]
fn the_clock_reconciles_once_every_few_ticks() {
    let mut reg = world();
    for who in [ALICE, BOB] {
        undock(&mut reg, who);
    }
    put(&mut reg, BOB, BELT_A_AT);
    let ft = crate::pyutil::filetime_now();
    let mut ticks_that_sent = 0;
    for _ in 0..RECONCILE_EVERY {
        tick(&mut reg, Instant::now(), ft);
        if !drain(&mut reg).is_empty() {
            ticks_that_sent += 1;
        }
    }
    assert_eq!(ticks_that_sent, 1);
    assert!(!held(&reg, ALICE).contains(&BOB_SHIP));
}

/// **Docking tells only the parks that hold the ship**, and the docker's
/// own park stops being modelled.
#[test]
fn dock_clears_the_park_and_tells_only_holders() {
    let mut reg = world();
    for who in [ALICE, BOB, CAROL] {
        undock(&mut reg, who);
    }
    put(&mut reg, CAROL, BELT_A_AT);
    let _ = settle(&mut reg);
    assert!(held(&reg, BOB).contains(&ALICE_SHIP));
    assert!(!held(&reg, CAROL).contains(&ALICE_SHIP));
    dock(&mut reg, ALICE, ALICE_SHIP);
    let out = drain(&mut reg);
    assert!(
        removed_from(&out, BOB).contains(&ALICE_SHIP),
        "Bob held her"
    );
    assert!(sent_to(&out, CAROL).is_empty(), "Carol never had her");
    assert!(
        reg.ext::<Parks>().of(ALICE).is_none(),
        "not modelled docked"
    );
    assert!(!held(&reg, BOB).contains(&ALICE_SHIP));
}

/// **Warp to an off-grid fleet member still lands**: the target is the
/// roster's (`Registry::character_target`), never the park's.
#[test]
fn warp_to_an_off_grid_fleet_member_still_lands() {
    let mut reg = world();
    for who in [ALICE, BOB] {
        undock(&mut reg, who);
    }
    put(&mut reg, BOB, BELT_B_AT);
    let _ = settle(&mut reg);
    assert!(!held(&reg, ALICE).contains(&BOB_SHIP), "setup: off grid");
    let t = reg.character_target(BOB).expect("a target");
    assert!(crate::space_state::distance(t.position, BELT_B_AT) < 1.0);
}

// ---------------------------------------------------------------------------
// FX-REPLAY: effects already running reach a late arrival
// ---------------------------------------------------------------------------

const LASER_MODULE: i64 = 7_001;
const LASER_PERIOD_MS: i64 = 3_000;

/// `who`'s ship runs a laser on `target`, with `cycles` started.
fn run_laser(reg: &mut Registry, who: i64, ship: i64, module: i64, target: i64, cycles: i64) {
    reg.modules_mut().insert_running(crate::modules::Running {
        character_id: who,
        ship_id: ship,
        module_id: module,
        type_id: 3_001,
        effect: crate::modules::Effect {
            effect_id: 7_101,
            name: "fixtureLaser",
            category: crate::modules::CATEGORY_TARGET,
            guid: "effects.Laser",
            is_offensive: true,
            duration_attribute: 51,
            warp_safe: None,
        },
        target_id: Some(target),
        started: crate::pyutil::filetime_now(),
        period: LASER_PERIOD_MS * crate::modules::MSEC,
        repeat: 1000,
        cycles,
        stop_at: None,
    });
}

/// Alice and Bob fight at belt A (Alice's laser on Bob); Carol is docked.
fn fight() -> Registry {
    let mut reg = world();
    for who in [ALICE, BOB, CAROL] {
        undock(&mut reg, who);
    }
    put(&mut reg, ALICE, BELT_A_AT);
    put(&mut reg, BOB, plus(BELT_A_AT, [0.0, 1_000.0, 0.0]));
    let _ = settle(&mut reg);
    run_laser(&mut reg, ALICE, ALICE_SHIP, LASER_MODULE, BOB_SHIP, 2);
    reg
}

fn fx_entries(out: &[Sent], who: i64) -> Vec<Vec<PyValue>> {
    sent_to(out, who)
        .into_iter()
        .filter(|(n, _)| n == "OnSpecialFX")
        .map(|(_, a)| a)
        .collect()
}

/// (a) A park that gains the shooter and the target is told the laser, in
/// the same batch, after the `AddBalls2`.
#[test]
fn a_late_arrival_is_replayed_the_running_laser_after_the_add() {
    let mut reg = fight();
    put(&mut reg, CAROL, plus(BELT_A_AT, [0.0, 2_000.0, 0.0]));
    let out = settle(&mut reg);
    let names: Vec<String> = sent_to(&out, CAROL).into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["AddBalls2", "OnSpecialFX"], "one batch, add first");
    let batches = out
        .iter()
        .filter(|(to, n, _)| *n == "DoDestinyUpdate" && to.contains(&CAROL))
        .count();
    assert_eq!(batches, 1, "the same batch, so it cannot overtake the add");
    let fx = fx_entries(&out, CAROL);
    let [f] = fx.as_slice() else { panic!("{fx:?}") };
    assert_eq!(f.len(), 14, "fourteen positionals");
    assert_eq!(f[0], PyValue::Int(ALICE_SHIP));
    assert_eq!(f[1], PyValue::Int(LASER_MODULE));
    assert_eq!(f[3], PyValue::Int(BOB_SHIP));
    assert_eq!(f[5], PyValue::Bytes(b"effects.Laser".to_vec()));
    assert_eq!(
        (f[7].clone(), f[8].clone()),
        (PyValue::Int(1), PyValue::Int(1))
    );
    assert_eq!(f[9], PyValue::Int(LASER_PERIOD_MS), "the period, ms");
    assert_eq!(f[10], PyValue::Int(999), "1000 wanted, one done: 999 left");
    assert_eq!(
        (f[11].clone(), f[12].clone()),
        (PyValue::None, PyValue::Int(0))
    );
    // Alice and Bob, who were there all along, are told nothing.
    assert!(fx_entries(&out, ALICE).is_empty() && fx_entries(&out, BOB).is_empty());
}

/// (b) A viewer that gains only the shooter, the target being off its grid,
/// gets no effect.
#[test]
fn an_effect_aimed_off_the_viewers_grid_is_not_replayed() {
    let mut reg = fight();
    // Carol sees Alice but not Bob: Bob is far from her, Alice is not.
    put(&mut reg, BOB, BELT_B_AT);
    let _ = settle(&mut reg);
    put(&mut reg, CAROL, plus(BELT_A_AT, [0.0, 2_000.0, 0.0]));
    let out = settle(&mut reg);
    assert!(added_to(&out, CAROL).contains(&ALICE_SHIP));
    assert!(!added_to(&out, CAROL).contains(&BOB_SHIP), "setup");
    assert!(fx_entries(&out, CAROL).is_empty());
}

/// (c) Nothing is sent twice.
#[test]
fn the_replay_is_sent_once() {
    let mut reg = fight();
    put(&mut reg, CAROL, plus(BELT_A_AT, [0.0, 2_000.0, 0.0]));
    assert_eq!(fx_entries(&settle(&mut reg), CAROL).len(), 1);
    assert!(settle(&mut reg).is_empty());
}

/// A viewer that already holds the shooter and gains the target is told too.
#[test]
fn gaining_only_the_target_replays_the_effect() {
    let mut reg = fight();
    put(&mut reg, BOB, BELT_B_AT);
    run_laser(&mut reg, ALICE, ALICE_SHIP, LASER_MODULE, BOB_SHIP, 2);
    let _ = settle(&mut reg);
    put(&mut reg, CAROL, plus(BELT_A_AT, [0.0, 2_000.0, 0.0]));
    let _ = settle(&mut reg);
    put(&mut reg, BOB, plus(BELT_A_AT, [0.0, 1_000.0, 0.0]));
    let out = settle(&mut reg);
    assert_eq!(
        fx_entries(&out, CAROL).len(),
        1,
        "Carol held Alice, gained Bob"
    );
}

/// A stopping effect and a missile launcher are not replayed.
#[test]
fn a_stopping_effect_is_not_replayed() {
    let mut reg = fight();
    let mut r = *reg.modules().running(LASER_MODULE).expect("running");
    r.stop_at = Some(crate::pyutil::filetime_now() - 1);
    reg.modules_mut().insert_running(r);
    put(&mut reg, CAROL, plus(BELT_A_AT, [0.0, 2_000.0, 0.0]));
    assert!(fx_entries(&settle(&mut reg), CAROL).is_empty());
}

fn state_attr(args: &[PyValue], key: &[u8]) -> PyValue {
    let PyValue::Instance { state, .. } = &args[0] else {
        panic!("a KeyVal: {args:?}")
    };
    let PyValue::Dict(kv) = &**state else {
        panic!("a dict")
    };
    kv.iter()
        .find(|(_, k)| {
            *k == PyValue::Bytes(key.to_vec())
                || *k == crate::pyutil::bstr(&String::from_utf8_lossy(key))
        })
        .map(|(v, _)| v.clone())
        .unwrap_or_else(|| panic!("no {key:?}"))
}

fn set_state_args(b: &crate::destiny::Bootstrap) -> Vec<PyValue> {
    let (_, args) = b.to_push();
    let mut es = entries(&args);
    assert_eq!(es.len(), 1);
    es.remove(0).1
}

/// (d) A `SetState` carries one 14-tuple per running effect whose two ends
/// it holds, and none for an effect aimed outside it.
#[test]
fn set_state_effect_states_are_fourteen_tuples() {
    let mut reg = fight();
    undock(&mut reg, CAROL);
    put(&mut reg, CAROL, plus(BELT_A_AT, [0.0, 2_000.0, 0.0]));
    let b = bootstrap_for(&mut reg, CAROL).expect("in space");
    let args = set_state_args(&b);
    let PyValue::List(states) = state_attr(&args, b"effectStates") else {
        panic!("a list")
    };
    let [PyValue::Tuple(t)] = states.as_slice() else {
        panic!("{states:?}")
    };
    assert!(t.len() >= 9, "OnSpecialFX(*t) needs nine at least");
    assert_eq!(t.len(), 14);
    assert_eq!(t[5], PyValue::Bytes(b"effects.Laser".to_vec()));
    assert_eq!(
        (t[7].clone(), t[8].clone()),
        (PyValue::Int(1), PyValue::Int(1))
    );
    assert_eq!(t[11], PyValue::None);
    // Bob leaves the grid: the effect is aimed outside the state.
    put(&mut reg, BOB, BELT_B_AT);
    let b = bootstrap_for(&mut reg, CAROL).expect("in space");
    let args = set_state_args(&b);
    assert_eq!(state_attr(&args, b"effectStates"), PyValue::List(vec![]));
}

/// (e) The ego's `dbuffState` is the field's value, and an empty list when
/// nothing applies.
#[test]
fn set_state_carries_the_dbuff_state() {
    let mut reg = fight();
    undock(&mut reg, CAROL);
    put(&mut reg, CAROL, plus(BELT_A_AT, [0.0, 2_000.0, 0.0]));
    let b = bootstrap_for(&mut reg, CAROL).expect("in space");
    assert_eq!(
        state_attr(&set_state_args(&b), b"dbuffState"),
        PyValue::List(vec![])
    );
    let mut b = b;
    let one = PyValue::List(vec![PyValue::Tuple(vec![
        PyValue::Int(11),
        PyValue::Tuple(vec![PyValue::Real(5.0), PyValue::Int(77)]),
    ])]);
    b.dbuff = Some(one.clone());
    assert_eq!(state_attr(&set_state_args(&b), b"dbuffState"), one);
}

/// A `SetState` resets the park model, so the reconcile after it does not
/// add (or replay) what the state held.
#[test]
fn a_state_is_not_replayed_by_the_next_reconcile() {
    let mut reg = fight();
    put(&mut reg, CAROL, plus(BELT_A_AT, [0.0, 2_000.0, 0.0]));
    let state = bootstrap_for(&mut reg, CAROL).expect("in space");
    assert!(!state.fx.is_empty(), "setup: the state carries the laser");
    let update = crate::destiny::mid_session_state(&mut reg, CAROL, state).expect("a stamp");
    reg.push(update);
    let _ = drain(&mut reg);
    let out = settle(&mut reg);
    assert!(sent_to(&out, CAROL).is_empty(), "{out:?}");
}

/// PA-7's twin: the same crowd with running modules reconciles to the same
/// pushes serially and on the kit.
#[test]
fn the_parallel_reconcile_matches_the_serial_with_running_effects() {
    let n = 40;
    let mut a = crowd(n);
    let mut b = crowd(n);
    undock_all(&mut a, n);
    undock_all(&mut b, n);
    let ids = crowd_ids(n);
    // The second half starts a belt away; the first half's lasers are on it.
    for reg in [&mut a, &mut b] {
        for (k, who) in ids.iter().enumerate() {
            let belt = if k < 20 { BELT_A_AT } else { BELT_B_AT };
            put(reg, *who, plus(belt, [(k as f64) * 300.0, 0.0, 0.0]));
        }
        let _ = settle(reg);
        for k in 0..20_i64 {
            run_laser(
                reg,
                ids[k as usize],
                ALICE_SHIP + 100 + k,
                8_000 + k,
                ALICE_SHIP + 120 + k,
                3,
            );
        }
        // The wave warps in.
        for (k, who) in ids.iter().enumerate().skip(20) {
            put(reg, *who, plus(BELT_A_AT, [(k as f64) * 300.0, 0.0, 0.0]));
        }
    }
    let mins = ReconcileMins {
        diff_pairs: 0,
        added: 0,
    };
    reconcile_in_with(&mut a, SYSTEM, 1, mins);
    reconcile_in_with(&mut b, SYSTEM, 8, mins);
    let far = Instant::now() + Duration::from_secs(600);
    let (oa, ob) = (a.take_due(far), b.take_due(far));
    let fx_count = |o: &[Box<dyn Push>]| {
        o.iter()
            .map(|p| p.to_push().1)
            .filter(|args| {
                matches!(args.first(), Some(PyValue::List(_)))
                    && entries(args).iter().any(|(n, _)| n == "OnSpecialFX")
            })
            .count()
    };
    assert!(fx_count(&oa) > 0, "setup: effects were replayed");
    assert_eq!(dump(&ob), dump(&oa));
}

/// Cost at 500 pilots in one grid, 250 of them running a module, a wave of
/// 100 warping in. `#[ignore]`d: a timing.
#[test]
#[ignore = "a timing: run it in release with --ignored --nocapture"]
fn replay_bench() {
    let n = 500;
    for lasers in [0_i64, 250] {
        let mut reg = crowd(n);
        undock_all(&mut reg, n);
        let ids = crowd_ids(n);
        for (k, who) in ids.iter().enumerate() {
            let at = if k < 400 { BELT_A_AT } else { BELT_B_AT };
            let off = [(k % 50) as f64 * 200.0, (k / 50) as f64 * 200.0, 0.0];
            put(&mut reg, *who, plus(at, off));
        }
        let _ = settle(&mut reg);
        for k in 0..lasers {
            let tgt = ALICE_SHIP + 100 + (k + 1) % 400;
            run_laser(
                &mut reg,
                ids[k as usize],
                ALICE_SHIP + 100 + k,
                9_000 + k,
                tgt,
                3,
            );
        }
        for who in &ids[400..] {
            put(&mut reg, *who, plus(BELT_A_AT, [0.0, 3_000.0, 0.0]));
        }
        let t = Instant::now();
        reconcile_in(&mut reg, SYSTEM);
        let took = t.elapsed();
        let out = drain(&mut reg);
        let fx: usize = ids[400..].iter().map(|w| fx_entries(&out, *w).len()).sum();
        println!("100-pilot wave, {lasers} running modules: reconcile {took:?}, {fx} replayed");
    }
}

/// **Every Destiny push declares a grid role.** The list of the pushes whose
/// event is `DoDestinyUpdate`, the `restamp` test's pattern
/// (`destiny::push::tests::every_stamped_push_can_be_restamped`); the drain's
/// debug assertion is the other half, catching one that is not listed here
/// the first time any test drains it.
#[test]
fn every_destiny_push_declares_its_grid_role() {
    let ball = crate::destiny::wreck_ball(4242, [0.0; 3]);
    let slim = crate::destiny::Slim {
        item_id: 4242,
        type_id: 670,
        owner_id: 1,
        char_id: None,
        corp_id: None,
        response_thresholds: None,
    };
    let wreck = crate::destiny::WreckSlim {
        item_id: 4242,
        type_id: 26550,
        owner_id: 1,
        corp_id: 2,
        launcher_id: 4243,
        is_empty: true,
        fleet_id: None,
        abandoned: false,
    };
    let park = || Park {
        current_time: 1,
        balls: vec![ball],
    };
    let pushes: Vec<Box<dyn Push>> = vec![
        Box::new(crate::destiny::StateUpdate {
            to: vec![1],
            state: crate::destiny::Bootstrap::new(1, park(), 4242, vec![slim], Vec::new()),
        }),
        Box::new(
            crate::destiny::ActionUpdate::new(
                vec![1],
                1,
                vec![crate::destiny::Action::Stop { ball: 4242 }],
            )
            .expect("a batch"),
        ),
        Box::new(
            crate::destiny::BallsAdded::new(vec![1], 1, park(), vec![(slim, None)])
                .expect("an add"),
        ),
        Box::new(crate::destiny::BallsRemoved::new(vec![1], 1, vec![4242]).expect("a removal")),
        Box::new(
            crate::destiny::ShipHandoff::new(
                vec![1],
                1,
                vec![crate::destiny::HandoffEntry::Act(
                    crate::destiny::Action::Stop { ball: 4242 },
                )],
            )
            .expect("a handoff"),
        ),
        Box::new(
            crate::destiny::SpecialFx::new(vec![1], 1, 4242, crate::destiny::FX_WARPING, true)
                .expect("an effect"),
        ),
        Box::new(
            crate::destiny::DamageStateChange::new(
                vec![1],
                1,
                4242,
                crate::destiny::DamageState::FULL,
            )
            .expect("bars"),
        ),
        Box::new(
            crate::destiny::Kill::new(vec![1], 1, 4243, Some((ball, wreck)), None).expect("a kill"),
        ),
        Box::new(crate::destiny::SlimItemChange::new(vec![1], 1, wreck).expect("a slim change")),
        Box::new(crate::jump::JumpFx {
            to: vec![1],
            stamp: 1,
            ship: 4242,
            gate: 4243,
            dest_system: 1,
        }),
        Box::new(crate::mining::RockRadius::new(vec![1], 1, 4242, 100.0).expect("a radius")),
        Box::new(
            crate::destiny::MissileLaunched::new(
                vec![1],
                1,
                crate::destiny::MissileBall {
                    id: 4245,
                    owner: 4242,
                    target: 4243,
                    from: [0.0; 3],
                    toward: [1.0, 0.0, 0.0],
                    radius: 1.0,
                    target_radius: 1.0,
                    mass: 1.0,
                    max_velocity: 1.0,
                    agility: 1.0,
                    corporation_id: -1,
                    alliance_id: -1,
                },
                crate::destiny::MissileSlim::npc(4245, 670, 1, 4242),
            )
            .expect("a launch"),
        ),
    ];
    for p in &pushes {
        let (event, _) = p.to_push();
        assert_eq!(event, "DoDestinyUpdate", "{p:?}");
        assert_ne!(p.grid(), GridRole::None, "{p:?} would reach every pilot");
    }
}

/// **Off grid means not in the park, and the bit that would say so is
/// `0x02`**: the client's own writer, with `isGlobal` cleared by `AddBall`'s
/// sixth argument and by the attribute, wrote `FLAGS_FREE_BALL` less exactly
/// [`FLAG_IS_GLOBAL`] (`findings/destiny-ref/`) **[V]**. Phase 2 sets the
/// flags; this pins which bit it will clear.
#[test]
fn the_global_bit_is_0x02() {
    const BASE: &[u8] = include_bytes!("../../../../../findings/destiny-ref/03-base-one-ball.bin");
    const ARG: &[u8] = include_bytes!("../../../../../findings/destiny-ref/11-arg-a6_i.bin");
    const ATTR: &[u8] = include_bytes!("../../../../../findings/destiny-ref/28-attr-isGlobal.bin");
    let flags = |bytes: &[u8]| Park::decode(bytes).expect("the client wrote it").balls[0].flags;
    assert_eq!(flags(BASE), crate::destiny::FLAGS_FREE_BALL);
    for cleared in [ARG, ATTR] {
        assert_eq!(
            flags(cleared),
            crate::destiny::FLAGS_FREE_BALL & !FLAG_IS_GLOBAL
        );
    }
}

/// What a client's park would be, replayed from the wire alone: `SetState`
/// replaces it, `AddBalls2` adds, `RemoveBalls` / `RemoveBall` remove. The
/// filter's own model is not consulted -- this is the client's view.
const PILOTS: [i64; 3] = [ALICE, BOB, CAROL];

#[derive(Default)]
struct Replay(BTreeMap<i64, BTreeSet<i64>>);

impl Replay {
    /// Apply one drain; panic on an entry that names a ball its viewer does
    /// not hold, removes one it does not hold, or adds one it already does.
    fn apply(&mut self, out: &[Sent], step: usize) {
        for (to, event, args) in out {
            if *event != "DoDestinyUpdate" {
                continue;
            }
            for who in to {
                for (name, a) in entries(args) {
                    if name == "SetState" {
                        self.0
                            .insert(*who, created(&name, &a).into_iter().collect());
                        continue;
                    }
                    // An NPC in an audience (`others_in_system` counts the
                    // roster) has no session and is sent nothing.
                    if !PILOTS.contains(who) {
                        continue;
                    }
                    let Some(park) = self.0.get_mut(who) else {
                        panic!("step {step}: {name} to {who}, who has no park")
                    };
                    match name.as_str() {
                        "AddBalls2" => {
                            for id in created(&name, &a) {
                                assert!(park.insert(id), "step {step}: {who} sent {id} twice");
                            }
                        }
                        "RemoveBalls" => {
                            for id in removed(&a) {
                                assert!(
                                    park.remove(&id),
                                    "step {step}: {who} told to remove {id} it never had"
                                );
                            }
                        }
                        _ => {
                            let Some(PyValue::Int(subject)) = a.first() else {
                                panic!("step {step}: {name} names no ball: {a:?}")
                            };
                            assert!(
                                park.contains(subject),
                                "step {step}: {name} to {who} names {subject}, not in its park"
                            );
                            if name == "RemoveBall" {
                                park.remove(subject);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// **No Destiny entry names a ball outside its viewer's park** -- the
/// property the filter exists for. A seeded random walk of undock, warp,
/// rats deployed, NPC moves, damage, kill, mining, dock, `/tp` and the
/// reconcile; every push drained after each step and replayed into a
/// per-viewer park built from the wire alone ([`Replay`]). Every entry's
/// subject must be held, every removal must have been held, and nothing is
/// added twice.
#[test]
fn no_destiny_entry_names_a_ball_outside_its_viewers_park() {
    let mut reg = world();
    let mut walk = crate::asteroid::Mix(0x6772_6964);
    let mut below = |n: usize| (walk.next() % n as u64) as usize;
    let mut replay = Replay::default();
    let pilots = [(ALICE, ALICE_SHIP), (BOB, BOB_SHIP), (CAROL, CAROL_SHIP)];
    // Every pilot in space: the audience `combat` and `mining` build.
    let everyone = |reg: &Registry| -> Vec<i64> {
        pilots
            .iter()
            .map(|(c, _)| *c)
            .filter(|c| reg.roster().is_in_space(*c))
            .collect()
    };
    let spots = [
        STATION_AT,
        BELT_A_AT,
        BELT_B_AT,
        plus(BELT_A_AT, [(GRID_ENTER + GRID_LEAVE) / 2.0, 0.0, 0.0]),
        plus(BELT_A_AT, [GRID_ENTER / 3.0, 0.0, 0.0]),
    ];
    let rat = crate::npc::FIXTURE_RAT.type_id.to_string();
    let tp = crate::slash::COMMANDS
        .iter()
        .find(|c| c.name == "tp")
        .expect("/tp")
        .run;
    let mut most = 0;
    for step in 0..400 {
        let (who, ship) = pilots[below(pilots.len())];
        let in_space = reg.roster().is_in_space(who);
        match below(9) {
            // Undock, or a desync recovery's fresh state.
            0 => {
                if !in_space {
                    on_ballpark_bind(&mut reg, Some(who));
                } else if let Some(state) = bootstrap_for(&mut reg, who) {
                    reg.push(crate::destiny::StateUpdate {
                        to: vec![who],
                        state,
                    });
                }
            }
            // A warp lands somewhere.
            1 | 2 if in_space => put(&mut reg, who, spots[below(spots.len())]),
            // Rats, where the pilot is; announced to the whole system.
            3 if in_space => {
                let _ = crate::npc::entity_slash(&mut reg, Some(who), &["deploy", "2", &rat]);
            }
            // Every NPC acts, and the whole system is the audience.
            4 => crate::npc::tick(&mut reg, crate::pyutil::filetime_now()),
            // A ship's bars change, or a rat dies where it is: both
            // addressed to the whole system, as `combat` addresses them.
            5 if in_space => {
                if below(2) == 0 {
                    if let Some(p) = crate::destiny::DamageStateChange::new(
                        everyone(&reg),
                        0,
                        ship,
                        crate::destiny::DamageState::FULL,
                    ) {
                        reg.schedule(Duration::ZERO, p);
                    }
                } else if let Some(npc) = reg
                    .roster()
                    .in_system(SYSTEM)
                    .into_iter()
                    .find(|p| reg.npcs().get(p.character_id).is_some())
                {
                    reg.roster_mut().leave(npc.character_id);
                    let wreck = crate::destiny::WreckSlim {
                        item_id: 900_000_000 + step as i64,
                        type_id: 26550,
                        owner_id: 1,
                        corp_id: 1,
                        launcher_id: npc.ship_id,
                        is_empty: true,
                        fleet_id: None,
                        abandoned: false,
                    };
                    let ball = crate::destiny::wreck_ball(wreck.item_id, npc.motion.position);
                    if let Some(k) = crate::destiny::Kill::new(
                        everyone(&reg),
                        0,
                        npc.ship_id,
                        Some((ball, wreck)),
                        None,
                    ) {
                        reg.schedule(Duration::ZERO, k);
                    }
                }
            }
            // A rock shrinks, or is mined out, told to the whole system.
            6 => {
                let rock = rocks_a()[below(rocks_a().len())];
                if below(2) == 0 {
                    if let Some(p) = crate::mining::RockRadius::new(everyone(&reg), 0, rock, 400.0)
                    {
                        reg.schedule(Duration::ZERO, p);
                    }
                } else if let Some(p) =
                    crate::destiny::BallsRemoved::new(everyone(&reg), 0, vec![rock])
                {
                    reg.schedule(Duration::ZERO, p);
                }
            }
            // Dock, or a teleport.
            7 if in_space => {
                if below(2) == 0 {
                    dock(&mut reg, who, ship);
                    replay.0.remove(&who);
                } else {
                    let _ = tp(&mut reg, Some(who), &["5000"]);
                }
            }
            // The clock's reconcile.
            _ => reconcile_in(&mut reg, SYSTEM),
        }
        let out = drain(&mut reg);
        replay.apply(&out, step);
        most = most.max(replay.0.values().map(BTreeSet::len).max().unwrap_or(0));
    }
    assert!(most > 10, "setup: the walk put pilots among things");
}

/// **A pilot on belt A is never put at belt B by a stale warp settle**
/// (GRID-VIS). Bob warps to belt B and, before that warp lands, back to belt
/// A. The first warp's late `SetBallPosition` must not reach Alice: it would
/// move Bob's ball, which her park still holds, to the other belt. Only
/// positions near belt A may be sent for Bob, and Alice never gets a ball of
/// belt B.
#[test]
fn a_stale_warp_settle_never_puts_a_held_ship_on_the_other_belt() {
    let mut reg = world();
    for who in [ALICE, BOB] {
        undock(&mut reg, who);
    }
    put(&mut reg, ALICE, BELT_A_AT);
    // 200 km out, so the warp back to belt A clears the 150 km minimum
    // (WARP-MIN) and is still on Alice's grid.
    put(&mut reg, BOB, plus(BELT_A_AT, [200_000.0, 0.0, 0.0]));
    let _ = settle(&mut reg);
    assert!(held(&reg, ALICE).contains(&BOB_SHIP));
    for belt in [BELT_B, BELT_A] {
        let _ = reg
            .dispatch(&crate::service::Request {
                service: "beyonce",
                method: "CmdWarpToStuff",
                args: &[PyValue::Bytes(b"item".to_vec()), PyValue::Int(belt)],
                kwargs: &[],
                bind_params: None,
                actor: Some(BOB),
            })
            .expect("answered");
    }
    let out = drain(&mut reg);
    for (name, args) in sent_to(&out, ALICE) {
        if name != "SetBallPosition" {
            continue;
        }
        let [
            PyValue::Int(ball),
            PyValue::Real(x),
            PyValue::Real(y),
            PyValue::Real(z),
        ] = args.as_slice()
        else {
            panic!("SetBallPosition args: {args:?}");
        };
        if *ball == BOB_SHIP {
            assert!(
                crate::space_state::distance([*x, *y, *z], BELT_A_AT) < AU / 2.0,
                "Bob was snapped to {x} {y} {z}, not belt A"
            );
        }
    }
    assert!(
        held(&reg, ALICE).iter().all(|b| !rocks_b().contains(b)),
        "belt B's rocks never reach Alice"
    );
}

/// The mode byte of Bob's ball inside the first `AddBalls2` of `out` that
/// carries it, for `who`'s pushes.
fn mode_of_added(out: &[Sent], who: i64, ball: i64) -> Option<crate::destiny::Mode> {
    for (name, args) in sent_to(out, who) {
        if name != "AddBalls2" {
            continue;
        }
        let Some(PyValue::Tuple(pair)) = args.first() else {
            continue;
        };
        let PyValue::Bytes(bytes) = &pair[0] else {
            continue;
        };
        let park = Park::decode(bytes).expect("our own blob decodes");
        if let Some(b) = park.balls.iter().find(|b| b.ball_id == ball) {
            return Some(b.mode);
        }
    }
    None
}

/// **An observer at the mouth sees the undock** (UNDOCK-VIS): Bob binds his
/// ballpark; Alice, already in space beside the station, is sent his ball at
/// the mouth in Stop mode -- before any `WarpTo` -- and then sees the warp as
/// a `WarpTo` followed by the removal when he leaves her grid.
#[test]
fn an_observer_at_the_mouth_sees_the_undock_then_the_warp_then_the_removal() {
    let mut reg = world();
    undock(&mut reg, ALICE);
    // Alice sits at her own berth; Bob undocks beside her.
    let out = undock(&mut reg, BOB);
    assert!(
        added_to(&out, ALICE).contains(&BOB_SHIP),
        "Alice is told about Bob's ball at the bind: {:?}",
        sent_to(&out, ALICE)
            .iter()
            .map(|(n, _)| n)
            .collect::<Vec<_>>()
    );
    assert!(
        !sent_to(&out, ALICE).iter().any(|(n, _)| n == "WarpTo"),
        "no warp before the add"
    );
    assert!(
        matches!(
            mode_of_added(&out, ALICE, BOB_SHIP),
            Some(crate::destiny::Mode::Stop)
        ),
        "added in Stop mode, coming out of the mouth"
    );
    assert!(held(&reg, ALICE).contains(&BOB_SHIP));

    // Bob warps to belt A; Alice sees the WarpTo, then the removal.
    let _ = reg
        .dispatch(&crate::service::Request {
            service: "beyonce",
            method: "CmdWarpToStuff",
            args: &[PyValue::Bytes(b"item".to_vec()), PyValue::Int(BELT_A)],
            kwargs: &[],
            bind_params: None,
            actor: Some(BOB),
        })
        .expect("answered");
    let mut seen: Vec<String> = sent_to(&drain(&mut reg), ALICE)
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    // Bob mid-warp, past Alice's grid edge.
    put(&mut reg, BOB, plus(BELT_A_AT, [0.0, AU / 2.0, 0.0]));
    reconcile_in(&mut reg, SYSTEM);
    seen.extend(sent_to(&drain(&mut reg), ALICE).into_iter().map(|(n, _)| n));
    let warp = seen.iter().position(|n| n == "WarpTo").expect("a WarpTo");
    let gone = seen
        .iter()
        .position(|n| n == "RemoveBalls")
        .expect("a RemoveBalls");
    assert!(warp < gone, "WarpTo, then the removal: {seen:?}");
}

/// **A warper leaves the observer's park when its true warp position passes
/// the grid edge** (LOCK-WARP, reverting BOT-REDOCK's hold): not before, while
/// it still accelerates away on the grid, and not only at landing, which kept
/// a ship bound for another belt on the overview for the whole warp.
#[test]
fn a_warping_ship_is_removed_when_its_warp_position_passes_the_grid_edge() {
    let mut reg = world();
    undock(&mut reg, ALICE);
    let _ = undock(&mut reg, BOB);
    assert!(held(&reg, ALICE).contains(&BOB_SHIP));
    let _ = reg
        .dispatch(&crate::service::Request {
            service: "beyonce",
            method: "CmdWarpToStuff",
            args: &[PyValue::Bytes(b"item".to_vec()), PyValue::Int(BELT_A)],
            kwargs: &[],
            bind_params: None,
            actor: Some(BOB),
        })
        .expect("answered");
    let _ = drain(&mut reg);
    let start = {
        let p = reg.roster().get(BOB).expect("in space");
        assert!(matches!(p.motion.mode, crate::destiny::Mode::Warp { .. }));
        p.motion.position
    };
    // In warp but still well inside the grid: kept.
    let alice_at = reg.roster().get(ALICE).expect("in space").motion.position;
    let inside = {
        let d = super::GRID_ENTER / 2.0;
        [alice_at[0] + d, alice_at[1], alice_at[2]]
    };
    reg.roster_mut().get_mut(BOB).unwrap().motion.position = inside;
    for _ in 0..3 {
        let out = settle(&mut reg);
        assert!(
            !removed_from(&out, ALICE).contains(&BOB_SHIP),
            "no removal while the warp position is on the grid"
        );
    }
    assert!(held(&reg, ALICE).contains(&BOB_SHIP));
    // Past the edge, still mid-warp: removed now, not at landing.
    reg.roster_mut().get_mut(BOB).unwrap().motion.position =
        plus(start, [0.0, super::GRID_LEAVE * 2.0 + AU / 2.0, 0.0]);
    assert!(matches!(
        reg.roster().get(BOB).unwrap().motion.mode,
        crate::destiny::Mode::Warp { .. }
    ));
    let out = settle(&mut reg);
    assert!(
        removed_from(&out, ALICE).contains(&BOB_SHIP),
        "removed once his warp position is off the grid"
    );
    assert!(!held(&reg, ALICE).contains(&BOB_SHIP));
}

/// **A second undock is seen as an undock too** (UNDOCK-VIS): Bob undocks,
/// docks, and undocks again while Alice watches; the second bind adds his
/// ball in Stop mode all the same.
#[test]
fn an_observer_sees_the_second_undock_in_stop_mode() {
    let mut reg = world();
    undock(&mut reg, ALICE);
    undock(&mut reg, BOB);
    dock(&mut reg, BOB, BOB_SHIP);
    let out = drain(&mut reg);
    assert!(removed_from(&out, ALICE).contains(&BOB_SHIP));
    let out = undock(&mut reg, BOB);
    assert!(added_to(&out, ALICE).contains(&BOB_SHIP), "{out:?}");
    assert!(matches!(
        mode_of_added(&out, ALICE, BOB_SHIP),
        Some(crate::destiny::Mode::Stop)
    ));
}

/// PERF-HOLDS: the indexed diff is the plain loop's answer, in the same
/// order, over many pseudo-random parks -- balls placed around both edges,
/// held and not, landed and not, ids the park holds that are not local, a
/// ball at the ego's own id.
#[test]
fn the_indexed_park_diff_matches_the_scan() {
    let mut x: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for _ in 0..200 {
        let n = (next() % 60) as usize;
        let at = [0.0, 0.0, 0.0];
        let local: Vec<(i64, [f64; 3])> = (0..n)
            .map(|i| {
                // Radii straddle both edges, some exactly on them.
                let r = match next() % 6 {
                    0 => GRID_ENTER,
                    1 => GRID_LEAVE,
                    _ => (next() % 3000) as f64 / 1000.0 * GRID_ENTER,
                };
                (1_000 + i as i64, [r, 0.0, 0.0])
            })
            .collect();
        let held: BTreeSet<i64> = (0..n + 5)
            .filter(|_| next() % 2 == 0)
            .map(|i| 1_000 + i as i64)
            .collect();
        let landed: BTreeSet<i64> = (0..n)
            .filter(|_| next() % 4 == 0)
            .map(|i| 1_000 + i as i64)
            .collect();
        let ego = 1_000 + (next() % (n as u64 + 1)) as i64;
        let keep: Vec<i64> = (0..n)
            .filter(|_| next() % 5 == 0)
            .map(|i| 1_000 + i as i64)
            .collect();
        let hidden: BTreeSet<i64> = (0..n)
            .filter(|_| next() % 7 == 0)
            .map(|i| 1_000 + i as i64)
            .collect();
        let by_id = id_order(&local);
        assert!(by_id.is_some());
        assert_eq!(
            diff_park(
                ego,
                at,
                &local,
                by_id.as_deref(),
                &held,
                &landed,
                &keep,
                &hidden
            ),
            diff_scan(ego, at, &local, &held, &landed, &keep, &hidden)
        );
    }
}

/// LANDING-ORBIT (#112): a landed ball the viewer orbits, follows or has
/// locked stays in its park, so its client keeps the order; any other landed
/// ball is still removed and added.
#[test]
fn a_landing_is_not_removed_from_a_viewer_who_holds_an_order_on_it() {
    let at = [0.0; 3];
    let local = vec![(2_001, [1_000.0, 0.0, 0.0]), (2_002, [2_000.0, 0.0, 0.0])];
    let held: BTreeSet<i64> = [2_001, 2_002].into();
    let landed: BTreeSet<i64> = [2_001, 2_002].into();
    let by_id = id_order(&local);
    let plain = diff_park(
        2_000,
        at,
        &local,
        by_id.as_deref(),
        &held,
        &landed,
        &[],
        &BTreeSet::new(),
    );
    assert_eq!(plain, (vec![2_001, 2_002], vec![2_001, 2_002]));
    let kept = diff_park(
        2_000,
        at,
        &local,
        by_id.as_deref(),
        &held,
        &landed,
        &[2_001],
        &BTreeSet::new(),
    );
    assert_eq!(kept, (vec![2_002], vec![2_002]));
    assert_eq!(
        kept,
        diff_scan(
            2_000,
            at,
            &local,
            &held,
            &landed,
            &[2_001],
            &BTreeSet::new()
        )
    );
}

// ---- PA-7 --------------------------------------------------------------

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// The old `filter`, kept as the oracle the fast path and `filter_all` are held to.
fn cut_reference(ids: &[i64], keep: impl Fn(i64) -> bool) -> Cut {
    let kept: Vec<i64> = ids.iter().copied().filter(|id| keep(*id)).collect();
    if kept.is_empty() {
        Cut::Nothing
    } else if kept.len() == ids.len() {
        Cut::All
    } else {
        Cut::Only(kept)
    }
}

/// **The drain filter**: one push in, the pushes to send out, with
/// [`Parks`] brought up to date as if they had been sent.
///
/// Run by `Registry::take_pending` and `Registry::take_due` on every push, in
/// order, so the model and the wire agree. A push whose audience is
/// unchanged comes back as itself; one whose audience shrank comes back
/// wrapped in [`Addressed`]; a viewer who takes part of a multi-ball batch
/// gets a cut copy of their own ([`Push::narrowed`]).
fn filter_reference(w: &mut Registry, push: Box<dyn Push>) -> Vec<Box<dyn Push>> {
    let role = push.grid();
    if role == GridRole::None {
        // The forcing function for a new Destiny push (`Push::grid`): one
        // that forgot its role would reach every pilot in the system.
        debug_assert!(
            push.to_push().0 != "DoDestinyUpdate",
            "a DoDestinyUpdate push with no grid role: {push:?}"
        );
        return vec![push];
    }
    let Some(audience) = push.audience() else {
        return vec![push];
    };
    let mut whole = Vec::new();
    let mut parts: Vec<Box<dyn Push>> = Vec::new();
    let mut writes: Vec<Write> = Vec::new();
    for v in audience.iter().copied() {
        // A state is where a model begins, so it is taken before the
        // unmodelled test below.
        if let GridRole::Resets(ids) = &role {
            writes.push(Write::reset(v, ids.clone()));
            whole.push(v);
            continue;
        }
        let Some(held) = w.ext::<Parks>().of(v) else {
            whole.push(v);
            continue;
        };
        let taken = match &role {
            GridRole::None | GridRole::Resets(_) => unreachable!("handled above"),
            GridRole::Swaps { gone, adds } => {
                if held.contains(gone) {
                    whole.push(v);
                    writes.push(Write {
                        viewer: v,
                        removed: vec![*gone],
                        added: adds.clone(),
                        reset: None,
                    });
                }
                continue;
            }
            GridRole::Spawns { needs, adds } => {
                if needs.iter().all(|b| held.contains(b)) {
                    whole.push(v);
                    writes.push(Write {
                        viewer: v,
                        removed: Vec::new(),
                        added: adds.clone(),
                        reset: None,
                    });
                }
                continue;
            }
            GridRole::Names(ids) | GridRole::Removes(ids) | GridRole::Hides(ids) => {
                (cut_reference(ids, |b| held.contains(&b)), ids.clone())
            }
            GridRole::Adds(placed) => {
                // Inside the enter radius of the viewer's ship as the roster
                // last placed it -- at most a reconcile old -- and not held
                // already: the reconcile and a spawn path may both add a
                // ball in one tick, and the client must not get it twice.
                let me = w.roster().get(v).map(|p| (p.ship_id, p.motion.position));
                let on = |id: i64, at: [f64; 3]| {
                    me.is_none_or(|(ship, ego)| {
                        id == ship
                            || (on_grid(ego, at, false) && !crate::cloak::hidden_from(w, id, ship))
                    })
                };
                let ids: Vec<i64> = placed.iter().map(|(id, _)| *id).collect();
                // A set, not a `Vec`: a warm belt's batch is thousands of
                // balls, and `cut` asks once per ball (lane perf-fight).
                let wanted: BTreeSet<i64> = placed
                    .iter()
                    .filter(|(id, at)| !held.contains(id) && on(*id, *at))
                    .map(|(id, _)| *id)
                    .collect();
                (cut_reference(&ids, |b| wanted.contains(&b)), ids)
            }
        };
        let delivered = match taken {
            (Cut::All, ids) => {
                whole.push(v);
                ids
            }
            (Cut::Nothing, _) => continue,
            (Cut::Only(some), _) => {
                let keep: BTreeSet<i64> = some.iter().copied().collect();
                match push.narrowed(vec![v], &|b| keep.contains(&b)) {
                    Some(p) => {
                        parts.push(p);
                        some
                    }
                    None => continue,
                }
            }
        };
        match role {
            GridRole::Removes(_) => writes.push(Write {
                viewer: v,
                removed: delivered,
                added: Vec::new(),
                reset: None,
            }),
            GridRole::Hides(_) => {
                let own = w.roster().get(v).map(|p| p.ship_id);
                writes.push(Write {
                    viewer: v,
                    removed: delivered.into_iter().filter(|b| Some(*b) != own).collect(),
                    added: Vec::new(),
                    reset: None,
                });
            }
            GridRole::Adds(_) => writes.push(Write {
                viewer: v,
                removed: Vec::new(),
                added: delivered,
                reset: None,
            }),
            _ => {}
        }
    }
    let parks = w.ext_mut::<Parks>();
    for write in writes {
        let held = parks.held.entry(write.viewer).or_default();
        if let Some(ids) = write.reset {
            *held = ids.into_iter().collect();
        }
        for id in write.removed {
            held.remove(&id);
        }
        held.extend(write.added);
    }
    let mut out: Vec<Box<dyn Push>> = Vec::new();
    if whole.len() == audience.len() {
        out.push(push);
    } else if !whole.is_empty() {
        out.push(Box::new(Addressed {
            to: Some(whole),
            inner: push,
        }));
    }
    out.extend(parts);
    out
}

/// A push with a declared role, for the filter's differential tests.
#[derive(Debug)]
struct RolePush {
    to: Vec<i64>,
    role: GridRole,
    tag: u32,
}

/// A tag whose `narrowed` panics.
const POISON: u32 = 9_999;

impl Push for RolePush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("TestPush", vec![PyValue::Int(i64::from(self.tag))])
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
    fn grid(&self) -> GridRole {
        self.role.clone()
    }
    fn narrowed(&self, to: Vec<i64>, keep: &dyn Fn(i64) -> bool) -> Option<Box<dyn Push>> {
        assert!(self.tag != POISON, "poisoned push");
        let role = match &self.role {
            GridRole::Names(ids) => {
                GridRole::Names(ids.iter().copied().filter(|b| keep(*b)).collect())
            }
            GridRole::Removes(ids) => {
                GridRole::Removes(ids.iter().copied().filter(|b| keep(*b)).collect())
            }
            GridRole::Adds(p) => {
                GridRole::Adds(p.iter().copied().filter(|(b, _)| keep(*b)).collect())
            }
            // A role that is never cut: a push that cannot be narrowed.
            _ => return None,
        };
        Some(Box::new(RolePush {
            to,
            role,
            tag: self.tag,
        }))
    }
}

const BALLS: i64 = 12;
const FIRST_BALL: i64 = 1_000;

/// Two identical worlds: three real pilots in space at set places, and
/// `extra` more viewer ids that exist only in the parks.
fn twin_worlds(rng: &mut Lcg, extra: i64) -> (Registry, Registry, Vec<i64>) {
    let mut worlds = Vec::new();
    let mut viewers: Vec<i64> = vec![ALICE, BOB, CAROL];
    viewers.extend((0..extra).map(|k| 7_000 + k));
    let mut held: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    for v in &viewers {
        // Some viewers are unmodelled.
        if rng.below(5) != 0 {
            held.insert(
                *v,
                (0..BALLS)
                    .filter(|_| rng.below(2) == 0)
                    .map(|k| FIRST_BALL + k)
                    .collect(),
            );
        }
    }
    let offsets = [rng.below(3), rng.below(3), rng.below(3)];
    for _ in 0..2 {
        let mut reg = world();
        for (who, off) in [ALICE, BOB, CAROL].into_iter().zip(offsets) {
            undock(&mut reg, who);
            put(&mut reg, who, [GRID_ENTER * off as f64, 0.0, 0.0]);
        }
        reg.ext_mut::<Parks>().held = held.clone();
        worlds.push(reg);
    }
    let b = worlds.pop().expect("two");
    let a = worlds.pop().expect("two");
    (a, b, viewers)
}

fn random_pushes(rng: &mut Lcg, viewers: &[i64], n: usize) -> Vec<(Vec<i64>, GridRole, u32)> {
    let ball = |rng: &mut Lcg| FIRST_BALL + rng.below(BALLS as u64) as i64;
    (0..n)
        .map(|tag| {
            let to: Vec<i64> = (0..1 + rng.below(30))
                .map(|_| viewers[rng.below(viewers.len() as u64) as usize])
                .collect();
            let ids =
                |rng: &mut Lcg| -> Vec<i64> { (0..1 + rng.below(5)).map(|_| ball(rng)).collect() };
            let role = match rng.below(7) {
                0 => GridRole::None,
                1 => GridRole::Resets(ids(rng)),
                2 => GridRole::Swaps {
                    gone: ball(rng),
                    adds: ids(rng),
                },
                3 => GridRole::Spawns {
                    needs: ids(rng),
                    adds: ids(rng),
                },
                4 => GridRole::Names(ids(rng)),
                5 => GridRole::Removes(ids(rng)),
                _ => GridRole::Adds(
                    ids(rng)
                        .into_iter()
                        .map(|b| (b, [GRID_ENTER * rng.below(4) as f64 * 0.7, 0.0, 0.0]))
                        .collect(),
                ),
            };
            (to, role, tag as u32)
        })
        .collect()
}

fn boxed(specs: &[(Vec<i64>, GridRole, u32)]) -> Vec<Box<dyn Push>> {
    specs
        .iter()
        .map(|(to, role, tag)| {
            Box::new(RolePush {
                to: to.clone(),
                role: role.clone(),
                tag: *tag,
            }) as Box<dyn Push>
        })
        .collect()
}

/// The pushes as text, clocks blanked: `take_due` binds a stamp, and a park
/// its `current_time`, of whole wall-clock seconds, so two drains back to
/// back can straddle a second.
fn dump(out: &[Box<dyn Push>]) -> Vec<String> {
    out.iter()
        .map(|p| blanked(&unstamped(&format!("{p:?}")), "current_time: "))
        .collect()
}

fn unstamped(s: &str) -> String {
    blanked(s, "stamp: ")
}

/// `s` with the digits after each `key` replaced by `_`.
fn blanked(s: &str, key: &str) -> String {
    let mut parts = s.split(key);
    let mut out = parts.next().unwrap_or_default().to_owned();
    for part in parts {
        out.push_str(key);
        out.push('_');
        out.push_str(part.trim_start_matches(|c: char| c.is_ascii_digit()));
    }
    out
}

fn park_state(reg: &Registry) -> String {
    format!("{:?}", reg.ext::<Parks>().held)
}

/// 7b: the fast `filter` is the old one, push by push, outputs and parks.
#[test]
fn the_fast_filter_matches_the_reference() {
    let mut rng = Lcg(0x9E37_79B9_7F4A_7C15);
    for case in 0..300 {
        let (mut a, mut b, viewers) = twin_worlds(&mut rng, 37);
        let n = 1 + rng.below(60) as usize;
        let specs = random_pushes(&mut rng, &viewers, n);
        let mut got = Vec::new();
        let mut want = Vec::new();
        for p in boxed(&specs) {
            want.extend(filter_reference(&mut a, p));
        }
        for p in boxed(&specs) {
            got.extend(filter(&mut b, p));
        }
        assert_eq!(dump(&got), dump(&want), "case {case}");
        assert_eq!(park_state(&b), park_state(&a), "case {case}");
    }
}

/// 7c: `filter_all` per viewer group is the push-by-push filter, whatever
/// the thread count; the parallel path really runs.
#[test]
fn filter_all_matches_the_push_by_push_filter() {
    let mut rng = Lcg(0x2545_F491_4F6C_DD1D);
    for case in 0..300 {
        let (mut a, mut b, viewers) = twin_worlds(&mut rng, 37);
        let n = 1 + rng.below(60) as usize;
        let specs = random_pushes(&mut rng, &viewers, n);
        let mut want = Vec::new();
        for p in boxed(&specs) {
            want.extend(filter(&mut a, p));
        }
        let threads = [1, 2, 3, 8][case % 4];
        let spawned = crate::par::spawned_here();
        let got = filter_all_with(&mut b, boxed(&specs), threads, 0);
        if threads > 1 {
            assert!(crate::par::spawned_here() > spawned, "ran on the kit");
        }
        assert_eq!(dump(&got), dump(&want), "case {case}");
        assert_eq!(park_state(&b), park_state(&a), "case {case}");
    }
}

/// A worker that panics: `filter_all` raises it again and the parks are put back.
#[test]
fn filter_all_puts_the_parks_back_when_a_worker_panics() {
    let mut rng = Lcg(0x1234_5678_9ABC_DEF1);
    let (_, mut b, viewers) = twin_worlds(&mut rng, 37);
    // Every viewer holds exactly the first ball: a batch of two names is cut.
    for park in b.ext_mut::<Parks>().held.values_mut() {
        *park = BTreeSet::from([FIRST_BALL]);
    }
    let keys: Vec<i64> = b.ext::<Parks>().viewers().collect();
    assert!(!keys.is_empty());
    let pushes: Vec<Box<dyn Push>> = vec![Box::new(RolePush {
        to: viewers.clone(),
        role: GridRole::Names(vec![FIRST_BALL, FIRST_BALL + 1]),
        tag: POISON,
    })];
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        filter_all_with(&mut b, pushes, 4, 0)
    }));
    let msg = match r {
        Ok(_) => panic!("a poisoned push must panic"),
        Err(e) => e.downcast_ref::<String>().cloned().unwrap_or_default(),
    };
    assert!(msg.contains("grid filter worker panicked"), "{msg}");
    let after: Vec<i64> = b.ext::<Parks>().viewers().collect();
    assert_eq!(after, keys, "every viewer's park is back");
}

/// `n` pilots, docked, ids invented.
fn crowd(n: i64) -> Registry {
    world_of(
        (0..n)
            .map(|k| pilot_at_station(ALICE + 100 + k, &format!("P{k}"), ALICE_SHIP + 100 + k))
            .collect(),
    )
}

fn crowd_ids(n: i64) -> Vec<i64> {
    (0..n).map(|k| ALICE + 100 + k).collect()
}

fn undock_all(reg: &mut Registry, n: i64) {
    for who in crowd_ids(n) {
        on_ballpark_bind(reg, Some(who));
    }
    drain(reg);
}

/// 7a: the indexed `added` is the scan's answer, over random scenes with
/// repeated slim ids, unknown ids, duplicate ids and wrecks.
#[test]
fn the_indexed_added_matches_added() {
    let mut rng = Lcg(0xDEAD_BEEF_CAFE_F00D);
    let mut reg = crowd(12);
    undock_all(&mut reg, 12);
    let base = crate::destiny::scene(&reg, SYSTEM, 0, None, 0);
    assert!(base.slims.len() > 3 && base.park.balls.len() > 3, "setup");
    for case in 0..200 {
        let mut scene = base.clone();
        let ball_ids: Vec<i64> = scene.park.balls.iter().map(|b| b.ball_id).collect();
        let pick = |rng: &mut Lcg| ball_ids[rng.below(ball_ids.len() as u64) as usize];
        // A repeated slim id, damage twice, modules and extras for balls.
        if case % 2 == 0 {
            let s = scene.slims[rng.below(scene.slims.len() as u64) as usize];
            scene.slims.push(s);
        }
        for _ in 0..rng.below(4) {
            let id = pick(&mut rng);
            scene.damage.push((id, PyValue::Int(rng.below(100) as i64)));
        }
        for _ in 0..rng.below(6) {
            let id = pick(&mut rng);
            scene.modules.push((id, Vec::new()));
            scene
                .extras
                .push((id, vec![("k", PyValue::Int(i64::from(case)))]));
        }
        for _ in 0..rng.below(4) {
            let id = pick(&mut rng);
            scene.wrecks.push(crate::destiny::WreckSlim {
                item_id: id,
                type_id: 1,
                owner_id: 2,
                corp_id: 3,
                launcher_id: 4,
                is_empty: rng.below(2) == 0,
                fleet_id: None,
                abandoned: false,
            });
        }
        let ix = scene.add_index();
        let ids: Vec<i64> = (0..rng.below(20))
            .map(|_| match rng.below(4) {
                0 => 5_000 + rng.below(10) as i64, // unknown
                _ => pick(&mut rng),
            })
            .collect();
        assert_eq!(
            scene.added_indexed(&ix, vec![ALICE], 7, &ids),
            scene.added_reference(vec![ALICE], 7, &ids),
            "case {case}"
        );
    }
}

/// 7a: the reconcile on the kit schedules what the serial one does, in the
/// same order, and leaves the same parks.
#[test]
fn the_parallel_reconcile_matches_the_serial() {
    let mut rng = Lcg(0xA5A5_1234_5678_9ABC);
    for round in 0..6 {
        let n = 60;
        let mut a = crowd(n);
        let mut b = crowd(n);
        undock_all(&mut a, n);
        undock_all(&mut b, n);
        // Two clusters, a third of the pilots far off both.
        for (k, who) in crowd_ids(n).into_iter().enumerate() {
            let cluster = [BELT_A_AT, BELT_B_AT][k % 2];
            let off = if rng.below(3) == 0 { 40.0 } else { 0.6 };
            let jitter = |rng: &mut Lcg| (rng.below(2000) as f64 / 1000.0 - 1.0) * GRID_ENTER * off;
            let at = plus(cluster, [jitter(&mut rng), jitter(&mut rng), 0.0]);
            put(&mut a, who, at);
            put(&mut b, who, at);
        }
        let mins = ReconcileMins {
            diff_pairs: 0,
            added: 0,
        };
        let spawned = crate::par::spawned_here();
        reconcile_in_with(&mut a, SYSTEM, 1, mins);
        assert_eq!(
            crate::par::spawned_here(),
            spawned,
            "one thread spawns none"
        );
        reconcile_in_with(&mut b, SYSTEM, 8, mins);
        assert!(
            crate::par::spawned_here() > spawned,
            "round {round}: on the kit"
        );
        let far = Instant::now() + Duration::from_secs(600);
        // Both drained through the same filter; `dump` blanks the stamps.
        let (oa, ob) = (a.take_due(far), b.take_due(far));
        assert!(!oa.is_empty());
        assert_eq!(dump(&ob), dump(&oa), "round {round}");
        assert_eq!(park_state(&b), park_state(&a), "round {round}");
    }
}

/// **PA-7's filter bench**: a drain of `Names` pushes (the shape of a mining
/// or fight tick: many pushes, each to every pilot on the grid), through the
/// old filter, the fast serial filter and `filter_all`. `#[ignore]`d.
///
/// ```text
/// tools/dev.sh cargo test --release --locked -p eve-server filter_bench -- --ignored --nocapture
/// ```
#[test]
#[ignore = "a timing: run it in release with --ignored --nocapture"]
fn filter_bench() {
    let pushes_per_drain = 150;
    println!("| viewers | pushes | reference ms | filter ms | filter_all x1 ms | x4 ms | x12 ms |");
    println!("|---|---|---|---|---|---|---|");
    for viewers_n in [262_i64, 500] {
        let viewers: Vec<i64> = (0..viewers_n).map(|k| 7_000 + k).collect();
        let mut reg = world();
        reg.ext_mut::<Parks>().held = viewers
            .iter()
            .map(|v| (*v, (0..BALLS).map(|k| FIRST_BALL + k).collect()))
            .collect();
        let make = || -> Vec<Box<dyn Push>> {
            (0..pushes_per_drain)
                .map(|tag| {
                    Box::new(RolePush {
                        to: viewers.clone(),
                        role: GridRole::Names(vec![FIRST_BALL + tag as i64 % BALLS]),
                        tag,
                    }) as Box<dyn Push>
                })
                .collect()
        };
        let time = |f: &mut dyn FnMut(Vec<Box<dyn Push>>)| {
            let reps = 30;
            let mut total = Duration::ZERO;
            for _ in 0..reps {
                let input = make();
                let t = Instant::now();
                f(input);
                total += t.elapsed();
            }
            total.as_secs_f64() * 1000.0 / f64::from(reps)
        };
        let reference = time(&mut |p| {
            for x in p {
                std::hint::black_box(filter_reference(&mut reg, x));
            }
        });
        let fast = time(&mut |p| {
            for x in p {
                std::hint::black_box(filter(&mut reg, x));
            }
        });
        let mut all = [0.0; 3];
        for (slot, threads) in all.iter_mut().zip([1, 4, 12]) {
            *slot = time(&mut |p| {
                std::hint::black_box(filter_all_with(&mut reg, p, threads, FILTER_PAR_MIN));
            });
        }
        println!(
            "| {viewers_n} | {pushes_per_drain} | {reference:.2} | {fast:.2} | {:.2} | {:.2} | {:.2} |",
            all[0], all[1], all[2]
        );
    }
}

/// **A follower does not ride its leader's warp out of the park**
/// (live-fleetgrid): Alice follows Bob, Bob lands 16 AU away. The follow snap
/// used to carry her with him, so neither left the other's park and the
/// overview listed her at 16 AU. Now she is flown, not snapped, and the grid
/// rule drops both.
#[test]
fn a_follower_leaves_the_park_of_a_leader_who_warped_off_grid() {
    let mut reg = world();
    undock(&mut reg, ALICE);
    undock(&mut reg, BOB);
    assert!(held(&reg, ALICE).contains(&BOB_SHIP), "setup");
    assert!(held(&reg, BOB).contains(&ALICE_SHIP), "setup");
    reg.roster_mut()
        .get_mut(ALICE)
        .expect("in space")
        .motion
        .after(crate::destiny::Action::FollowBall {
            ball: ALICE_SHIP,
            target: BOB_SHIP,
            range: 2_500.0,
        });
    let _ = drain(&mut reg);
    put(&mut reg, BOB, plus(STATION_AT, [0.0, 0.0, 16.0 * AU]));
    let out = settle(&mut reg);
    assert!(removed_from(&out, ALICE).contains(&BOB_SHIP));
    assert!(removed_from(&out, BOB).contains(&ALICE_SHIP));
    let at = reg.roster().get(ALICE).expect("in space").motion.position;
    let off: f64 = (0..3)
        .map(|i| (at[i] - STATION_AT[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    assert!(off < GRID_ENTER, "she was not dragged: {off} m");
    assert!(settle(&mut reg).is_empty(), "nothing more to send");
}

fn damage_to_both(reg: &mut Registry) {
    let p = crate::destiny::DamageStateChange::new(
        vec![ALICE, BOB],
        1,
        BOB_SHIP,
        crate::destiny::DamageState::FULL,
    )
    .expect("bars");
    reg.schedule(Duration::ZERO, p);
}

/// **A pilot whose session change has gone and whose bind has not come takes
/// no push**: a stale volley ahead of the new park's `SetState` would make the
/// client ignore the state for good. Others still get it.
#[test]
fn a_pilot_awaiting_her_bind_takes_no_stale_push() {
    let mut reg = world();
    undock(&mut reg, ALICE);
    undock(&mut reg, BOB);
    forget(&mut reg, ALICE);
    crate::jump::arm_arrival(&mut reg, ALICE, OTHER_SYSTEM, [0.0; 3]);
    damage_to_both(&mut reg);
    let out = drain(&mut reg);
    assert!(sent_to(&out, ALICE).is_empty(), "Alice awaits her bind");
    assert_eq!(sent_to(&out, BOB).len(), 1, "Bob still takes it");
}

/// The departing leg is untouched: the tunnel and the removal reach the jumper.
#[test]
fn a_departing_pilot_still_takes_her_pushes() {
    let mut reg = world();
    undock(&mut reg, ALICE);
    undock(&mut reg, BOB);
    forget(&mut reg, ALICE);
    crate::jump::arm_at(&mut reg, ALICE, SYSTEM, OTHER_SYSTEM, [0.0; 3]);
    damage_to_both(&mut reg);
    let out = drain(&mut reg);
    assert_eq!(sent_to(&out, ALICE).len(), 1, "Alice is still departing");
}

/// Once the bind has landed her, pushes reach her again.
#[test]
fn pushes_reach_her_again_after_the_bind() {
    let mut reg = world();
    undock(&mut reg, BOB);
    crate::jump::arm_arrival(&mut reg, ALICE, SYSTEM, BELT_A_AT);
    assert!(crate::jump::awaiting_bind(&reg, ALICE));
    on_ballpark_bind(&mut reg, Some(ALICE));
    let _ = drain(&mut reg);
    put(&mut reg, ALICE, BELT_A_AT);
    put(&mut reg, BOB, BELT_A_AT);
    let _ = settle(&mut reg);
    assert!(held(&reg, ALICE).contains(&BOB_SHIP), "Bob is on her grid");
    assert!(!crate::jump::awaiting_bind(&reg, ALICE));
    damage_to_both(&mut reg);
    let out = drain(&mut reg);
    assert_eq!(sent_to(&out, ALICE).len(), 1);
}
