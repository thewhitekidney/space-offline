//! **The PI farmer makes P3** (X-PI3) over a socket.
//!
//! The farmer holds one P2 (what its P2 colony launched), the home
//! station's market sells the other, and a bid waits for the P3. It founds
//! a P3 colony (a factory and a launchpad), buys the missing P2, carries
//! both onto the pad through the customs office (the import tax), the
//! server's simulation -- at the game clock's fastest scale -- makes P3 on
//! the pad, and the next visit exports it through the office (the export
//! tax), docks and sells it into the bid.
//!
//! **Synthetic furniture only** (the no-client-data rule): invented planet
//! type, planet, pin types, schematics, commodities and prices. The one
//! client id is the link type, [`pi::LINK_TYPE`] (2280 [V]).

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
const SELLER: i64 = 90_000_702;
const FARMER: i64 = 90_000_701;
const PT: i64 = 990_710_001;
const PT_B: i64 = 990_710_002;
const ECU: i64 = 990_710_012;
const RES_B: i64 = 990_710_022;
const CC: i64 = 990_710_011;
const BASIC: i64 = 990_710_015;
const ADVANCED: i64 = 990_710_016;
const PAD: i64 = 990_710_017;
const OFFICE: i64 = 990_710_019;
const RES_A: i64 = 990_710_021;
const P1_A: i64 = 990_710_031;
const P1_B: i64 = 990_710_032;
const P2_A: i64 = 990_710_041;
const P2_B: i64 = 990_710_042;
const P3: i64 = 990_710_051;
const S_A: i64 = 990_711;
const S_B: i64 = 990_712;
const S_P2A: i64 = 990_713;
const S_P2B: i64 = 990_714;
const S_P3: i64 = 990_715;
const PLANET: i64 = 40_971_001;
const PLANET_B: i64 = 40_971_002;
const PLANET_RADIUS: f64 = 3_000_000.0;
const SYSTEM: i64 = 30_071_001;
const STATION: i64 = 60_971_001;
/// The fixture frigate of `typedata::FIXTURE`: a hull with a hold.
const FRIGATE: i64 = 990_000_001;
const WALLET: f64 = 10_000_000.0;
const CC_PRICE: f64 = 1_000.0;
/// Reference prices (the server's price oracle): the P3 is worth three
/// times its inputs, so the colony pays at any tax this table has.
const P2_REF: f64 = 10.0;
const P3_REF: f64 = 200.0;
const ASK: f64 = 10.0;
const BID: f64 = 150.0;
const SUPPLY: i64 = 400;
/// The game clock's fastest scale: a 900 s cycle is 15 s of the test.
const SCALE: u32 = eve_server::game_clock::MAX_SCALE;

fn home() -> Character {
    Character::default()
}

fn catalogue() -> eve_server::celestial::Catalogue {
    let (constellation, region) = (home().constellation_id, home().region_id);
    let station_type = home().station_type_id;
    let (x, y) = (50_000_000_000.0, 100_000_000_000.0);
    eve_server::celestial::Catalogue::parse(&format!(
        "#system\t{SYSTEM}\t{constellation}\t{region}\n\
         station\t{STATION}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{SYSTEM}\t0\t0\n\
         planet\t{PLANET}\t{PT}\t{PLANET_RADIUS}\t{x}\t0.0\t0.0\t{SYSTEM}\t0\t0\n\
         planet\t{PLANET_B}\t{PT_B}\t{PLANET_RADIUS}\t{y}\t0.0\t0.0\t{SYSTEM}\t0\t0\n"
    ))
}

