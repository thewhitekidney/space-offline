//! **FE-TRADE over a socket**: two regions that are islands. A seller lists
//! a good cheap in the home region; an NPC corporation bids for it at a
//! station in another region. `GetOrders` answers only the region of the
//! station the caller is docked in, so a trader at home sees no bid and makes
//! no deal; it learns the other region's book by *being there* (the desk's
//! price memory), and the next cycle at home buys, carries the goods across
//! the border, confirms the live bid on arrival and sells into it.
//!
//! **Synthetic furniture only**: invented ids and numbers; two systems, a
//! gate pair, one invented trade good, one invented NPC corporation.

use crate::common::{config, gm, login, serve};
use eve_bot::builder::SellLine;
use eve_bot::price_memory::Desk;
use eve_bot::trader::{self, Costs, Outlet, Setup, SkillUse};
use eve_bot::travel::{self, Map, SECURITY_FLOOR, Trips};
use eve_server::content::Content;
use eve_server::econ_exchange as exchange;
use eve_server::model::Character;
use std::net::SocketAddr;
use std::sync::Arc;

const TRADER: i64 = 90_000_501;
const SELLER: i64 = 90_000_502;
const CAROL: i64 = 90_000_503;
const CORP: i64 = 1_999_851;
const B: i64 = 30_000_005;
const STATION_B: i64 = 60_900_201;
const GATE_A: i64 = 50_900_201;
const GATE_B: i64 = 50_900_202;
const GATE_TYPE: i64 = 990_500_201;
const GOOD: i64 = 990_500_202;
const GOOD_GROUP: i64 = 99_281;
const GOOD_BASE: f64 = 10_000.0;
const CHEAP: f64 = 1_000.0;
const UNITS: i64 = 100;
const FRIGATE: i64 = 990_000_001;
const WALLET: f64 = 1_000_000.0;

fn home() -> Character {
    Character::default()
}

/// The far region: not the home one.
fn region_b() -> i64 {
    home().region_id + 7
}

fn system_a() -> i64 {
    home()
        .solar_system_id
        .expect("the default pilot is in a system")
}

fn station_a() -> i64 {
    home().station_id.expect("the default pilot is docked")
}

fn catalogue() -> eve_server::celestial::Catalogue {
    let (a, home_station) = (system_a(), station_a());
    let constellation = home().constellation_id;
    let (ra, rb) = (home().region_id, region_b());
    let station_type = home().station_type_id;
    eve_server::celestial::Catalogue::parse(&format!(
        "#system\t{a}\t{constellation}\t{ra}\n\
         #system\t{B}\t{constellation}\t{rb}\n\
         station\t{home_station}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{a}\t0\t0\n\
         gate\t{GATE_A}\t{GATE_TYPE}\t2473.0\t1000000.0\t0.0\t0.0\t{a}\t{GATE_B}\t{B}\n\
         station\t{STATION_B}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{B}\t0\t0\n\
         gate\t{GATE_B}\t{GATE_TYPE}\t2473.0\t0.0\t0.0\t-1000000.0\t{B}\t{GATE_A}\t{a}\n"
    ))
}

fn content() -> Content {
    let types = format!(
        "{}\
         type\t{GOOD}\t{GOOD_GROUP}\t{cat}\t1\t1\t1.0\t0.0\t1.0\t1.0\t0\t\t\tFixture Trade Good\n",
        eve_server::typedata::FIXTURE,
        cat = exchange::CATEGORY_COMMODITY,
    );
    let td = eve_server::typedata::Table::parse(&types).expect("the test rows parse");
    let prices = eve_server::econ_prices::Table::parse(&[format!(
        "{GOOD}\t{GOOD_BASE}\tbase\t{GOOD_BASE}\t{}\t{GOOD_GROUP}\t\t1",
        exchange::CATEGORY_COMMODITY
    )])
    .expect("the prices parse");
    let table = exchange::Table::new(
        vec![exchange::Station {
            station_id: STATION_B,
            system_id: B,
            region_id: region_b(),
            owner_id: CORP,
        }],
        &[(CORP, GOOD, -1.0)],
        [(
            GOOD,
            exchange::TypeRow {
                group_id: GOOD_GROUP,
                category_id: exchange::CATEGORY_COMMODITY,
                base_price: GOOD_BASE,
                meta_group_id: None,
                tech_level: None,
            },
        )]
        .into_iter()
        .collect(),
        &[],
    )
    .expect("the exchange builds");
    let mut c = Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(td));
    c.insert("econ_prices", prices);
    c.insert("econ_exchange", table);
    c
}

