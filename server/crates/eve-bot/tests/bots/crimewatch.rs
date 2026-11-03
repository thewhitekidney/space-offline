//! CW-1 over a real socket: crimewatch in high sec.
//!
//! Two scenarios, each on its own server, both in an invented high-sec
//! system B one jump from the sandbox home A:
//!
//! - **a criminal kill earns a kill right**: Alice's gun is refused at green
//!   safety (`ModuleActivationDeniedSafetyPreventsCriminal`); at red it
//!   fires, Alice is a criminal (her HUD's `OnCriminalTimerUpdate`, the
//!   system's `OnSystemCriminalFlagUpdates`), Bob's ship dies, and Bob holds
//!   a kill right on Alice (`OnKillRightCreated`, `GetMyKillRights`);
//! - **theft makes the thief a suspect, and the victim may shoot**: Carol
//!   is refused Bob's wreck at green (`LootTheftDeniedSafetyPreventsSuspect`);
//!   at yellow she loots it and is a suspect; Bob goes home, reships, comes
//!   back and shoots her at **green** -- no refusal, no flag of his own,
//!   and a limited engagement between the two.
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **Invented rows only** (the no-client-data rule): `typedata::FIXTURE`'s
//! frigate and autocannon, an invented round (killmail's), an invented
//! wreck type for the frigate, an invented gate type, and an invented
//! universe row for B at 0.9. Nothing here is an `[A]` value.

use super::common::{config, gm, login, pilot, serve, until};
use eve_bot::Conn;
use eve_marshal::PyValue;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const ALICE: i64 = 90_000_201;
const BOB: i64 = 90_000_202;
const CAROL: i64 = 90_000_203;
/// The sandbox home (`Character::default()`'s) and the high-sec system.
const A: i64 = 30_000_001;
const B: i64 = 30_000_003;
const GATE_A: i64 = 50_000_201;
const GATE_B: i64 = 50_000_202;
/// An invented stargate type.
const GATE_TYPE: i64 = 990_029_001;
/// `typedata::FIXTURE`'s frigate and its autocannon.
const FRIGATE: i64 = 990_000_001;
const AUTOCANNON: i64 = 990_000_002;
/// Killmail's invented round: kinetic, one volley kills a fixture frigate.
const ROUND: i64 = 990_630_001;
/// An invented wreck type for the fixture frigate (an invented category:
/// typedata keeps no celestial-category type as an item).
const WRECK: i64 = 990_026_001;
/// Stacks of rounds in Bob's hold: each survives his ship's death on its
/// own roll (`combat::DROP_CHANCE`), so the wreck holds something.
const STACKS: usize = 8;

/// `crimewatch/const.py` **[V]** state numbers the test reads.
const TIMER_CRIMINAL: i64 = 303;
const TIMER_SUSPECT: i64 = 304;

fn content() -> Arc<eve_server::content::Content> {
    let rows = format!(
        "{}type\t{ROUND}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=1000000.0;128=1.0\tFixture Killer Round\n\
         type\t{WRECK}\t186\t990\t1\t1\t0.0\t1000.0\t10000.0\t14.0\t0\t\t\tFixture Wreck\n",
        eve_server::typedata::FIXTURE
    );
    let mut content = eve_server::content::Content::fixture();
    let table = eve_server::typedata::Table::parse(&rows).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    let universe = eve_server::celestial::Universe::parse(
        &[format!(
            "{B}\t19000001\t29000001\t\t\t0.9\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tx"
        )],
        &[],
        &[],
    )
    .expect("the invented universe row parses");
    content.insert("universe", universe);
    content.insert("wrecks", eve_server::Wrecks::from_pairs([(FRIGATE, WRECK)]));
    Arc::new(content)
}

/// Two systems and their gates, synthetic (`jump.rs`'s format).
fn catalogue() -> eve_server::celestial::Catalogue {
    eve_server::celestial::Catalogue::parse(&format!(
        "#system\t{A}\t20000001\t10000001\n\
         #system\t{B}\t20000001\t10000001\n\
         gate\t{GATE_A}\t{GATE_TYPE}\t2473.0\t3000000000.0\t0.0\t0.0\t{A}\t{GATE_B}\t{B}\n\
         gate\t{GATE_B}\t{GATE_TYPE}\t2473.0\t0.0\t0.0\t-3000000000.0\t{B}\t{GATE_A}\t{A}\n"
    ))
}

fn start() -> (SocketAddr, Arc<eve_server::content::Content>) {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_201),
        pilot(BOB, "Bob", 100_000_202),
        pilot(CAROL, "Carol", 100_000_203),
    ];
    config.sandbox.celestials = catalogue();
    let content = content();
    config.content = Some(content.clone());
    (serve(config), content)
}

