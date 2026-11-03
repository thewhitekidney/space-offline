//! HK4 over a real socket: a pilot docked with a hull, a probe launcher,
//! probes and an analyzer runs one [`hacker::run`] trip -- outfit, undock,
//! scan the data signature down, warp, lock the can, activate the
//! analyzer, play the game the server opens to its end with the solver,
//! open the won can, fly home and dock.
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **Invented rows only** (the no-client-data rule): SOC-B3's invented
//! home system, region, pirate and data dungeon (`probe.rs`), whose one
//! object is an invented hackable can type (`hackable`, a tier and an
//! invented skill); an invented analyzer with an invented virus, far
//! stronger than any board can hurt, and an invented short cycle. The
//! server plays its own shipped board rules; nothing here names one of
//! their numbers, and **no `[A]` value is an expected value**: the
//! assertions are about the trip's shape and the game's end.

use super::common::{SYSTEM, config, gm, login, pilot, serve};
use eve_bot::explore_roam::{self as roam, Nerve};
use eve_bot::hacker;
use eve_bot::probe::{self, Kind};
use eve_bot::travel::{self, Map, Trips};
use eve_server::celestial::{Band, Universe};
use eve_server::dungeon_data::{self, Dungeon, Entry, Object, Room};
use eve_server::model::Character;
use eve_server::npc_data::{self, RegionPirate};
use eve_server::population::{self, Diffs, Row};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

const HACKER: i64 = 90_000_001;
const REGION: i64 = 19_054_001;
const CONSTELLATION: i64 = 29_054_001;
const PIRATE: i64 = 599_054;
const DUNGEON: i64 = 999_541;
/// An invented hackable can, its invented group and the invented skill it
/// and the analyzer require.
const CAN: i64 = 990_541;
const CAN_GROUP: i64 = 990_120;
const SKILL: i64 = 990_217_054;
const LAUNCHER: i64 = 990_542_001;
const PROBE: i64 = 990_542_010;
const ANALYZER: i64 = 990_542_020;
/// `typedata::FIXTURE`'s frigate: three high and three mid slots.
const FRIGATE: i64 = 990_000_001;
/// The analyzer's invented numbers: range (m), cycle (ms), virus.
const RANGE: f64 = 9_000.0;
const CYCLE_MS: f64 = 2_000.0;
const VIRUS: (f64, f64, f64) = (900.0, 150.0, 3.0);
const DATA: i64 = dungeon_data::FACTION_ARCHETYPES[2];

fn types() -> String {
    let launcher_group = probe::GROUP_LAUNCHER;
    let probe_group = probe::GROUP_PROBE;
    let factor = probe::RANGE_FACTOR;
    let analyzer_group = hacker::GROUP_ANALYZER;
    let (effect_name, effect) = hacker::DO_HACKING;
    let (coh, strength, slots) = VIRUS;
    format!(
        "{}\
type\t{LAUNCHER}\t{launcher_group}\t7\t1\t1\t5.0\t0.8\t0.0\t1.0\t0\t12,16\t30=1.0;50=10.0;604={probe_group}.0\tFixture Probe Launcher\n\
type\t{PROBE}\t{probe_group}\t8\t1\t1\t0.1\t0.0\t1.0\t1.0\t0\t\t281=3600000.0;1370=0.5;1371=50.0;1373={factor}\tFixture Site Probe\n\
type\t{CAN}\t{CAN_GROUP}\t2\t1\t1\t1.0\t0.0\t0.0\t500.0\t0\t\t182={SKILL}.0;1919=1.0;1927=1.0\tFixture Hackable Can\n\
type\t{ANALYZER}\t{analyzer_group}\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{effect}\t13,16,{effect}\t6=1.0;30=1.0;50=5.0;54={RANGE};73={CYCLE_MS};182={SKILL}.0;1909={coh};1910={strength};1911={slots}\tFixture Analyzer\n\
effect\t{effect}\t2\t0\t73\t6\t54\t0\t0\t{effect_name}\teffects.FixtureHack\n",
        eve_server::typedata::FIXTURE
    )
}

fn universe() -> Universe {
    let system = format!(
        "{SYSTEM}\t{REGION}\t{CONSTELLATION}\t\t\t0.9\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture home"
    );
    Universe::parse(
        &[system],
        &[format!("{REGION}\t\t\t\t\tFixture region")],
        &[format!(
            "{CONSTELLATION}\t{REGION}\t\t\t\tFixture constellation"
        )],
    )
    .expect("the invented rows parse")
}