/// The PI table: the chain P0 -> P1 -> P2 -> P3 (so the tiers read off it),
/// the P3 recipe takes the two P2 and only the advanced factory runs it.
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
         schematic\t{S_P2A}\t60\t{ADVANCED}\t{P1_A}=5;{P1_B}=5\t{P2_A}=3\n\
         schematic\t{S_P2B}\t60\t{ADVANCED}\t{P1_A}=4;{P1_B}=4\t{P2_B}=3\n\
         schematic\t{S_P3}\t900\t{ADVANCED}\t{P2_A}=10;{P2_B}=10\t{P3}=3\n\
         resource\t{PT}\t{RES_A}=1.0;{RES_B}=1.0\n\
         resource\t{PT_B}\t{RES_A}=0.5\n\
         itemtype\t{CC}\t1027\t41\t10.0\t500.0\tFixture PI Centre\n\
         itemtype\t{RES_A}\t1032\t42\t0.005\t0.0\tFixture P0 A\n\
         itemtype\t{RES_B}\t1032\t42\t0.005\t0.0\tFixture P0 B\n\
         itemtype\t{P1_A}\t1042\t43\t0.1\t0.0\tFixture P1 A\n\
         itemtype\t{P1_B}\t1042\t43\t0.1\t0.0\tFixture P1 B\n\
         itemtype\t{P2_A}\t1034\t43\t0.2\t0.0\tFixture P2 A\n\
         itemtype\t{P2_B}\t1034\t43\t0.2\t0.0\tFixture P2 B\n\
         itemtype\t{P3}\t1040\t43\t0.6\t0.0\tFixture P3\n\
         customsoffice\t{OFFICE}\t35000.0\t8000.0\t0.1\n"
    ))
    .expect("the invented rows parse")
}

/// The content: the PI table, the skill row for Interplanetary
/// Consolidation, and the price oracle's rows for the three goods.
fn content() -> Content {
    let mut c = Content::fixture();
    let skills = format!(
        "{}skill\t{}\t990710100\t1\t165\t166\t0.01\t\tFixture PI Skill\n",
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
    // The goods as the item store sees them: planetary commodities (43).
    let types = format!(
        "{}type\t{P2_A}\t1034\t43\t1\t1\t0.2\t0.0\t1.0\t1.0\t0\t\t\tFixture P2 A\n\
         type\t{P2_B}\t1034\t43\t1\t1\t0.2\t0.0\t1.0\t1.0\t0\t\t\tFixture P2 B\n\
         type\t{P3}\t1040\t43\t1\t1\t0.6\t0.0\t1.0\t1.0\t0\t\t\tFixture P3\n",
        eve_server::typedata::FIXTURE
    );
    c.insert(
        "typedata",
        eve_server::typedata::Installed::build(
            eve_server::typedata::Table::parse(&types).expect("the type rows parse"),
        ),
    );
    let mut prices = eve_server::econ_prices::Table::fixture();
    prices.prices.extend(
        eve_server::econ_prices::Table::parse(&[
            format!("{P2_A}\t{P2_REF}\tbase\t{P2_REF}\t43\t1034\t\t1"),
            format!("{P2_B}\t{P2_REF}\tbase\t{P2_REF}\t43\t1034\t\t1"),
            format!("{P3}\t{P3_REF}\tbase\t{P3_REF}\t43\t1040\t\t1"),
        ])
        .expect("the prices parse")
        .prices,
    );
    c.insert("econ_prices", prices);
    c
}

fn start() -> SocketAddr {
    let mut c = config();
    let at = |id: i64, name: &str, ship: i64| Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        balance: WALLET,
        station_id: Some(STATION),
        solar_system_id: Some(SYSTEM),
        ..home()
    };
    c.sandbox.characters = vec![
        at(FARMER, "Farmer7", FARMER + 10_000_000),
        at(SELLER, "PiSeller7", 100_000_702),
    ];
    c.sandbox.celestials = catalogue();
    c.content = Some(Arc::new(content()));
    c.time_scale = eve_server::game_clock::GameClock::new(SCALE).expect("a scale");
    serve(c)
}

/// List `n` of `type_id` at `price`.
fn list(s: &mut eve_bot::Conn, type_id: i64, n: i64, price: f64) {
    gm(s, &format!("/createitem {type_id} {n}"));
    let stack = s
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == type_id)
        .expect("the seller's stack");
    s.sell(
        &[SellLine {
            station_id: STATION,
            type_id,
            item_id: stack.item_id,
            price,
            quantity: n,
        }],
        eve_bot::escort::LIST_DAYS,
        0.0,
    )
    .expect("the seller lists");
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

