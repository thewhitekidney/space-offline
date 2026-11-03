//! SOC-C5 over real sockets: a hunter bot -- brain, driver, verbs, the
//! society's hall -- in a low-sec fixture system attacks a pilot it is not
//! allied with, spares one it is, and the fight lands in the relation
//! matrix; the directional scan the client calls is answered; a hunter
//! roams to another hunting-ground system over the gates.
//!
//! Same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes. **No `[A]` value is
//! an expected value here**: the doctrine is an invented fixture (the
//! sandbox's system given an invented low-sec number, the Rifter classed
//! `small`), and the assertions are about kinds, ids and signs.
//!
//! The pilots are plain connections ("Alice" beside the hunter's corp,
//! "Bob" outside it, both in Rifters); the hunter's hall holds two bot
//! corps by hand (the hunter's and Bob's), so a kill has a viewer.

use super::common::{SYSTEM, config, dump, login, ok, pilot, serve, until};
use eve_bot::brain::ActKind;
use eve_bot::career::{Band, Career, INCOME_REF, Signals, Support};
use eve_bot::corpmind::{CorpMind, Hall, Member};
use eve_bot::driver::{self, BotConfig, Pace, Society};
use eve_bot::hostile::Class;
use eve_bot::hunt::{self, Ground};
use eve_bot::mind::{Mind, TRAIT_COUNT, Trait, Traits};
use eve_bot::society::{Profile, Satisfaction};
use eve_server::model::Character;
use std::sync::Arc;
use std::time::Duration;

const HUNTER: i64 = 90_000_501;
const ALICE: i64 = 90_000_502;
const BOB: i64 = 90_000_503;
const HUNTER_SHIP: i64 = 100_000_501;
const ALICE_SHIP: i64 = 100_000_502;
const BOB_SHIP: i64 = 100_000_503;
/// The hunter's and Alice's player corporation; Bob's own. Invented.
const CORP: i64 = 98_000_511;
const BOB_CORP: i64 = 98_000_512;
/// A Rifter **[V-data]**: Alice's and Bob's hull, classed `small` by the
/// fixture doctrine.
const RIFTER: i64 = 587;
/// An invented low-sec number for the sandbox's system.
const LOW_SEC: f64 = 0.2;
/// An invented high-sec number.
const HIGH_SEC: f64 = 0.9;

/// One career, played by the hunter role: the society's `pirate`.
static TABLE: &[Career] = &[Career {
    name: "pirate",
    role: Some(&hunt::ROLE),
    support: Support::Served,
    base: 1.0,
    affinity: [0.0; TRAIT_COUNT],
    band: Band::Low,
    prior_isk_h: INCOME_REF,
}];

fn in_corp(c: Character, corp: i64) -> Character {
    Character {
        corporation_id: corp,
        ..c
    }
}

fn rifter(c: Character) -> Character {
    Character {
        ship_type_id: RIFTER,
        ..c
    }
}

/// The three pilots: the hunter and Alice in one corp, Bob in another.
fn pilots() -> Vec<Character> {
    vec![
        in_corp(pilot(HUNTER, "Hunter1", HUNTER_SHIP), CORP),
        rifter(in_corp(pilot(ALICE, "Alice", ALICE_SHIP), CORP)),
        rifter(in_corp(pilot(BOB, "Bob", BOB_SHIP), BOB_CORP)),
    ]
}

fn doctrine(security: f64) -> Ground {
    Ground::new()
        .with_security(SYSTEM, security)
        .with_class(RIFTER, Class::Small)
}

fn member(name: &str, corp: i64) -> Member {
    Member {
        name: name.to_owned(),
        traits: Traits([0.5; TRAIT_COUNT]),
        career: "pirate".to_owned(),
        band: Band::Low,
        corp,
        alliance: None,
        // A turn just taken, and turns an age apart: the hall's own social
        // turn does nothing in this test.
        last_turn: Some(driver::unix_now()),
        satisfaction: Satisfaction::default(),
        applied: None,
        salt: 0,
    }
}

/// A hall holding two bot corps: the hunter's and Bob's.
fn hall() -> Arc<Hall> {
    let hall = Hall::new(60).with_every(1 << 40);
    {
        let mut st = hall.lock();
        for (corp, ceo, name) in [(CORP, HUNTER, "Hunter1"), (BOB_CORP, BOB, "Bob")] {
            st.members.insert(ceo, member(name, corp));
            st.corps.insert(
                corp,
                CorpMind {
                    id: corp,
                    ceo: Some(ceo),
                    profile: Profile::of("pirate", Band::Low),
                    founded: 0,
                    alliance: None,
                    name: String::new(),
                    applied_alliance: None,
                    salt: 0,
                },
            );
        }
    }
    Arc::new(hall)
}