/// One data site: a room with one hackable can.
fn dungeons() -> dungeon_data::Table {
    let d = Dungeon {
        id: DUNGEON,
        archetype: DATA,
        faction: Some(PIRATE),
        difficulty: Some(1),
        entry_type: Some(dungeon_data::ENTRY_SIGNATURE),
        name_id: Some(DUNGEON + 1),
        entry_object: Some(1),
        rooms: vec![Room {
            id: 1,
            ..Room::default()
        }],
        objects: vec![Object {
            room: 1,
            id: 1,
            type_id: CAN,
            pos: [4_000.0, 0.0, 0.0],
            radius: Some(500.0),
            ..Object::default()
        }],
        ..Dungeon::default()
    };
    dungeon_data::Table {
        dungeons: BTreeMap::from([(DUNGEON, d)]),
        ..dungeon_data::Table::default()
    }
}

fn npcs() -> npc_data::Table {
    let mut t = npc_data::Table::default();
    t.regions.insert(
        REGION,
        vec![RegionPirate {
            faction_id: PIRATE,
            weight: 1.0,
            rule: npc_data::Rule::Override,
        }],
    );
    t
}

fn sites() -> population::Table {
    population::Table::new(vec![Row {
        key: "fx_data_high".to_owned(),
        band: Band::High,
        archetypes: vec![DATA],
        entry: Entry::Signature,
        count: 1.0,
        difficulties: Diffs::In(vec![1]),
        respawn_s: 600,
        ttl_s: 172_800,
    }])
    .expect("one row")
}

/// The invented skill the analyzer needs, as a skill the GM can grant (the
/// skill gate reads the sheet, and `/giveskill` finds only skills the table
/// holds).
fn skills() -> eve_server::skill::Table {
    eve_server::skill::Table::parse(&format!(
        "{}skill\t{SKILL}\t730100\t1\t164\t166\t0.01\t\tInvented Analyzer Skill\n",
        eve_server::skill::FIXTURE
    ))
    .expect("the skill row parses")
}

fn start() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![pilot(HACKER, "Hacker", 100_000_001)];
    let mut content = eve_server::content::Content::fixture();
    content.insert(eve_server::skill::TABLE.name, skills());
    let table = eve_server::typedata::Table::parse(&types()).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    content.insert("universe", universe());
    content.insert("dungeon_data", dungeons());
    content.insert("npc_data", npcs());
    content.insert("population", sites());
    config.content = Some(Arc::new(content));
    serve(config)
}

/// **The career's trip** (R-HACK HK4 test 7): ready, outfit, undock, scan
/// down, warp, lock, activate, the game played to a win, the can opened,
/// home and docked.
#[test]
fn a_hacker_scans_a_data_site_down_hacks_its_can_and_docks_again() {
    let addr = start();
    let mut c = login(addr, "Hacker");
    gm(&mut c, &format!("/createitem {FRIGATE}"));
    gm(&mut c, &format!("/createitem {LAUNCHER}"));
    gm(
        &mut c,
        &format!("/createitem {PROBE} {}", probe::SETUP_PROBES),
    );
    gm(&mut c, &format!("/giveskill {SKILL} 1"));
    gm(&mut c, &format!("/createitem {ANALYZER}"));
    let home = c.session().station_id().expect("docked");

    let trip = hacker::run(&mut c, "Hacker", 0).expect("one trip");
    let said = trip.summary();
    assert_eq!(trip.lacking, None, "{said}");
    assert_eq!(trip.sites.len(), 1, "{said}");
    let game = trip.games.first().expect("a can was played");
    assert!(game.won, "{said}");
    assert_eq!(game.type_id, Some(CAN), "{said}");
    assert!(game.turns > 0 && game.coherence > 0, "{said}");
    assert!(said.contains("[hack] can"), "{said}");
    assert_eq!(c.session().station_id(), Some(home), "{said}");
    let fitted = c.fitted().expect("the fitting lists");
    assert!(fitted.iter().any(hacker::is_analyzer), "{fitted:?}");
}

// ---- Y-ROAM: the same trip, one jump out --------------------------------------------
//
// Two invented systems: the home (high-sec, no site) and one a gate away
// (low-sec, one data site: the population row is for the low band only).
// The gates are the travel lane's shape: a thousand kilometres out, beyond
// the client's warp floor.

/// The far system, and the invented gate pair.
const FAR: i64 = 30_000_003;
const GATE_HOME: i64 = 50_954_001;
const GATE_FAR: i64 = 50_954_002;
const GATE_TYPE: i64 = 990_541_001;
/// The far system's invented security: low-sec.
const FAR_SECURITY: f64 = 0.3;
/// The home's: high-sec.
const HOME_SECURITY: f64 = 0.9;
const LURKER: i64 = 90_000_002;
/// A corporation that is not the sandbox default's.
const OTHER_CORP: i64 = 98_000_541;