fn start() -> SocketAddr {
    let mut c = config();
    c.sandbox.characters = vec![
        Character {
            character_id: TRADER,
            name: "Trader5".to_owned(),
            ship_id: 100_000_501,
            balance: WALLET,
            ..home()
        },
        Character {
            character_id: SELLER,
            name: "Seller5".to_owned(),
            ship_id: 100_000_502,
            balance: WALLET,
            ..home()
        },
        // Makes station B one this world knows, in region B.
        Character {
            character_id: CAROL,
            name: "Carol5".to_owned(),
            ship_id: 100_000_503,
            station_id: Some(STATION_B),
            solar_system_id: Some(B),
            region_id: region_b(),
            ..home()
        },
    ];
    c.sandbox.celestials = catalogue();
    c.content = Some(Arc::new(content()));
    serve(c)
}

fn map() -> Map {
    Map::from_catalogue(&catalogue(), None)
        .with_security(system_a(), 1.0)
        .with_security(B, 1.0)
}

/// **The trader closes a regional spread.** At home the book shows a cheap
/// ask and no bid; the first cycle, with an empty memory, does nothing. The
/// trader flies to region B and reads its book there (the memory), flies
/// home, and the next cycle buys at home, carries the goods over the border,
/// confirms the live bid, sells into it and flies home with a profit.
#[test]
fn a_trader_buys_in_one_regions_book_and_sells_in_anothers() {
    let addr = start();

    let mut s = login(addr, "Seller5");
    gm(&mut s, &format!("/createitem {GOOD} {UNITS}"));
    let stack = s
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == GOOD)
        .expect("the seller's goods");
    s.sell(
        &[SellLine {
            station_id: station_a(),
            type_id: GOOD,
            item_id: stack.item_id,
            price: CHEAP,
            quantity: stack.quantity,
        }],
        trader::LIST_DAYS,
        trader::BROKER_FEE,
    )
    .expect("the seller lists");

    let mut t = login(addr, "Trader5");
    gm(&mut t, &format!("/createitem {FRIGATE}"));
    let hull = t
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE)
        .expect("the frigate");
    t.board(hull.item_id).expect("boards the frigate");

    let book = t.market_orders(GOOD).expect("the book reads");
    assert!(
        book.asks.iter().any(|a| a.station_id == station_a()),
        "{book:?}"
    );
    assert!(
        book.bids.is_empty(),
        "the home region's book shows no bid: regions are islands: {book:?}"
    );

    let map = map();
    let content = content();
    let trips = Trips {
        map: &map,
        floor: SECURITY_FLOOR,
    };
    let setup = Setup {
        trips,
        content: Some(&content),
        types: &[GOOD],
        offset: 0,
        couriers: false,
        costs: Costs::default(),
        skills: SkillUse::Given,
    };
    let mut desk = Desk::default();

    // No memory of region B, no deal.
    let first = trader::run_with(&mut t, &setup, &mut desk).expect("a cycle");
    assert!(first.deal.is_none(), "{}", first.summary());
    assert_eq!(first.bought, 0);

    // Be there: the cycle at B writes region B's book into the memory.
    travel::go(&mut t, trips, STATION_B).expect("flies to B");
    let at_b = trader::run_with(&mut t, &setup, &mut desk).expect("a cycle at B");
    assert!(at_b.deal.is_none(), "{}", at_b.summary());
    let seen = desk
        .memory
        .seen(region_b(), GOOD)
        .expect("region B's book is remembered");
    assert_eq!(seen.bid.map(|q| q.station), Some(STATION_B));
    travel::go(&mut t, trips, station_a()).expect("flies home");

    // Now the spread is known.
    let before = t.cash_balance().expect("wallet");
    let got = trader::run_with(&mut t, &setup, &mut desk).expect("a cycle at home");
    let deal = got.deal.expect("a regional deal");
    assert_eq!(deal.outlet, Outlet::Remote, "{}", got.summary());
    assert_eq!(
        (deal.buy_station, deal.sell_station),
        (station_a(), STATION_B),
        "{}",
        got.summary()
    );
    assert_eq!(got.bought, UNITS, "{}", got.summary());
    assert_eq!(got.sold, UNITS, "{}", got.summary());
    assert_eq!(got.jumps, 2, "{}", got.summary());
    assert!(got.after > before, "{}", got.summary());
    assert_eq!(
        t.session().station_id(),
        Some(station_a()),
        "docked at home"
    );
    assert!(t.cargo().expect("the hold lists").is_empty());
}
