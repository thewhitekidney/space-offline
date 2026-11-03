//! **The PI farmer makes P2** (FU-PI2) over a socket.
//!
//! 1. One planet holds both resources of an invented P2 recipe: the
//!    farmer founds a P2 colony (two extraction units, two basic factories,
//!    an advanced factory, five routes), the server's simulation -- run at
//!    the game clock's fastest scale -- fills the command centre with P2,
//!    and the next visit launches it and sells it into a bid.
//! 2. Two planets, one resource each: the farmer founds a P1 colony on the
//!    first, then (one more colony allowed) a P2 colony on the second whose
//!    missing P1 comes in through the customs office, and feeds it from the
//!    hangar.
//!
//! **Synthetic furniture only** (the no-client-data rule): invented planet
//! types, planets, pin types, schematics, commodities and office type. The
//! one client id is the link type, [`pi::LINK_TYPE`] (2280 [V]): the server
//! builds every link as it.

use crate::common::{config, gm, login, serve};
use eve_bot::builder::SellLine;
use eve_bot::pi;
use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::model::Character;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

// Invented ids and numbers only.
const SELLER: i64 = 90_000_682;
const PT: i64 = 990_680_001;
const PT_A: i64 = 990_680_002;
const PT_B: i64 = 990_680_003;
const CC: i64 = 990_680_011;
const ECU: i64 = 990_680_012;
const BASIC: i64 = 990_680_015;
const ADVANCED: i64 = 990_680_016;
const PAD: i64 = 990_680_017;
const OFFICE: i64 = 990_680_019;
const RES_A: i64 = 990_680_021;
const RES_B: i64 = 990_680_022;
const P1_A: i64 = 990_680_031;
const P1_B: i64 = 990_680_032;
const P2: i64 = 990_680_041;
const S_A: i64 = 990_681;
const S_B: i64 = 990_682;
const S_AB: i64 = 990_685;
const PLANET: i64 = 40_968_001;
const PLANET_A: i64 = 40_968_002;
const PLANET_B: i64 = 40_968_003;
const PLANET_RADIUS: f64 = 3_000_000.0;
/// The fixture frigate of `typedata::FIXTURE`: a hull with a hold.
const FRIGATE: i64 = 990_000_001;
const WALLET: f64 = 10_000_000.0;
const CC_PRICE: f64 = 1_000.0;
const BID: f64 = 50.0;
const FEED: i64 = 40;
/// The game clock's fastest scale: an extractor's 15-minute cycle is
/// 15 s of the test.
const SCALE: u32 = eve_server::game_clock::MAX_SCALE;

fn home() -> Character {
    Character::default()
}

/// Each test's own system and station (invented): the farmer remembers a
/// system's planets for the whole process, so two tests must not share one.
#[derive(Clone, Copy)]
struct Where {
    system: i64,
    station: i64,
}

const ONE: Where = Where {
    system: 30_068_001,
    station: 60_968_001,
};
const TWO: Where = Where {
    system: 30_068_002,
    station: 60_968_002,
};

fn catalogue(at: Where, planets: &[(i64, i64)]) -> eve_server::celestial::Catalogue {
    let (a, st) = (at.system, at.station);
    let (constellation, region) = (home().constellation_id, home().region_id);
    let station_type = home().station_type_id;
    let mut text = format!(
        "#system\t{a}\t{constellation}\t{region}\n\
         station\t{st}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{a}\t0\t0\n"
    );
    for (i, (p, t)) in planets.iter().enumerate() {
        let x = 50_000_000_000.0 * (i + 1) as f64;
        text.push_str(&format!(
            "planet\t{p}\t{t}\t{PLANET_RADIUS}\t{x}\t0.0\t0.0\t{a}\t0\t0\n"
        ));
    }
    eve_server::celestial::Catalogue::parse(&text)
}