fn home_station() -> i64 {
    Character::default().station_id.expect("docked by default")
}

fn catalogue() -> eve_server::celestial::Catalogue {
    let c = Character::default();
    let (constellation, region) = (c.constellation_id, c.region_id);
    let station_type = c.station_type_id;
    eve_server::celestial::Catalogue::parse(&format!(
        "#system\t{SYSTEM}\t{constellation}\t{region}\n\
         #system\t{FAR}\t{constellation}\t{region}\n\
         station\t{}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{SYSTEM}\t0\t0\n\
         gate\t{GATE_HOME}\t{GATE_TYPE}\t2473.0\t1000000.0\t0.0\t0.0\t{SYSTEM}\t{GATE_FAR}\t{FAR}\n\
         gate\t{GATE_FAR}\t{GATE_TYPE}\t2473.0\t0.0\t0.0\t-1000000.0\t{FAR}\t{GATE_HOME}\t{SYSTEM}\n",
        home_station()
    ))
}

fn two_universe() -> Universe {
    let home = format!(
        "{SYSTEM}\t{REGION}\t{CONSTELLATION}\t\t\t{HOME_SECURITY}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture home"
    );
    let far = format!(
        "{FAR}\t{REGION}\t{CONSTELLATION}\t\t\t{FAR_SECURITY}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture far"
    );
    Universe::parse(
        &[home, far],
        &[format!("{REGION}\t\t\t\t\tFixture region")],
        &[format!(
            "{CONSTELLATION}\t{REGION}\t\t\t\tFixture constellation"
        )],
    )
    .expect("the invented rows parse")
}

/// One data signature in every low-sec system, none in high-sec.
fn low_sites() -> population::Table {
    population::Table::new(vec![Row {
        key: "fx_data_low".to_owned(),
        band: Band::Low,
        archetypes: vec![DATA],
        entry: Entry::Signature,
        count: 1.0,
        difficulties: Diffs::In(vec![1]),
        respawn_s: 600,
        ttl_s: 172_800,
    }])
    .expect("one row")
}

fn start_roam(characters: Vec<Character>) -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = characters;
    config.sandbox.celestials = catalogue();
    let mut content = eve_server::content::Content::fixture();
    content.insert(eve_server::skill::TABLE.name, skills());
    let table = eve_server::typedata::Table::parse(&types()).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    content.insert("universe", two_universe());
    content.insert("dungeon_data", dungeons());
    content.insert("npc_data", npcs());
    content.insert("population", low_sites());
    config.content = Some(Arc::new(content));
    serve(config)
}

fn roam_map() -> Map {
    Map::from_catalogue(&catalogue(), None)
        .with_security(SYSTEM, HOME_SECURITY)
        .with_security(FAR, FAR_SECURITY)
}

/// A nerve that flies low-sec and is not put off by company, but is put
/// off by a stranger on the grid: between the named cuts.
fn steady() -> Nerve {
    let v = (roam::NULL_NERVE + roam::FEARLESS_NERVE) / 2.0;
    let n = Nerve {
        risk: v,
        aggression: v,
    };
    assert!(roam::floor_for(n) <= roam::LOW_FLOOR, "flies low-sec");
    assert!(
        !roam::avoid(roam::Threat::Company(1), n),
        "not put off by company"
    );
    assert!(
        roam::avoid(roam::Threat::Hostile(1), n),
        "put off by a stranger on the grid"
    );
    n
}

fn gear(c: &mut eve_bot::Conn) {
    gm(c, &format!("/createitem {FRIGATE}"));
    gm(c, &format!("/createitem {LAUNCHER}"));
    gm(c, &format!("/createitem {PROBE} {}", probe::SETUP_PROBES));
    gm(c, &format!("/giveskill {SKILL} 1"));
    gm(c, &format!("/createitem {ANALYZER}"));
}

fn ctx_for<'a>(map: &'a Map, home: i64) -> roam::Ctx<'a> {
    roam::Ctx {
        trips: Some(Trips {
            map,
            floor: travel::SECURITY_FLOOR,
        }),
        nerve: Some(steady()),
        home: Some(home),
        ..roam::Ctx::default()
    }
}