fn ids_of(c: &mut Conn, type_id: i64) -> Vec<i64> {
    c.hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .filter(|r| r.type_id == type_id)
        .map(|r| r.item_id)
        .collect()
}

/// Docked: create a fixture frigate and board it.
fn board_frigate(c: &mut Conn) -> i64 {
    gm(c, &format!("/createitem {FRIGATE}"));
    let hull = *ids_of(c, FRIGATE)
        .last()
        .expect("the frigate is in the hangar");
    c.board(hull).expect("boards the frigate");
    hull
}

/// Docked, in a frigate: fit the autocannon, online it, load the rounds.
fn arm(c: &mut Conn) -> i64 {
    gm(c, &format!("/createitem {AUTOCANNON}"));
    gm(c, &format!("/createitem {ROUND} 10"));
    let station = c.session().station_id().expect("docked");
    let spare = ids_of(c, AUTOCANNON);
    let gun = c
        .fit_auto(&spare)
        .expect("fits")
        .into_iter()
        .find(|r| r.type_id == AUTOCANNON)
        .expect("the autocannon is fitted");
    if !c.is_online(gun.item_id).expect("asks") {
        c.set_online(gun.item_id).expect("onlines");
    }
    let rounds = ids_of(c, ROUND);
    c.load_ammo(&[(gun.item_id, gun.flag_id)], &rounds, ROUND, station)
        .expect("loads");
    gun.item_id
}

/// In space by `from_gate`: jump to `to_system` and bind its park.
fn jump(c: &mut Conn, from_gate: i64, to_gate: i64, to_system: i64) {
    let ship = c.session().ship_id().expect("a ship");
    let outcome = c
        .bound_call(
            "CmdStargateJump",
            vec![
                PyValue::Int(from_gate),
                PyValue::Int(to_gate),
                PyValue::Int(ship),
            ],
        )
        .expect("the jump is answered");
    assert_eq!(outcome.refusal(), None, "the gate lets us through");
    let arrived = until(Duration::from_secs(15), || {
        let _ = c.collect_pushes(Duration::from_millis(200));
        c.session().solar_system_id() == Some(to_system)
    });
    assert!(arrived, "the jump's session change into {to_system}");
    c.bind_ballpark(to_system).expect("binds the new park");
}

/// Docked at home: undock and jump to B.
fn undock_to_b(c: &mut Conn) {
    c.undock().expect("undocks");
    c.enter_space(A).expect("A's state");
    jump(c, GATE_A, GATE_B, B);
}

/// `crimewatch.SetSafetyLevel(level)` on the crimewatch moniker.
fn set_safety(c: &mut Conn, level: i64) {
    let answer = c
        .moniker_call(
            "crimewatch",
            PyValue::Int(c.session().solar_system_id().unwrap_or(A)),
            "SetSafetyLevel",
            vec![PyValue::Int(level)],
            vec![],
        )
        .expect("answered");
    assert_eq!(answer.outcome.refusal(), None);
}

/// Every push this connection kept since the last call, after reading for
/// a moment.
fn pushes(c: &mut Conn) -> Vec<(String, Vec<PyValue>)> {
    let _ = c.collect_pushes(Duration::from_millis(500));
    c.take_pushes()
}

type Pushes = Vec<(String, Vec<PyValue>)>;

/// Every push kept until one named `event` whose arguments satisfy `args`
/// has arrived, or ten seconds: the clock delivers crimewatch's pushes on
/// its own beat, which a loaded machine stretches.
fn pushes_until(c: &mut Conn, event: &str, args: impl Fn(&[PyValue]) -> bool) -> Pushes {
    let mut seen: Pushes = Vec::new();
    let found = until(Duration::from_secs(10), || {
        let _ = c.collect_pushes(Duration::from_millis(200));
        seen.extend(c.take_pushes());
        seen.iter().any(|(e, a)| e == event && args(a))
    });
    assert!(
        found,
        "no {event} with the wanted arguments: {:?}",
        seen.iter().map(|(e, _)| e).collect::<Vec<_>>()
    );
    seen
}