/// The PI table: pins light enough for a level-0 centre to carry a P2
/// colony, two P0 -> P1 recipes, one P1 + P1 -> P2 recipe that only the
/// advanced factory runs, a launchpad and a customs office; `PT` holds
/// both resources, `PT_A` and `PT_B` one each.
fn statics() -> eve_server::planet::Statics {
    let link = pi::LINK_TYPE;
    eve_server::planet::Statics::parse(&format!(
        "pintype\t{CC}\t1027\t500.0\t0.0\t0\t48=1675.0;11=6000.0;1639=3.0\n\
         pintype\t{ECU}\t1063\t0.0\t1000.0\t0\t49=100.0;15=500.0;1690=10.0;1691=50.0;1689=0.3\n\
         pintype\t{BASIC}\t1028\t0.0\t2000.0\t0\t49=100.0;15=100.0\n\
         pintype\t{ADVANCED}\t1028\t0.0\t5000.0\t0\t49=150.0;15=150.0\n\
         pintype\t{PAD}\t1030\t10000.0\t9000.0\t0\t49=300.0;15=200.0;1638=0.5;1639=1.0\n\
         pintype\t{link}\t1036\t0.0\t0.0\t0\t49=15.0;15=10.0;1631=1250.0;1633=0.15;1634=0.2;1635=1.4;1636=1.2\n\
         schematic\t{S_A}\t60\t{BASIC}\t{RES_A}=10\t{P1_A}=5\n\
         schematic\t{S_B}\t60\t{BASIC}\t{RES_B}=10\t{P1_B}=5\n\
         schematic\t{S_AB}\t60\t{ADVANCED}\t{P1_A}=5;{P1_B}=5\t{P2}=1\n\
         resource\t{PT}\t{RES_A}=1.0;{RES_B}=1.0\n\
         resource\t{PT_A}\t{RES_A}=1.0\n\
         resource\t{PT_B}\t{RES_B}=0.5\n\
         itemtype\t{CC}\t1027\t41\t10.0\t500.0\tFixture PI Centre\n\
         itemtype\t{RES_A}\t1032\t42\t0.005\t0.0\tFixture P0 A\n\
         itemtype\t{RES_B}\t1032\t42\t0.005\t0.0\tFixture P0 B\n\
         itemtype\t{P1_A}\t1042\t43\t0.1\t0.0\tFixture P1 A\n\
         itemtype\t{P1_B}\t1042\t43\t0.1\t0.0\tFixture P1 B\n\
         itemtype\t{P2}\t1034\t43\t0.2\t0.0\tFixture P2\n\
         customsoffice\t{OFFICE}\t35000.0\t8000.0\t0.1\n"
    ))
    .expect("the invented rows parse")
}

/// The content: the PI table, and the skill table with a row for
/// Interplanetary Consolidation (invented rank and attributes; the id is
/// [`pi::SKILL_INTERPLANETARY_CONSOLIDATION`]) so `/giveskill` knows it.
fn content() -> Content {
    let mut c = Content::fixture();
    let skills = format!(
        "{}skill\t{}\t990680100\t1\t165\t166\t0.01\t\tFixture PI Skill\n",
        eve_server::skill::FIXTURE,
        pi::SKILL_INTERPLANETARY_CONSOLIDATION
    );
    c.insert(
        eve_server::skill::TABLE.name,
        eve_server::skill::Table::parse(&skills).expect("the skill rows parse"),
    );
    c.insert(
        eve_server::planet::STATICS.name,
        eve_server::planet::Installed::build(statics()),
    );
    c
}

fn start(at: Where, farmer: (i64, &str), planets: &[(i64, i64)], scale: u32) -> SocketAddr {
    let mut c = config();
    c.sandbox.characters = vec![
        Character {
            character_id: farmer.0,
            name: farmer.1.to_owned(),
            ship_id: farmer.0 + 10_000_000,
            balance: WALLET,
            station_id: Some(at.station),
            solar_system_id: Some(at.system),
            ..home()
        },
        Character {
            character_id: SELLER,
            name: "PiSeller2".to_owned(),
            ship_id: 100_000_682,
            balance: WALLET,
            station_id: Some(at.station),
            solar_system_id: Some(at.system),
            ..home()
        },
    ];
    c.sandbox.celestials = catalogue(at, planets);
    c.content = Some(Arc::new(content()));
    c.time_scale = eve_server::game_clock::GameClock::new(scale).expect("a scale");
    serve(c)
}

/// The seller lists `n` command centres and bids for `bid_type`.
fn seller(addr: SocketAddr, at: Where, n: i64, bid_type: i64) {
    let mut s = login(addr, "PiSeller2");
    gm(&mut s, &format!("/createitem {CC} {n}"));
    let centre = s
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == CC)
        .expect("the seller's centres");
    s.sell(
        &[SellLine {
            station_id: at.station,
            type_id: CC,
            item_id: centre.item_id,
            price: CC_PRICE,
            quantity: n,
        }],
        eve_bot::escort::LIST_DAYS,
        0.0,
    )
    .expect("the seller lists the centres");
    s.buy(
        at.station,
        bid_type,
        BID,
        100,
        eve_bot::escort::LIST_DAYS,
        0.0,
    )
    .expect("the seller bids");
}

/// The farmer in a frigate (its hold takes the fixture centre).
fn farmer(addr: SocketAddr, name: &str) -> eve_bot::Conn {
    let mut f = login(addr, name);
    gm(&mut f, &format!("/createitem {FRIGATE}"));
    let hull = f
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE)
        .expect("the frigate");
    f.board(hull.item_id).expect("boards the frigate");
    f
}

fn colony(f: &mut eve_bot::Conn, planet: i64) -> pi::ColonyView {
    let info = f
        .moniker_call(
            "planetMgr",
            PyValue::Int(planet),
            "GetPlanetInfo",
            vec![],
            vec![],
        )
        .expect("the planet reads")
        .value
        .expect("an answer");
    pi::read_colony(&info).expect("a colony")
}