fn hunter_config(addr: std::net::SocketAddr, security: f64, hall: &Arc<Hall>) -> BotConfig {
    let now = driver::unix_now();
    let mut mind = Mind::seeded("Hunter1", 1, TABLE, &Signals::default(), now);
    mind.career = "pirate".to_owned();
    mind.since = now;
    mind.traits.0 = [0.5; TRAIT_COUNT];
    mind.traits.0[Trait::Aggression as usize] = 1.0;
    let mut soc = Society::new(mind, TABLE, Signals::default());
    soc.decide_every = Duration::from_secs(3_600);
    soc.hall = Some(Arc::clone(hall));
    let mut config = BotConfig::new("Hunter1", addr, Pace::Fast).with_society(soc);
    config.chat = false;
    config.policy.pvp = Some(Arc::new(doctrine(security)));
    config
}

/// A plain connection in a Rifter, undocked into the sandbox's system.
fn in_space(addr: std::net::SocketAddr, name: &str) -> eve_bot::Conn {
    let mut c = login(addr, name);
    c.undock().expect("undocks");
    c.enter_space(SYSTEM).expect("the state");
    c
}

/// The row is registered, not a civilian, opt-in, and the career that
/// plays it is the pirate.
#[test]
fn the_hunter_row_is_registered_opt_in_and_plays_the_pirate() {
    let row = eve_bot::roles::parse(hunt::ROLE.name()).expect("--loop hunter parses");
    assert!(std::ptr::eq(row, &hunt::ROLE));
    assert!(!row.civilian);
    assert!(hunt::check_opt_in([row], false).is_err());
    assert!(hunt::check_opt_in([row], true).is_ok());
    let pirate = eve_bot::career::find(eve_bot::career::CAREERS, "pirate").expect("a row");
    assert!(std::ptr::eq(pirate.role.expect("a role"), row));
}

/// **A hunter attacks a pilot it is not allied with and spares one it is.**
/// Alice shares the hunter's corporation; Bob does not. The hunter sets
/// itself up, undocks onto a grid with both, and its guns go on Bob's ship;
/// no lock, orbit or shot of its ever names Alice's. Bob dies (the hunter
/// reads the killmail), and the fight lands in the matrix: Bob's corporation
/// holds the hunter's against it, and the report is recorded once.
#[test]
fn a_hunter_attacks_a_stranger_spares_an_ally_and_the_fight_lands_in_the_matrix() {
    let mut cfg = config();
    cfg.sandbox.characters = pilots();
    let addr = serve(cfg);
    let mut alice = in_space(addr, "Alice");
    let mut bob = in_space(addr, "Bob");
    let hall = hall();
    let bot = driver::spawn(hunter_config(addr, LOW_SEC, &hall)).0;

    let fired = until(Duration::from_secs(120), || {
        let _ = alice.collect_pushes(Duration::from_millis(25));
        let _ = bob.collect_pushes(Duration::from_millis(25));
        bot.log()
            .iter()
            .any(|e| ok(e, ActKind::Fire) && e.ids.get(1) == Some(&BOB_SHIP))
    });
    assert!(fired, "the hunter's guns go on Bob:\n{}", dump(&bot));
    // It set its crimewatch safety red first (merge-d b): on green the
    // server refuses a low-sec attack that would make it a suspect.
    assert!(
        bot.log()
            .iter()
            .any(|e| e.kind == ActKind::Hunt && e.what.contains("safety") && e.verdict.is_ok()),
        "{}",
        dump(&bot)
    );

    // The matrix: Bob's ship dies, the killmail is read, the report is
    // recorded once, and Bob's corporation holds the hunter's corporation
    // against it.
    let landed = until(Duration::from_secs(180), || {
        let _ = alice.collect_pushes(Duration::from_millis(25));
        let _ = bob.collect_pushes(Duration::from_millis(25));
        let st = hall.snapshot();
        !st.recorded.is_empty() && st.relations.get(BOB_CORP, CORP) < 0.0
    });
    let log = bot.log();
    bot.stop();
    let st = hall.snapshot();
    assert!(
        landed,
        "the kill in the matrix: recorded {:?}, relations {:?}\n{}",
        st.recorded,
        st.relations,
        dump(&bot)
    );
    assert_eq!(st.recorded.len(), 1, "one report, one record");
    assert_eq!(
        st.relations.get(CORP, BOB_CORP),
        0.0,
        "the killer's own view of its victim is unmoved"
    );

    // The ally was never touched: no lock, orbit, shot or warp names her.
    let touched: Vec<_> = log
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                ActKind::Lock | ActKind::Orbit | ActKind::Fire | ActKind::Follow
            ) && e.ids.contains(&ALICE_SHIP)
        })
        .collect();
    assert!(touched.is_empty(), "Alice was spared: {touched:?}");
    // The settle act counted the kill on its line.
    assert!(
        log.iter().any(|e| e.kind == ActKind::Hunt
            && e.what.contains("settle")
            && e.what.contains("1 kills")
            && e.verdict.is_ok()),
        "{}",
        dump(&bot)
    );
}

