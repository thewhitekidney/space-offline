//! **The PI farmer** (SOC-B6) over a socket: the first visit sells the
//! hangar's commodities into a bid and reads the system's planets off the
//! overview; the second buys the planet's command centre from a seller's
//! ask, loads it, undocks and founds the templated colony; the third only
//! tends it. Every visit ends docked at home.
//!
//! **Synthetic furniture only** (the no-client-data rule): an invented
//! planet type, planet, pin types, schematic and commodities. The one
//! client id is the link type, [`pi::LINK_TYPE`] (2280 [V]): the server
//! builds every link as it, so the table must carry its row.

use crate::common::{config, gm, login, serve};
use eve_bot::builder::SellLine;
use eve_bot::pi;
use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::model::Character;
use std::net::SocketAddr;
use std::sync::Arc;

// Invented ids and numbers only.
const FARMER: i64 = 90_000_651;
const SELLER: i64 = 90_000_652;
const PT: i64 = 990_660_001;
const CC: i64 = 990_660_011;
const ECU: i64 = 990_660_012;
const FACTORY: i64 = 990_660_015;
const RES: i64 = 990_660_021;
const PROD: i64 = 990_660_022;
const SCHEMATIC: i64 = 99_066;
const PLANET: i64 = 40_966_001;
const PLANET_RADIUS: f64 = 3_000_000.0;
/// The fixture frigate of `typedata::FIXTURE`: a hull with a hold.
const FRIGATE: i64 = 990_000_001;
const WALLET: f64 = 10_000_000.0;
const CC_PRICE: f64 = 1_000.0;
const GOODS: i64 = 30;
const BID: f64 = 50.0;

fn home() -> Character {
    Character::default()
}

fn system() -> i64 {
    home()
        .solar_system_id
        .expect("the default pilot is in a system")
}

fn station() -> i64 {
    home().station_id.expect("the default pilot is docked")
}

fn catalogue() -> eve_server::celestial::Catalogue {
    let (a, st) = (system(), station());
    let (constellation, region) = (home().constellation_id, home().region_id);
    let station_type = home().station_type_id;
    eve_server::celestial::Catalogue::parse(&format!(
        "#system\t{a}\t{constellation}\t{region}\n\
         station\t{st}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{a}\t0\t0\n\
         planet\t{PLANET}\t{PT}\t{PLANET_RADIUS}\t50000000000.0\t0.0\t0.0\t{a}\t0\t0\n"
    ))
}

/// The PI table: a command centre the planet allows (10 m3, so a frigate
/// carries it), an extraction unit, a basic factory, the link, one P0 ->
/// P1 schematic, the planet type's one resource, and the item rows.
fn statics() -> eve_server::planet::Statics {
    let link = pi::LINK_TYPE;
    eve_server::planet::Statics::parse(&format!(
        "pintype\t{CC}\t1027\t500.0\t0.0\t0\t48=1675.0;11=6000.0;1632={PT}.0\n\
         pintype\t{ECU}\t1063\t0.0\t45000.0\t0\t49=400.0;15=2600.0;1690=110.0;1691=550.0;1689=0.3;1632={PT}.0\n\
         pintype\t{FACTORY}\t1028\t0.0\t75000.0\t0\t49=200.0;15=800.0;1632={PT}.0\n\
         pintype\t{link}\t1036\t0.0\t0.0\t0\t49=15.0;15=10.0;1631=1250.0;1633=0.15;1634=0.2;1635=1.4;1636=1.2\n\
         schematic\t{SCHEMATIC}\t1800\t{FACTORY}\t{RES}=3000\t{PROD}=20\n\
         resource\t{PT}\t{RES}=1.0\n\
         itemtype\t{CC}\t1027\t41\t10.0\t500.0\tFixture PI Centre\n\
         itemtype\t{RES}\t1032\t42\t0.005\t0.0\tFixture PI Resource\n\
         itemtype\t{PROD}\t1042\t43\t0.19\t0.0\tFixture PI Product\n"
    ))
    .expect("the invented rows parse")
}

fn content() -> Content {
    let mut c = Content::fixture();
    c.insert(
        eve_server::planet::STATICS.name,
        eve_server::planet::Installed::build(statics()),
    );
    c
}

fn start() -> SocketAddr {
    let mut c = config();
    c.sandbox.characters = vec![
        Character {
            character_id: FARMER,
            name: "Farmer1".to_owned(),
            ship_id: 100_000_651,
            balance: WALLET,
            ..home()
        },
        Character {
            character_id: SELLER,
            name: "PiSeller".to_owned(),
            ship_id: 100_000_652,
            balance: WALLET,
            ..home()
        },
    ];
    c.sandbox.celestials = catalogue();
    c.content = Some(Arc::new(content()));
    serve(c)
}