/// **The brief's socket test**: an explorer docked at home with no site in
/// its own system plans a loop, jumps one gate to the low-sec system,
/// scans the data signature there down, hacks its can, records it on the
/// shared ledger, and flies back through the gate to dock at home. No state
/// is left behind.
#[test]
fn a_roaming_hacker_travels_one_jump_hacks_a_site_there_and_returns() {
    let addr = start_roam(vec![pilot(HACKER, "Roamer", 100_000_001)]);
    let mut c = login(addr, "Roamer");
    gear(&mut c);
    let home = c.session().station_id().expect("docked");
    let map = roam_map();
    let ctx = ctx_for(&map, home);

    let trip = hacker::run_with(&mut c, "Roamer", 0, &ctx).expect("one trip");
    let said = trip.summary();
    assert_eq!(trip.lacking, None, "{said}");
    assert_eq!(trip.route, vec![FAR], "one jump out: {said}");
    assert_eq!(trip.sites.len(), 1, "the far system's site: {said}");
    assert!(trip.avoided.is_empty(), "{said}");
    let game = trip.games.first().expect("a can was played");
    assert!(game.won, "{said}");
    assert_eq!(game.type_id, Some(CAN), "{said}");
    assert!(said.contains("[roam] route"), "{said}");
    assert_eq!(
        c.session().station_id(),
        Some(home),
        "docked at home: {said}"
    );
    assert_eq!(c.session().solar_system_id2(), Some(SYSTEM), "{said}");
    assert!(
        probe::LEDGER
            .of_kind(addr, Kind::Data)
            .iter()
            .any(|(s, t)| *s == FAR && Some(t) == trip.sites.first()),
        "the far site is on the shared ledger"
    );
    assert!(
        roam::get(None, "Roamer", roam::unix_now()).is_none(),
        "the trip's state is cleared at the dock"
    );
    assert!(
        c.cargo()
            .expect("the hold lists")
            .iter()
            .all(|r| r.group_id == probe::GROUP_PROBE),
        "nothing but the spare probes left in the hold"
    );
}

/// **A trip cut off in the far system carries on**: the state a dropped
/// connection left (out, the home visited, a loop with no stops left) and
/// the pilot in space beyond the gate. `run_with` finds the state, works
/// the system it is in, and flies home; the state is cleared.
#[test]
fn a_trip_cut_off_beyond_the_gate_carries_on_and_comes_home() {
    let addr = start_roam(vec![pilot(HACKER, "Resumer", 100_000_001)]);
    let mut c = login(addr, "Resumer");
    gear(&mut c);
    let home = c.session().station_id().expect("docked");
    let map = roam_map();
    // A plain trip fits the ship, then the pilot flies out by hand.
    hacker::run(&mut c, "Resumer", 0).expect("a trip at home");
    c.undock().expect("undocks");
    c.bind_ballpark(SYSTEM).expect("binds");
    probe::jump_route(&mut c, &map, travel::ANY_SECURITY, FAR).expect("through the gate");
    assert_eq!(c.session().solar_system_id(), Some(FAR));

    let mut st = roam::TripState::new("Resumer", home, SYSTEM, roam::unix_now(), 0.0);
    st.visited.push(SYSTEM);
    roam::put(None, &st);
    assert!(roam::resumable("Resumer"));

    let ctx = ctx_for(&map, home);
    let trip = hacker::run_with(&mut c, "Resumer", 0, &ctx).expect("the trip carries on");
    let said = trip.summary();
    assert!(trip.resumed, "{said}");
    assert_eq!(trip.sites.len(), 1, "worked where it was found: {said}");
    assert!(trip.games.iter().all(|g| g.won), "{said}");
    assert_eq!(c.session().station_id(), Some(home), "{said}");
    assert!(!roam::resumable("Resumer"), "the state is cleared");
}

/// **A bot found in space with no trip is flown home** over the gate.
#[test]
fn a_bot_found_in_space_with_no_trip_is_flown_home() {
    let addr = start_roam(vec![pilot(HACKER, "Stray", 100_000_001)]);
    let mut c = login(addr, "Stray");
    gear(&mut c);
    let home = c.session().station_id().expect("docked");
    let map = roam_map();
    hacker::run(&mut c, "Stray", 0).expect("a trip at home");
    c.undock().expect("undocks");
    c.bind_ballpark(SYSTEM).expect("binds");
    probe::jump_route(&mut c, &map, travel::ANY_SECURITY, FAR).expect("through the gate");
    let ctx = ctx_for(&map, home);
    let trip = hacker::run_with(&mut c, "Stray", 0, &ctx).expect("flown home");
    assert!(trip.homed, "{}", trip.summary());
    assert_eq!(c.session().station_id(), Some(home));
}