/// **High sec stays safe**: the same grid with the system known to be high
/// sec, and the hunter engages nobody in the time the hostile case needed
/// to fire.
#[test]
fn a_hunter_in_high_sec_attacks_nobody() {
    let mut cfg = config();
    cfg.sandbox.characters = pilots();
    let addr = serve(cfg);
    let mut bob = in_space(addr, "Bob");
    let mut alice = in_space(addr, "Alice");
    let hall = hall();
    let bot = driver::spawn(hunter_config(addr, HIGH_SEC, &hall)).0;

    // The hunter undocks and is on the grid; give it a long look.
    let undocked = until(Duration::from_secs(120), || {
        let _ = alice.collect_pushes(Duration::from_millis(25));
        let _ = bob.collect_pushes(Duration::from_millis(25));
        bot.log().iter().any(|e| ok(e, ActKind::Undock))
    });
    assert!(undocked, "{}", dump(&bot));
    let _ = until(Duration::from_secs(8), || {
        let _ = bob.collect_pushes(Duration::from_millis(25));
        false
    });
    let log = bot.log();
    bot.stop();
    let aimed: Vec<_> = log
        .iter()
        .filter(|e| {
            matches!(e.kind, ActKind::Lock | ActKind::Orbit | ActKind::Fire)
                && (e.ids.contains(&BOB_SHIP) || e.ids.contains(&ALICE_SHIP))
        })
        .collect();
    assert!(aimed.is_empty(), "nobody is prey in high sec: {aimed:?}");
}

/// **The directional scan**, the way the client calls it
/// (`ConeScan(angle, range, x, y, z)` on the system scan manager): the
/// sphere sees the ships in the system, a narrow cone away from them
/// sees none, and a cone at one sees it. And [`hunt::sweep`], on top of it,
/// counts pilots' ships and leaves out those already on the grid.
#[test]
fn the_directional_scan_finds_ships_by_cone_and_the_sweep_counts_pilots() {
    let mut cfg = config();
    cfg.sandbox.characters = pilots();
    let addr = serve(cfg);
    let mut bob = in_space(addr, "Bob");
    let mut alice = in_space(addr, "Alice");
    // Where each is, from Alice's own view of the park.
    let _ = alice.collect_pushes(Duration::from_millis(200));
    let _ = bob.collect_pushes(Duration::from_millis(200));
    let ego = alice
        .world_now()
        .ego_ball()
        .expect("her ball")
        .motion
        .position;
    let bob_at = alice
        .world_now()
        .get(BOB_SHIP)
        .expect("Bob is on her grid")
        .motion
        .position;
    let manager = alice.system_scan_manager().expect("the scan manager");
    let sphere = alice
        .directional_scan(&manager, std::f64::consts::TAU, 1.0e12, [1.0, 0.0, 0.0])
        .expect("a sphere scan");
    assert!(
        sphere
            .iter()
            .any(|r| r.id == BOB_SHIP && r.type_id == RIFTER),
        "{sphere:?}"
    );
    assert!(
        !sphere.iter().any(|r| r.id == ALICE_SHIP),
        "the scanner's own ship is not in the answer"
    );
    let toward = [bob_at[0] - ego[0], bob_at[1] - ego[1], bob_at[2] - ego[2]];
    let reach = (toward.iter().map(|v| v * v).sum::<f64>()).sqrt() + 1_000.0;
    let narrow = hunt::DSCAN_NARROW.to_radians();
    let at_him = alice
        .directional_scan(&manager, narrow, reach, toward)
        .expect("a cone at Bob");
    assert!(at_him.iter().any(|r| r.id == BOB_SHIP), "{at_him:?}");
    let away = [-toward[0], -toward[1], -toward[2]];
    let behind = alice
        .directional_scan(&manager, narrow, reach, away)
        .expect("a cone away from Bob");
    assert!(
        !behind.iter().any(|r| r.id == BOB_SHIP),
        "a narrow cone the other way does not see him: {behind:?}"
    );

    // The sweep: Bob is a pilot's ship; known to be on the grid, he is
    // not counted; a belt placed where he is "sees" him.
    let d = doctrine(LOW_SEC);
    let known = std::collections::BTreeSet::new();
    let belt = (77_i64, bob_at);
    let s = hunt::sweep(&mut alice, &d, &[belt], ego, SYSTEM, &known).expect("a sweep");
    assert!(s.ships >= 1, "{s:?}");
    assert_eq!(s.toward, vec![(77, 1)], "the cone at the belt sees Bob");
    let known = std::collections::BTreeSet::from([BOB_SHIP]);
    let s = hunt::sweep(&mut alice, &d, &[belt], ego, SYSTEM, &known).expect("a sweep");
    assert_eq!(s.ships, 0, "Bob is on the grid already: {s:?}");
    assert!(s.toward.is_empty());
}