/// **One planet, P2 made and sold.**
#[test]
fn a_farmer_founds_a_p2_colony_whose_p2_is_launched_and_sold() {
    let name = "Farmer2";
    let addr = start(ONE, (90_000_681, name), &[(PLANET, PT)], SCALE);
    seller(addr, ONE, 1, P2);
    let mut f = farmer(addr, name);
    let content = content();

    // 1. Look. 2. Found the P2 colony.
    let first = pi::run(&mut f, Some(&content), name, 0).expect("a first visit");
    assert_eq!(first.surveyed, Some(1), "{}", first.summary());
    let second = pi::run(&mut f, Some(&content), name, 0).expect("a second visit");
    let (planet, level, heads) = second
        .founded
        .unwrap_or_else(|| panic!("{}", second.summary()));
    assert_eq!(
        (planet, level, second.p2),
        (PLANET, 0, Some(P2)),
        "{}",
        second.summary()
    );
    assert!(heads >= 1);
    assert_eq!(
        f.session().station_id(),
        Some(ONE.station),
        "docked at home"
    );

    let view = colony(&mut f, PLANET);
    let count = |t: i64| view.pins.iter().filter(|p| p.type_id == t).count();
    assert_eq!(
        (count(CC), count(ECU), count(BASIC), count(ADVANCED)),
        (1, 2, 2, 1)
    );
    let adv = view.pins.iter().find(|p| p.type_id == ADVANCED).unwrap();
    assert_eq!(adv.schematic_id, Some(S_AB));
    let mut programs: Vec<i64> = view.pins.iter().filter_map(|p| p.program_type).collect();
    programs.sort();
    assert_eq!(programs, vec![RES_A, RES_B]);

    // The server's simulation, at the clock's fastest scale: P2 reaches the
    // centre after the first extraction cycle and two factory cycles.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let view = colony(&mut f, PLANET);
        let cc = view.pins.iter().find(|p| p.type_id == CC).unwrap();
        if cc.contents.get(&P2).is_some_and(|q| *q > 0) {
            break;
        }
        assert!(Instant::now() < deadline, "no P2 in the centre: {view:?}");
        std::thread::sleep(Duration::from_millis(500));
    }

    // 3. Launch it and sell it into the bid.
    let third = pi::run(&mut f, Some(&content), name, 0).expect("a third visit");
    assert!(third.launched > 0, "{}", third.summary());
    assert!(
        third
            .sales
            .iter()
            .any(|s| matches!(s, pi::Sale::Bid(l) if l.type_id == P2)),
        "{}",
        third.summary()
    );
    assert!(third.after > third.before, "{}", third.summary());
}

/// **Two planets: P1 on one, an import-fed P2 colony on the other.**
#[test]
fn a_second_colony_makes_p2_from_the_firsts_p1_through_the_customs_office() {
    let name = "Farmer3";
    let addr = start(
        TWO,
        (90_000_683, name),
        &[(PLANET_A, PT_A), (PLANET_B, PT_B)],
        1,
    );
    seller(addr, TWO, 2, P2);
    let mut f = farmer(addr, name);
    let content = content();

    let first = pi::run(&mut f, Some(&content), name, 0).expect("a first visit");
    assert_eq!(first.surveyed, Some(2), "{}", first.summary());

    // No P2 is reachable yet: B6's P1 colony on the better planet.
    let second = pi::run(&mut f, Some(&content), name, 0).expect("a second visit");
    assert_eq!(
        second.founded.map(|x| x.0),
        Some(PLANET_A),
        "{}",
        second.summary()
    );
    assert_eq!(second.p2, None);

    // One more colony allowed: the P2 colony, fed P1_A by import.
    gm(
        &mut f,
        &format!("/giveskill {} 1", pi::SKILL_INTERPLANETARY_CONSOLIDATION),
    );
    let third = pi::run(&mut f, Some(&content), name, 0).expect("a third visit");
    assert_eq!(
        third.founded.map(|x| x.0),
        Some(PLANET_B),
        "{}",
        third.summary()
    );
    assert_eq!(third.p2, Some(P2), "{}", third.summary());
    let view = colony(&mut f, PLANET_B);
    let types: Vec<i64> = view.pins.iter().map(|p| p.type_id).collect();
    for t in [CC, ECU, BASIC, ADVANCED, PAD] {
        assert!(types.contains(&t), "{types:?}");
    }

    // P1_A in the hangar (what colony A launches): fed, not sold.
    gm(&mut f, &format!("/createitem {P1_A} {FEED}"));
    let fourth = pi::run(&mut f, Some(&content), name, 0).expect("a fourth visit");
    assert_eq!(fourth.fed, FEED, "{}", fourth.summary());
    assert!(fourth.sales.is_empty(), "{}", fourth.summary());
    assert!(
        fourth.after < fourth.before,
        "the import tax: {}",
        fourth.summary()
    );
    assert_eq!(
        f.session().station_id(),
        Some(TWO.station),
        "docked at home"
    );
    let view = colony(&mut f, PLANET_B);
    let on_planet: i64 = view
        .pins
        .iter()
        .map(|p| p.contents.get(&P1_A).copied().unwrap_or(0))
        .sum();
    assert_eq!(on_planet, FEED, "{view:?}");
    let left: i64 = f
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .chain(f.cargo().expect("the hold lists"))
        .filter(|r| r.type_id == P1_A)
        .map(|r| r.quantity)
        .sum();
    assert_eq!(left, 0);
}