fn kv<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
    let PyValue::Instance { state, .. } = v else {
        return None;
    };
    let PyValue::Dict(entries) = state.as_ref() else {
        return None;
    };
    entries
        .iter()
        .find(|(_, k)| matches!(k, PyValue::Bytes(b) if b == name.as_bytes()))
        .map(|(v, _)| v)
}

/// **The lane's harness test** (module header).
#[test]
fn a_farmer_surveys_founds_a_templated_colony_then_tends_it() {
    let addr = start();

    // A human stand-in: one command centre listed, a bid for the P1.
    let mut s = login(addr, "PiSeller");
    gm(&mut s, &format!("/createitem {CC} 1"));
    let centre = s
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == CC)
        .expect("the seller's centre");
    s.sell(
        &[SellLine {
            station_id: station(),
            type_id: CC,
            item_id: centre.item_id,
            price: CC_PRICE,
            quantity: 1,
        }],
        pi_list_days(),
        0.0,
    )
    .expect("the seller lists the centre");
    s.buy(station(), PROD, BID, GOODS, pi_list_days(), 0.0)
        .expect("the seller bids for the P1");

    // The farmer's setup: a hull, and P1 left over from an earlier colony.
    let mut f = login(addr, "Farmer1");
    gm(&mut f, &format!("/createitem {FRIGATE}"));
    gm(&mut f, &format!("/createitem {PROD} {GOODS}"));
    let hull = f
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE)
        .expect("the frigate");
    f.board(hull.item_id).expect("boards the frigate");
    let content = content();

    // 1. Sell into the bid, and look at the planets.
    let first = pi::run(&mut f, Some(&content), "Farmer1", 0).expect("a first visit");
    assert_eq!(first.surveyed, Some(1), "{}", first.summary());
    assert!(
        matches!(first.sales.as_slice(), [pi::Sale::Bid(l)] if l.quantity == GOODS),
        "{}",
        first.summary()
    );
    assert!(first.after > first.before, "{}", first.summary());
    assert_eq!(f.session().station_id(), Some(station()), "docked at home");

    // 2. Buy the centre, undock, found the colony, dock.
    let second = pi::run(&mut f, Some(&content), "Farmer1", 0).expect("a second visit");
    let (planet, level, heads) = second
        .founded
        .unwrap_or_else(|| panic!("{}", second.summary()));
    assert_eq!((planet, level), (PLANET, 0), "{}", second.summary());
    assert!(heads >= 1, "{}", second.summary());
    assert_eq!(f.session().station_id(), Some(station()), "docked at home");
    assert!(
        f.cargo()
            .expect("the hold lists")
            .iter()
            .all(|r| r.type_id != CC),
        "the centre left the hold"
    );

    // The colony, as the planet window reads it.
    let info = f
        .moniker_call(
            "planetMgr",
            PyValue::Int(PLANET),
            "GetPlanetInfo",
            vec![],
            vec![],
        )
        .expect("the planet reads")
        .value
        .expect("an answer");
    let view = pi::read_colony(&info).expect("a colony");
    let types: Vec<i64> = view.pins.iter().map(|p| p.type_id).collect();
    assert_eq!(types.len(), 3, "{types:?}");
    for t in [CC, ECU, FACTORY] {
        assert!(types.contains(&t), "{types:?}");
    }
    let ecu = view.pins.iter().find(|p| p.type_id == ECU).unwrap();
    assert_eq!(ecu.program_type, Some(RES));
    assert_eq!(ecu.heads, heads);
    let factory = view.pins.iter().find(|p| p.type_id == FACTORY).unwrap();
    assert_eq!(factory.schematic_id, Some(SCHEMATIC));
    match kv(&info, "routes") {
        Some(PyValue::List(routes)) => assert_eq!(routes.len(), 2),
        other => panic!("routes: {other:?}"),
    }

    // 3. Tend only: no skill for a second planet, nothing expired yet.
    let third = pi::run(&mut f, Some(&content), "Farmer1", 0).expect("a third visit");
    assert_eq!(third.colonies, 1, "{}", third.summary());
    assert_eq!(third.restarted, 0, "{}", third.summary());
    assert!(third.founded.is_none() && third.surveyed.is_none());
    assert_eq!(f.session().station_id(), Some(station()));
}

fn pi_list_days() -> i64 {
    eve_bot::escort::LIST_DAYS
}