fn ints(v: &PyValue) -> Vec<i64> {
    match v {
        PyValue::List(xs) | PyValue::Tuple(xs) => xs
            .iter()
            .filter_map(|x| match x {
                PyValue::Int(i) => Some(*i),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

/// One attribute of a `utillib.KeyVal` (its state dict is value first).
fn attr<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
    let PyValue::Instance { state, .. } = v else {
        return None;
    };
    state
        .dict_as_kv()?
        .into_iter()
        .find_map(|(k, val)| matches!(k, PyValue::Bytes(b) if b == name.as_bytes()).then_some(val))
}

fn my_kill_rights(c: &mut Conn) -> Vec<(PyValue, PyValue)> {
    let answer = c
        .call("bountyProxy", "GetMyKillRights", vec![])
        .expect("answered")
        .value();
    let Some(PyValue::List(rows)) = answer else {
        panic!("a list: {answer:?}")
    };
    rows.iter()
        .map(|r| {
            (
                attr(r, "fromID").cloned().expect("fromID"),
                attr(r, "toID").cloned().expect("toID"),
            )
        })
        .collect()
}

pub(super) struct Fight {
    pub(super) content: Arc<eve_server::content::Content>,
    pub(super) alice: Conn,
    pub(super) bob: Conn,
    pub(super) carol: Conn,
    pub(super) bob_station: i64,
    pub(super) alice_pushes: Vec<(String, Vec<PyValue>)>,
    pub(super) carol_pushes: Vec<(String, Vec<PyValue>)>,
    pub(super) bob_pushes: Vec<(String, Vec<PyValue>)>,
}

/// The shared opening: Bob (a hold full of rounds) and Carol fly to B; Alice
/// follows armed, is refused at green, sets red and kills Bob's ship.
pub(super) fn fight() -> Fight {
    let (addr, content) = start();

    let mut bob = login(addr, "Bob");
    bob.keep_pushes();
    let bob_station = bob.session().station_id().expect("docked");
    let bob_ship = board_frigate(&mut bob);
    for _ in 0..STACKS {
        gm(&mut bob, &format!("/createitem {ROUND} 1"));
    }
    for id in ids_of(&mut bob, ROUND) {
        bob.load_cargo(id, None).expect("into the hold");
    }
    undock_to_b(&mut bob);

    let mut carol = login(addr, "Carol");
    carol.keep_pushes();
    board_frigate(&mut carol);
    undock_to_b(&mut carol);

    let mut alice = login(addr, "Alice");
    alice.keep_pushes();
    board_frigate(&mut alice);
    let gun = arm(&mut alice);
    undock_to_b(&mut alice);
    alice.lock_target(bob_ship).expect("locks Bob's ship");

    // Green: the server refuses, in the client's words.
    let refused = alice
        .fire(gun, AUTOCANNON, Some(&content), bob_ship)
        .expect_err("green safety refuses a criminal act");
    assert!(
        refused
            .to_string()
            .contains("ModuleActivationDeniedSafetyPreventsCriminal"),
        "{refused}"
    );
    let _ = pushes(&mut alice);

    // Red: it fires, and the ship dies.
    set_safety(&mut alice, 0);
    alice
        .fire(gun, AUTOCANNON, Some(&content), bob_ship)
        .expect("fires at red");
    let dead = until(Duration::from_secs(30), || {
        let _ = bob.collect_pushes(Duration::from_millis(100));
        bob.session().ship_id() != Some(bob_ship)
    });
    assert!(dead, "Bob's ship dies");

    let alice_pushes = pushes_until(&mut alice, "OnCriminalTimerUpdate", |a| {
        a.first() == Some(&PyValue::Int(TIMER_CRIMINAL))
    });
    let carol_pushes = pushes_until(&mut carol, "OnSystemCriminalFlagUpdates", |a| {
        a.get(2).is_some_and(|c| ints(c).contains(&ALICE))
    });
    let bob_pushes = pushes_until(&mut bob, "OnKillRightCreated", |_| true);
    Fight {
        content,
        alice,
        bob,
        carol,
        bob_station,
        alice_pushes,
        carol_pushes,
        bob_pushes,
    }
}

/// **A criminal kill in high sec grants a kill right.** Alice, refused at
/// green, kills Bob at red: her HUD counts the criminal timer, Carol's
/// overview hears her in `newCriminals`, and Bob holds a kill right on her
/// -- pushed to both, listed for both.
#[test]
fn a_criminal_kill_in_high_sec_grants_a_kill_right() {
    let mut f = fight();

    assert!(
        f.alice_pushes
            .iter()
            .any(|(e, a)| e == "OnCriminalTimerUpdate"
                && a.first() == Some(&PyValue::Int(TIMER_CRIMINAL))),
        "Alice's criminal timer: {:?}",
        f.alice_pushes.iter().map(|(e, _)| e).collect::<Vec<_>>()
    );
    assert!(
        f.carol_pushes
            .iter()
            .any(|(e, a)| e == "OnSystemCriminalFlagUpdates"
                && a.get(2).is_some_and(|c| ints(c).contains(&ALICE))),
        "Carol is told Alice is a criminal"
    );
    let created = f
        .bob_pushes
        .iter()
        .find(|(e, _)| e == "OnKillRightCreated")
        .map(|(_, a)| a.clone())
        .expect("Bob is told of his kill right");
    assert_eq!(created.get(1), Some(&PyValue::Int(BOB)));
    assert_eq!(created.get(2), Some(&PyValue::Int(ALICE)));

    let want = vec![(PyValue::Int(BOB), PyValue::Int(ALICE))];
    assert_eq!(my_kill_rights(&mut f.bob), want, "the holder's list");
    assert_eq!(my_kill_rights(&mut f.alice), want, "the target's list");
    assert!(my_kill_rights(&mut f.carol).is_empty(), "nobody else's");
}

/// **Theft makes the thief a suspect, and the victim may shoot.** Carol is
/// refused Bob's wreck at green, takes from it at yellow and is a suspect
/// (her HUD, and Bob's overview); Bob reships at home, comes back and
/// shoots her at green with no refusal, no flag of his own, and a limited
/// engagement between the two.
#[test]
fn theft_makes_the_thief_a_suspect_and_the_victim_may_shoot() {
    let mut f = fight();
    let capsule = f.bob.session().ship_id().expect("Bob's capsule");
    let carol_ship = f.carol.session().ship_id().expect("Carol's ship");
    let wreck = f
        .carol
        .world_now()
        .balls()
        .find(|b| b.type_id() == Some(WRECK))
        .map(|b| b.ball_id)
        .expect("Bob's wreck is in Carol's park");

    let refused = f.carol.loot_all(wreck).expect_err("green refuses theft");
    assert!(
        refused
            .to_string()
            .contains("LootTheftDeniedSafetyPreventsSuspect"),
        "{refused}"
    );
    set_safety(&mut f.carol, 1);
    let looted = f.carol.loot_all(wreck).expect("yellow allows it");
    assert!(looted.units > 0, "something was taken");
    let carol_pushes = pushes_until(&mut f.carol, "OnCriminalTimerUpdate", |a| {
        a.first() == Some(&PyValue::Int(TIMER_SUSPECT))
    });
    assert!(
        carol_pushes
            .iter()
            .any(|(e, a)| e == "OnCriminalTimerUpdate"
                && a.first() == Some(&PyValue::Int(TIMER_SUSPECT))),
        "Carol's suspect timer"
    );
    // Bob's overview hears Carol is a suspect (the wait asserts it).
    pushes_until(&mut f.bob, "OnSystemCriminalFlagUpdates", |a| {
        a.get(1).is_some_and(|s| ints(s).contains(&CAROL))
    });

    // Bob goes home in his capsule, reships, and comes back armed.
    jump(&mut f.bob, GATE_B, GATE_A, A);
    let outcome = f
        .bob
        .bound_call(
            "CmdDock",
            vec![PyValue::Int(f.bob_station), PyValue::Int(capsule)],
        )
        .expect("the dock is answered");
    assert_eq!(outcome.refusal(), None);
    let docked = until(Duration::from_secs(10), || {
        let _ = f.bob.collect_pushes(Duration::from_millis(200));
        f.bob.session().station_id().is_some()
    });
    assert!(docked, "Bob docks at home");
    board_frigate(&mut f.bob);
    let gun = arm(&mut f.bob);
    undock_to_b(&mut f.bob);
    let _ = pushes(&mut f.bob);
    f.bob.lock_target(carol_ship).expect("locks Carol's ship");
    f.bob
        .fire(gun, AUTOCANNON, Some(&f.content), carol_ship)
        .expect("green safety allows shooting a suspect");
    let bob_pushes = pushes_until(&mut f.bob, "OnCrimewatchEngagementCreated", |a| {
        a.first() == Some(&PyValue::Int(CAROL))
    });
    assert!(
        !bob_pushes.iter().any(|(e, a)| e == "OnCriminalTimerUpdate"
            && a.first().is_some_and(|s| *s != PyValue::Int(300))),
        "Bob is flagged nothing: {:?}",
        bob_pushes
            .iter()
            .filter(|(e, _)| e == "OnCriminalTimerUpdate")
            .collect::<Vec<_>>()
    );
    assert!(
        bob_pushes
            .iter()
            .any(|(e, a)| e == "OnCrimewatchEngagementCreated"
                && a.first() == Some(&PyValue::Int(CAROL))),
        "Bob and Carol are engaged: {:?}",
        bob_pushes.iter().map(|(e, _)| e).collect::<Vec<_>>()
    );
}