/// Two systems with an invented gate pair, a station each: the same shape
/// as the travel lane's fixture.
const SYSTEM_B: i64 = 30_000_007;
const STATION_B: i64 = 60_900_007;
const GATE_A: i64 = 50_900_007;
const GATE_B: i64 = 50_900_008;
const GATE_TYPE: i64 = 990_400_001;

fn catalogue() -> eve_server::celestial::Catalogue {
    let home = Character::default();
    let (a, station_a) = (SYSTEM, home.station_id.expect("docked"));
    let (constellation, region) = (home.constellation_id, home.region_id);
    let station_type = home.station_type_id;
    eve_server::celestial::Catalogue::parse(&format!(
        "#system\t{a}\t{constellation}\t{region}\n\
         #system\t{SYSTEM_B}\t{constellation}\t{region}\n\
         station\t{station_a}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{a}\t0\t0\n\
         gate\t{GATE_A}\t{GATE_TYPE}\t2473.0\t1000000.0\t0.0\t0.0\t{a}\t{GATE_B}\t{SYSTEM_B}\n\
         station\t{STATION_B}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{SYSTEM_B}\t0\t0\n\
         gate\t{GATE_B}\t{GATE_TYPE}\t2473.0\t0.0\t0.0\t-1000000.0\t{SYSTEM_B}\t{GATE_A}\t{a}\n"
    ))
}

/// **A hunter roams**: docked in a lawful system, the roam rule names the
/// station in the low-sec system a jump away, and [`hunt::roam`] flies the
/// gates and docks there -- at any security, which the errand floor would
/// have refused.
#[test]
fn a_hunter_roams_over_the_gates_to_a_low_sec_station() {
    let mut cfg = config();
    cfg.sandbox.characters = vec![
        pilot(HUNTER, "Hunter1", HUNTER_SHIP),
        // Seeded at the far station, so this world knows it.
        Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: ALICE_SHIP,
            station_id: Some(STATION_B),
            solar_system_id: Some(SYSTEM_B),
            ..Character::default()
        },
    ];
    cfg.sandbox.celestials = catalogue();
    let addr = serve(cfg);
    let map = eve_bot::travel::Map::from_catalogue(&catalogue(), None)
        .with_security(SYSTEM, HIGH_SEC)
        .with_security(SYSTEM_B, LOW_SEC);
    let d = Ground::new()
        .with_security(SYSTEM, HIGH_SEC)
        .with_security(SYSTEM_B, LOW_SEC);
    assert_eq!(
        hunt::roam_target(&map, &d, SYSTEM, 1.0, 0, &[]),
        Some(STATION_B),
        "the low-sec station one jump away"
    );
    assert_eq!(
        hunt::roam_target(&map, &d, SYSTEM_B, 1.0, 0, &[]),
        None,
        "and from there nowhere else to hunt"
    );
    let mut c = login(addr, "Hunter1");
    let system = hunt::roam(&mut c, &map, STATION_B).expect("the roam flies");
    assert_eq!(system, Some(SYSTEM_B));
    assert_eq!(c.session().station_id(), Some(STATION_B), "docked there");
}