/// **Hostiles on the gate's grid turn a nervous explorer round**: another
/// corporation's pilot sits at the far gate; the explorer jumps in, looks,
/// leaves the system unworked and comes home with nothing, and the danger
/// is on the shared book for the next one.
#[test]
fn a_stranger_at_the_far_gate_turns_a_steady_explorer_round() {
    let lurker = Character {
        corporation_id: OTHER_CORP,
        ..pilot(LURKER, "Lurker", 100_000_002)
    };
    let addr = start_roam(vec![pilot(HACKER, "Fleer", 100_000_001), lurker]);
    let map = roam_map();
    // The stranger flies to the far gate and stays.
    let mut other = login(addr, "Lurker");
    other.undock().expect("undocks");
    other.bind_ballpark(SYSTEM).expect("binds");
    probe::jump_route(&mut other, &map, travel::ANY_SECURITY, FAR).expect("through the gate");

    let mut c = login(addr, "Fleer");
    gear(&mut c);
    let home = c.session().station_id().expect("docked");
    let ctx = ctx_for(&map, home);
    let trip = hacker::run_with(&mut c, "Fleer", 0, &ctx).expect("one trip");
    let said = trip.summary();
    assert_eq!(trip.route, vec![FAR], "{said}");
    assert!(trip.sites.is_empty(), "left it unworked: {said}");
    assert!(
        matches!(trip.avoided.first(), Some((s, roam::Threat::Hostile(_))) if *s == FAR),
        "{said}"
    );
    assert_eq!(c.session().station_id(), Some(home), "{said}");
    assert!(
        matches!(
            roam::DANGER.recent((addr, FAR), std::time::Instant::now()),
            roam::Threat::Hostile(_)
        ),
        "the next explorer knows"
    );
}

/// **The prober roams too**: with a nerve it leaves a home with nothing new
/// for the system a gate away, and scans and books the data signature
/// there.
#[test]
fn a_roaming_prober_leaves_an_empty_home_and_books_the_far_signature() {
    let addr = start_roam(vec![pilot(HACKER, "Scout", 100_000_001)]);
    let mut c = login(addr, "Scout");
    gm(&mut c, &format!("/createitem {FRIGATE}"));
    gm(&mut c, &format!("/createitem {LAUNCHER}"));
    gm(
        &mut c,
        &format!("/createitem {PROBE} {}", probe::SETUP_PROBES),
    );
    let hull = c
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE)
        .expect("the frigate");
    c.board(hull.item_id).expect("boards");
    let map = roam_map();
    let trips = Trips {
        map: &map,
        floor: travel::SECURITY_FLOOR,
    };
    let ledger = probe::Ledger::new();
    let n = Some(steady());
    // Docked: outfit and remember home.
    probe::explore_with(&mut c, &ledger, Some(trips), n).expect("outfitted");
    c.undock().expect("undocks");
    c.bind_ballpark(SYSTEM).expect("binds");
    // Home has nothing: it roams a gate.
    let first = probe::explore_with(&mut c, &ledger, Some(trips), n).expect("home scanned out");
    assert_eq!(first.found, None, "{}", first.summary());
    assert_eq!(first.jumped_to, Some(FAR), "{}", first.summary());
    assert_eq!(c.session().solar_system_id(), Some(FAR));
    // The far system has the signature.
    let second = probe::explore_with(&mut c, &ledger, Some(trips), n).expect("far scanned");
    let sig = second.found.expect("a signature in the far system");
    assert_eq!(sig.solar_system_id, FAR);
    assert_eq!(sig.kind, Kind::Data);
    assert!(ledger.knows((addr, FAR), &sig.target_id));
}

/// SKILLGATE: an analyzer in the hangar that the skills do not fit yet ends
/// the trip lacking, and the trip says what it waits on.
#[test]
fn an_analyzer_the_skills_do_not_fit_says_what_it_waits_on() {
    let addr = start();
    let mut c = login(addr, "Hacker");
    gm(&mut c, &format!("/createitem {FRIGATE}"));
    gm(&mut c, &format!("/createitem {LAUNCHER}"));
    gm(
        &mut c,
        &format!("/createitem {PROBE} {}", probe::SETUP_PROBES),
    );
    gm(&mut c, &format!("/createitem {ANALYZER}"));

    let trip = hacker::run(&mut c, "Waiter", 0).expect("a trip that does not go out");
    assert_eq!(trip.lacking, Some(hacker::Lack::Analyzer));
    let said = trip.summary();
    assert!(said.contains("waits on skills"), "{said}");
}