/// **A P3 colony once a P2 colony runs: stocked from the market, fed
/// through the office, its product exported and sold.**
#[test]
fn a_farmer_founds_a_p3_colony_stocks_it_and_exports_and_sells_the_p3() {
    let name = "Farmer7";
    let addr = start();
    let mut seller = login(addr, "PiSeller7");
    list(&mut seller, CC, 2, CC_PRICE);
    list(&mut seller, P2_A, SUPPLY, ASK);
    list(&mut seller, P2_B, SUPPLY, ASK);
    seller
        .buy(STATION, P3, BID, 100, eve_bot::escort::LIST_DAYS, 0.0)
        .expect("the seller bids for the P3");

    let mut f = login(addr, name);
    gm(&mut f, &format!("/createitem {FRIGATE}"));
    let hull = f
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE)
        .expect("the frigate");
    f.board(hull.item_id).expect("boards the frigate");
    let content = content();

    // 1. Look. 2. A P2 colony (no P2 runs yet, so no P3 is offered).
    let first = pi::run(&mut f, Some(&content), name, 0).expect("a first visit");
    assert_eq!(first.surveyed, Some(2), "{}", first.summary());
    let second = pi::run(&mut f, Some(&content), name, 0).expect("a second visit");
    assert_eq!(
        (second.founded.map(|x| x.0), second.p2, second.p3),
        (Some(PLANET), Some(P2_A), None),
        "{}",
        second.summary()
    );

    // 3. With P2 running and a second colony allowed: the P3 colony.
    gm(
        &mut f,
        &format!("/giveskill {} 1", pi::SKILL_INTERPLANETARY_CONSOLIDATION),
    );
    let third = pi::run(&mut f, Some(&content), name, 0).expect("a third visit");
    assert_eq!(
        (third.founded.map(|x| x.0), third.p3),
        (Some(PLANET_B), Some(P3)),
        "{}",
        third.summary()
    );
    let view = colony(&mut f, PLANET_B);
    let types: Vec<i64> = view.pins.iter().map(|p| p.type_id).collect();
    for t in [CC, ADVANCED, PAD] {
        assert!(types.contains(&t), "{types:?}");
    }
    let adv = view.pins.iter().find(|p| p.type_id == ADVANCED).unwrap();
    assert_eq!(adv.schematic_id, Some(S_P3));

    // 4. Stock the missing P2 from the market, carry both onto the pad.
    let fourth = pi::run(&mut f, Some(&content), name, 0).expect("a fourth visit");
    assert!(fourth.stocked > 0, "{}", fourth.summary());
    assert!(fourth.fed > 0, "{}", fourth.summary());
    assert!(
        fourth.after < fourth.before,
        "the asks and the import tax: {}",
        fourth.summary()
    );
    assert_eq!(f.session().station_id(), Some(STATION), "docked at home");

    // The server's simulation turns them into P3 on the pad.
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let view = colony(&mut f, PLANET_B);
        let pad = view.pins.iter().find(|p| p.type_id == PAD).unwrap();
        if pad.contents.get(&P3).is_some_and(|q| *q > 0) {
            break;
        }
        assert!(Instant::now() < deadline, "no P3 on the pad: {view:?}");
        std::thread::sleep(Duration::from_millis(500));
    }

    // 5. Export it through the office and sell it into the bid.
    let fifth = pi::run(&mut f, Some(&content), name, 0).expect("a fifth visit");
    assert!(fifth.exported > 0, "{}", fifth.summary());
    assert!(
        fifth
            .sales
            .iter()
            .any(|s| matches!(s, pi::Sale::Bid(l) if l.type_id == P3)),
        "{}",
        fifth.summary()
    );
    assert_eq!(f.session().station_id(), Some(STATION), "docked at home");
    let left: i64 = f
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .chain(f.cargo().expect("the hold lists"))
        .filter(|r| r.type_id == P3)
        .map(|r| r.quantity)
        .sum();
    assert_eq!(left, 0, "all of it sold");
}
